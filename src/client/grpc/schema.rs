use prost_reflect::{DescriptorPool, Kind, MessageDescriptor, MethodDescriptor};
use serde_json::{Map, Value, json};

/// Whether a method streams requests, responses, both or neither.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallKind {
    Unary,
    ServerStreaming,
    ClientStreaming,
    Bidirectional,
}

impl CallKind {
    pub fn of(method: &MethodDescriptor) -> Self {
        match (method.is_client_streaming(), method.is_server_streaming()) {
            (false, false) => CallKind::Unary,
            (false, true) => CallKind::ServerStreaming,
            (true, false) => CallKind::ClientStreaming,
            (true, true) => CallKind::Bidirectional,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            CallKind::Unary => "unary",
            CallKind::ServerStreaming => "server-stream",
            CallKind::ClientStreaming => "client-stream",
            CallKind::Bidirectional => "bidi-stream",
        }
    }

    /// Whether the request body is a JSON array of messages.
    pub fn streams_request(self) -> bool {
        matches!(self, CallKind::ClientStreaming | CallKind::Bidirectional)
    }

    pub fn streams_response(self) -> bool {
        matches!(self, CallKind::ServerStreaming | CallKind::Bidirectional)
    }
}

/// One callable method of a loaded schema.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MethodInfo {
    pub service: String,
    pub name: String,
    pub kind: CallKind,
}

impl MethodInfo {
    /// `package.Service/Method`.
    pub fn full_name(&self) -> String {
        format!("{}/{}", self.service, self.name)
    }
}

/// The services and messages a server speaks, from reflection or from local files.
#[derive(Debug, Clone, Default)]
pub struct Schema {
    pool: DescriptorPool,
    methods: Vec<MethodInfo>,
}

impl Schema {
    pub fn from_pool(pool: DescriptorPool) -> Self {
        let mut methods = Vec::new();
        for service in pool.services() {
            for m in service.methods() {
                methods.push(MethodInfo {
                    service: service.full_name().to_owned(),
                    name: m.name().to_owned(),
                    kind: CallKind::of(&m),
                });
            }
        }
        methods.sort_by(|a, b| (&a.service, &a.name).cmp(&(&b.service, &b.name)));
        Self { pool, methods }
    }

    pub fn pool(&self) -> &DescriptorPool {
        &self.pool
    }

    /// Sorted by service, then method name.
    pub fn methods(&self) -> &[MethodInfo] {
        &self.methods
    }

    pub fn service_count(&self) -> usize {
        self.pool.services().count()
    }

    /// Finds `package.Service/Method`.
    pub fn method(&self, full_name: &str) -> Option<MethodDescriptor> {
        let (service, method) = full_name.split_once('/')?;
        self.pool
            .get_service_by_name(service)?
            .methods()
            .find(|m| m.name() == method)
    }

    /// A JSON body for `full_name` listing every field of its input message with a default
    /// value; an array of one such object when the method streams requests.
    pub fn skeleton(&self, full_name: &str) -> Option<String> {
        let method = self.method(full_name)?;
        let message = message_skeleton(&method.input(), 0);
        let body = if CallKind::of(&method).streams_request() {
            json!([message])
        } else {
            message
        };
        serde_json::to_string_pretty(&body).ok()
    }
}

/// Message fields nest one level; deeper ones are `null`, which also ends recursion.
const SKELETON_DEPTH: u8 = 1;

fn message_skeleton(message: &MessageDescriptor, depth: u8) -> Value {
    let mut object = Map::new();
    for field in message.fields() {
        // Filling every member of a oneof would set more than one.
        if field.containing_oneof().is_some_and(|o| !o.is_synthetic()) {
            continue;
        }
        let value = if field.is_map() {
            json!({})
        } else if field.is_list() {
            json!([])
        } else {
            kind_default(&field.kind(), depth)
        };
        object.insert(field.json_name().to_owned(), value);
    }
    Value::Object(object)
}

fn kind_default(kind: &Kind, depth: u8) -> Value {
    match kind {
        Kind::Double | Kind::Float => json!(0.0),
        Kind::Int32
        | Kind::Int64
        | Kind::Uint32
        | Kind::Uint64
        | Kind::Sint32
        | Kind::Sint64
        | Kind::Fixed32
        | Kind::Fixed64
        | Kind::Sfixed32
        | Kind::Sfixed64 => json!(0),
        Kind::Bool => json!(false),
        Kind::String | Kind::Bytes => json!(""),
        Kind::Enum(e) => json!(e.default_value().name()),
        Kind::Message(m) => match well_known_default(m) {
            Some(value) => value,
            None if depth < SKELETON_DEPTH => message_skeleton(m, depth + 1),
            None => Value::Null,
        },
    }
}

/// `google.protobuf` types have their own JSON form, not the field-by-field one.
fn well_known_default(message: &MessageDescriptor) -> Option<Value> {
    if message.package_name() != "google.protobuf" {
        return None;
    }
    Some(match message.name() {
        "Timestamp" => json!("1970-01-01T00:00:00Z"),
        "Duration" => json!("0s"),
        "FieldMask" => json!(""),
        "Empty" => json!({}),
        "DoubleValue" | "FloatValue" | "Int32Value" | "Int64Value" | "UInt32Value"
        | "UInt64Value" => json!(0),
        "BoolValue" => json!(false),
        "StringValue" | "BytesValue" => json!(""),
        _ => Value::Null,
    })
}
