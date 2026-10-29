//! Converting between JSON and protobuf for dynamic calls.

use prost::Message;
use prost_reflect::{DynamicMessage, MessageDescriptor, SerializeOptions};
use tonic::Status;
use tonic::codec::{Codec, DecodeBuf, Decoder, EncodeBuf, Encoder};

use super::{CallKind, GrpcError};

/// Encodes and decodes the messages of one method; the schema is known at run time, so the
/// messages are [`DynamicMessage`]s.
pub struct DynamicCodec {
    output: MessageDescriptor,
}

impl DynamicCodec {
    pub fn new(output: MessageDescriptor) -> Self {
        Self { output }
    }
}

impl Codec for DynamicCodec {
    type Encode = DynamicMessage;
    type Decode = DynamicMessage;
    type Encoder = DynamicEncoder;
    type Decoder = DynamicDecoder;

    fn encoder(&mut self) -> Self::Encoder {
        DynamicEncoder
    }

    fn decoder(&mut self) -> Self::Decoder {
        DynamicDecoder(self.output.clone())
    }
}

pub struct DynamicEncoder;

impl Encoder for DynamicEncoder {
    type Item = DynamicMessage;
    type Error = Status;

    fn encode(&mut self, item: Self::Item, dst: &mut EncodeBuf<'_>) -> Result<(), Self::Error> {
        item.encode(dst)
            .map_err(|e| Status::internal(format!("cannot encode message: {e}")))
    }
}

pub struct DynamicDecoder(MessageDescriptor);

impl Decoder for DynamicDecoder {
    type Item = DynamicMessage;
    type Error = Status;

    fn decode(&mut self, src: &mut DecodeBuf<'_>) -> Result<Option<Self::Item>, Self::Error> {
        let mut message = DynamicMessage::new(self.0.clone());
        message
            .merge(src)
            .map_err(|e| Status::internal(format!("cannot decode message: {e}")))?;
        Ok(Some(message))
    }
}

/// Turns the request body into the messages to send, checking everything before the call
/// is opened so that nothing is sent when the body is wrong.
///
/// A missing or blank body means one empty message (or none for streaming requests).
pub fn parse_body(
    kind: CallKind,
    input: &MessageDescriptor,
    body: Option<&str>,
) -> Result<Vec<DynamicMessage>, GrpcError> {
    let invalid = |message: String| GrpcError::Invalid(message);
    let text = body.map(str::trim).filter(|t| !t.is_empty());

    let value: serde_json::Value = match text {
        Some(text) => serde_json::from_str(text).map_err(|e| {
            invalid(format!(
                "the body is not valid JSON (line {}, column {}): {e}",
                e.line(),
                e.column()
            ))
        })?,
        None if kind.streams_request() => serde_json::Value::Array(Vec::new()),
        None => serde_json::json!({}),
    };

    let items = match (kind.streams_request(), value) {
        (true, serde_json::Value::Array(items)) => items,
        (false, object @ serde_json::Value::Object(_)) => vec![object],
        (true, _) => {
            return Err(invalid(
                "client and bidirectional streaming methods take a JSON array of objects, \
                 one per message"
                    .into(),
            ));
        }
        (false, _) => {
            return Err(invalid(
                "unary and server streaming methods take one JSON object".into(),
            ));
        }
    };

    items
        .into_iter()
        .enumerate()
        .map(|(i, item)| {
            // The path says which field a value was rejected for.
            let mut track = serde_path_to_error::Track::new();
            let deserializer = serde_path_to_error::Deserializer::new(item, &mut track);
            DynamicMessage::deserialize(input.clone(), deserializer).map_err(|e| {
                let path = track.path().to_string();
                let reason = if path == "." {
                    e.to_string()
                } else {
                    format!("field `{path}`: {e}")
                };
                if kind.streams_request() {
                    invalid(format!("message {}: {reason}", i + 1))
                } else {
                    invalid(reason)
                }
            })
        })
        .collect()
}

/// A response message as compact JSON with every field present, including defaults.
pub fn message_to_json(message: &DynamicMessage) -> String {
    let options = SerializeOptions::new().skip_default_fields(false);
    let mut out = Vec::new();
    let mut serializer = serde_json::Serializer::new(&mut out);
    match message.serialize_with_options(&mut serializer, &options) {
        Ok(()) => String::from_utf8(out).unwrap_or_default(),
        Err(e) => format!("{{\"error\":\"cannot render message: {e}\"}}"),
    }
}
