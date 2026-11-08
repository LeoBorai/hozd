//! Request bodies and how they go on the wire.
//!
//! [`Body`] is transport independent: it knows its `Content-Type` and how to turn itself
//! into bytes ([`Body::encode`]), so any request type that carries a payload can reuse it.

use std::fmt;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::ser::{SerializeMap, SerializeSeq};
use serde::{Deserialize, Serialize, Serializer};

/// Prefix of a [`Body`] stored as text; it cannot start a real body, which has no NULs.
const STORED_PREFIX: &str = "\0hozd-body:";

/// Which kind of payload a [`Body`] is; this is what the user picks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BodyKind {
    /// Free text, sent as `text/plain`.
    Text,
    /// A JSON document built field by field, sent as `application/json`.
    Json,
    /// A JSON document typed by hand, sent as `application/json`.
    #[default]
    JsonRaw,
    /// `application/x-www-form-urlencoded` fields.
    Form,
    /// `multipart/form-data` fields and files.
    Multipart,
    /// One file, sent as `application/octet-stream`.
    File,
}

impl BodyKind {
    pub const ALL: [BodyKind; 6] = [
        BodyKind::Text,
        BodyKind::Json,
        BodyKind::JsonRaw,
        BodyKind::Form,
        BodyKind::Multipart,
        BodyKind::File,
    ];

    pub fn label(self) -> &'static str {
        match self {
            BodyKind::Text => "Text",
            BodyKind::Json => "JSON",
            BodyKind::JsonRaw => "JSON (RAW)",
            BodyKind::Form => "Form URL Encoded",
            BodyKind::Multipart => "Multipart",
            BodyKind::File => "File",
        }
    }

    /// The `Content-Type` a body of this kind is sent with; multipart also carries a
    /// boundary, see [`Body::encode`].
    pub fn content_type(self) -> &'static str {
        match self {
            BodyKind::Text => "text/plain",
            BodyKind::Json | BodyKind::JsonRaw => "application/json",
            BodyKind::Form => "application/x-www-form-urlencoded",
            BodyKind::Multipart => "multipart/form-data",
            BodyKind::File => "application/octet-stream",
        }
    }

    /// The next kind in [`ALL`](Self::ALL), wrapping around.
    pub fn next(self) -> BodyKind {
        let i = Self::ALL.iter().position(|k| *k == self).unwrap_or(0);
        Self::ALL[(i + 1) % Self::ALL.len()]
    }
}

impl fmt::Display for BodyKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// A JSON value whose object members keep the order they were added in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum JsonValue {
    String(String),
    Number(serde_json::Number),
    Bool(bool),
    Null,
    Object(Vec<(String, JsonValue)>),
    Array(Vec<JsonValue>),
}

impl JsonValue {
    /// Compact JSON text.
    pub fn to_json(&self) -> String {
        serde_json::to_string(&Plain(self)).expect("a JSON tree serializes")
    }

    /// Indented JSON text.
    pub fn to_json_pretty(&self) -> String {
        serde_json::to_string_pretty(&Plain(self)).expect("a JSON tree serializes")
    }

    /// The tree of `text`; object members keep the order of the text.
    pub fn parse(text: &str) -> Result<JsonValue, String> {
        serde_json::from_str::<Ordered>(text)
            .map(|o| o.0)
            .map_err(|e| e.to_string())
    }
}

/// Serializes a [`JsonValue`] as the plain JSON it stands for.
struct Plain<'a>(&'a JsonValue);

impl Serialize for Plain<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self.0 {
            JsonValue::String(v) => s.serialize_str(v),
            JsonValue::Number(v) => v.serialize(s),
            JsonValue::Bool(v) => s.serialize_bool(*v),
            JsonValue::Null => s.serialize_unit(),
            JsonValue::Object(members) => {
                let mut map = s.serialize_map(Some(members.len()))?;
                for (k, v) in members {
                    map.serialize_entry(k, &Plain(v))?;
                }
                map.end()
            }
            JsonValue::Array(items) => {
                let mut seq = s.serialize_seq(Some(items.len()))?;
                for v in items {
                    seq.serialize_element(&Plain(v))?;
                }
                seq.end()
            }
        }
    }
}

/// Deserializes plain JSON into a [`JsonValue`], keeping the member order.
struct Ordered(JsonValue);

impl<'de> Deserialize<'de> for Ordered {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        use serde::de::{MapAccess, SeqAccess, Visitor};

        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = JsonValue;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("any JSON value")
            }

            fn visit_bool<E>(self, v: bool) -> Result<JsonValue, E> {
                Ok(JsonValue::Bool(v))
            }
            fn visit_i64<E>(self, v: i64) -> Result<JsonValue, E> {
                Ok(JsonValue::Number(v.into()))
            }
            fn visit_u64<E>(self, v: u64) -> Result<JsonValue, E> {
                Ok(JsonValue::Number(v.into()))
            }
            fn visit_f64<E>(self, v: f64) -> Result<JsonValue, E> {
                Ok(serde_json::Number::from_f64(v).map_or(JsonValue::Null, JsonValue::Number))
            }
            fn visit_str<E>(self, v: &str) -> Result<JsonValue, E> {
                Ok(JsonValue::String(v.to_owned()))
            }
            fn visit_unit<E>(self) -> Result<JsonValue, E> {
                Ok(JsonValue::Null)
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<JsonValue, A::Error> {
                let mut items = Vec::new();
                while let Some(Ordered(v)) = seq.next_element()? {
                    items.push(v);
                }
                Ok(JsonValue::Array(items))
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<JsonValue, A::Error> {
                let mut members = Vec::new();
                while let Some((k, Ordered(v))) = map.next_entry::<String, Ordered>()? {
                    members.push((k, v));
                }
                Ok(JsonValue::Object(members))
            }
        }
        d.deserialize_any(V).map(Ordered)
    }
}

/// A `name=value` pair of a URL encoded form.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Field {
    pub name: String,
    pub value: String,
}

impl Field {
    pub fn new(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: value.into(),
        }
    }
}

/// The content of one multipart part.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum PartValue {
    Text(String),
    /// A path; the file is read when the request is sent.
    File(String),
}

impl PartValue {
    /// `@path` is a file, `@@text` is the text `@text`, anything else is text.
    pub fn parse(input: &str) -> PartValue {
        match input.strip_prefix('@') {
            Some(rest) if rest.starts_with('@') => PartValue::Text(rest.to_owned()),
            Some(path) => PartValue::File(path.to_owned()),
            None => PartValue::Text(input.to_owned()),
        }
    }
}

impl fmt::Display for PartValue {
    /// The form [`parse`](Self::parse) reads back.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PartValue::File(path) => write!(f, "@{path}"),
            PartValue::Text(text) if text.starts_with('@') => write!(f, "@{text}"),
            PartValue::Text(text) => f.write_str(text),
        }
    }
}

/// One field of a multipart body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Part {
    pub name: String,
    pub value: PartValue,
}

impl Part {
    pub fn text(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: PartValue::Text(value.into()),
        }
    }

    pub fn file(name: impl Into<String>, path: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: PartValue::File(path.into()),
        }
    }
}

/// A request payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum Body {
    Text(String),
    Json(JsonValue),
    JsonRaw(String),
    Form(Vec<Field>),
    Multipart(Vec<Part>),
    /// A path; the file is read when the request is sent.
    File(String),
}

impl From<String> for Body {
    /// A bare string body is the typed-by-hand JSON the body editor always produced.
    fn from(text: String) -> Self {
        Body::JsonRaw(text)
    }
}

impl From<&str> for Body {
    fn from(text: &str) -> Self {
        Body::JsonRaw(text.to_owned())
    }
}

/// The payload as bytes, with the `Content-Type` to send it with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Encoded {
    pub content_type: String,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
pub enum BodyError {
    #[error("cannot read `{path}`: {reason}")]
    File { path: String, reason: String },
}

impl Body {
    pub fn kind(&self) -> BodyKind {
        match self {
            Body::Text(_) => BodyKind::Text,
            Body::Json(_) => BodyKind::Json,
            Body::JsonRaw(_) => BodyKind::JsonRaw,
            Body::Form(_) => BodyKind::Form,
            Body::Multipart(_) => BodyKind::Multipart,
            Body::File(_) => BodyKind::File,
        }
    }

    /// The text of a body that is text, whether plain or raw JSON.
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Body::Text(t) | Body::JsonRaw(t) => Some(t),
            _ => None,
        }
    }

    /// The body as the user would read it: JSON is indented, forms are `name=value` lines,
    /// multipart parts are `name: value` lines (files as `@path`), a file is `@path`.
    pub fn preview(&self) -> String {
        match self {
            Body::Text(t) | Body::JsonRaw(t) => t.clone(),
            Body::Json(v) => v.to_json_pretty(),
            Body::Form(fields) => fields
                .iter()
                .map(|f| format!("{}={}", f.name, f.value))
                .collect::<Vec<_>>()
                .join("\n"),
            Body::Multipart(parts) => parts
                .iter()
                .map(|p| format!("{}: {}", p.name, p.value))
                .collect::<Vec<_>>()
                .join("\n"),
            Body::File(path) => format!("@{}", file_path(path)),
        }
    }

    /// Fails when a file the body refers to cannot be read; reads nothing else.
    pub fn check(&self) -> Result<(), BodyError> {
        let paths: Vec<&str> = match self {
            Body::File(path) => vec![path],
            Body::Multipart(parts) => parts
                .iter()
                .filter_map(|p| match &p.value {
                    PartValue::File(path) => Some(path.as_str()),
                    PartValue::Text(_) => None,
                })
                .collect(),
            _ => Vec::new(),
        };
        for path in paths {
            let resolved = expand_home(file_path(path));
            match std::fs::metadata(&resolved) {
                Ok(m) if m.is_file() => {}
                Ok(_) => return Err(file_error(path, "not a file")),
                Err(e) => return Err(file_error(path, e)),
            }
        }
        Ok(())
    }

    /// The bytes to send. Files are read here.
    pub fn encode(&self) -> Result<Encoded, BodyError> {
        self.encode_with_boundary(&new_boundary())
    }

    /// Like [`encode`](Self::encode) with the multipart boundary chosen by the caller.
    pub fn encode_with_boundary(&self, boundary: &str) -> Result<Encoded, BodyError> {
        let kind = self.kind();
        let (content_type, bytes) = match self {
            Body::Text(t) | Body::JsonRaw(t) => (kind.content_type().to_owned(), t.clone().into()),
            Body::Json(v) => (kind.content_type().to_owned(), v.to_json().into_bytes()),
            Body::Form(fields) => (
                kind.content_type().to_owned(),
                form_encode(fields).into_bytes(),
            ),
            Body::Multipart(parts) => (
                format!("{}; boundary={boundary}", kind.content_type()),
                multipart_encode(parts, boundary)?,
            ),
            Body::File(path) => (kind.content_type().to_owned(), read(file_path(path))?),
        };
        Ok(Encoded {
            content_type,
            bytes,
        })
    }

    /// The text to keep this body in a database column. Raw JSON is kept as is, which is
    /// how bodies were stored before there were body types.
    pub fn to_stored(&self) -> String {
        match self {
            Body::JsonRaw(t) if !t.starts_with(STORED_PREFIX) => t.clone(),
            _ => format!(
                "{STORED_PREFIX}{}",
                serde_json::to_string(self).expect("a body serializes")
            ),
        }
    }

    /// Inverse of [`to_stored`](Self::to_stored); text that is not that form is raw JSON.
    pub fn from_stored(stored: &str) -> Body {
        stored
            .strip_prefix(STORED_PREFIX)
            .and_then(|json| serde_json::from_str(json).ok())
            .unwrap_or_else(|| Body::JsonRaw(stored.to_owned()))
    }

    /// Whether sending this body sends nothing: empty text, no fields, no file.
    pub fn is_empty(&self) -> bool {
        match self {
            Body::Text(t) | Body::JsonRaw(t) => t.is_empty(),
            Body::File(path) => file_path(path).is_empty(),
            Body::Json(JsonValue::Object(m)) => m.is_empty(),
            Body::Json(JsonValue::Array(a)) => a.is_empty(),
            Body::Json(_) => false,
            Body::Form(f) => f.is_empty(),
            Body::Multipart(p) => p.is_empty(),
        }
    }
}

/// `path` without the `@` that marks it as a file.
fn file_path(path: &str) -> &str {
    path.trim().strip_prefix('@').unwrap_or(path.trim())
}

fn file_error(path: &str, reason: impl fmt::Display) -> BodyError {
    BodyError::File {
        path: path.to_owned(),
        reason: reason.to_string(),
    }
}

fn read(path: &str) -> Result<Vec<u8>, BodyError> {
    std::fs::read(expand_home(path)).map_err(|e| file_error(path, e))
}

/// `path` with a leading `~` or `~/` replaced by the home directory.
pub fn expand_home(path: &str) -> PathBuf {
    let home = || dirs::home_dir().unwrap_or_default();
    match path.strip_prefix("~") {
        Some("") => home(),
        Some(rest) if rest.starts_with('/') => home().join(&rest[1..]),
        _ => PathBuf::from(path),
    }
}

fn form_encode(fields: &[Field]) -> String {
    let mut url = reqwest::Url::parse("http://form.invalid/").expect("a constant URL parses");
    url.query_pairs_mut()
        .extend_pairs(fields.iter().map(|f| (f.name.as_str(), f.value.as_str())));
    url.query().unwrap_or_default().to_owned()
}

fn new_boundary() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    format!("hozd-{nanos:032x}")
}

fn multipart_encode(parts: &[Part], boundary: &str) -> Result<Vec<u8>, BodyError> {
    let mut out = Vec::new();
    for part in parts {
        out.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        let name = quote(&part.name);
        match &part.value {
            PartValue::Text(text) => {
                out.extend_from_slice(
                    format!("Content-Disposition: form-data; name=\"{name}\"\r\n\r\n").as_bytes(),
                );
                out.extend_from_slice(text.as_bytes());
            }
            PartValue::File(path) => {
                let path = path.trim();
                let file_name = Path::new(path)
                    .file_name()
                    .map_or_else(|| path.to_owned(), |n| n.to_string_lossy().into_owned());
                out.extend_from_slice(
                    format!(
                        "Content-Disposition: form-data; name=\"{name}\"; filename=\"{}\"\r\n\
                         Content-Type: {}\r\n\r\n",
                        quote(&file_name),
                        mime_for(path)
                    )
                    .as_bytes(),
                );
                out.extend_from_slice(&read(path)?);
            }
        }
        out.extend_from_slice(b"\r\n");
    }
    out.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    Ok(out)
}

/// `value` made safe for a quoted `Content-Disposition` parameter.
fn quote(value: &str) -> String {
    value
        .replace('"', "%22")
        .replace('\r', "%0D")
        .replace('\n', "%0A")
}

/// The media type for the file's extension; `application/octet-stream` when unknown.
fn mime_for(path: &str) -> &'static str {
    let ext = Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    match ext.as_deref() {
        Some("json") => "application/json",
        Some("txt" | "md" | "log") => "text/plain",
        Some("csv") => "text/csv",
        Some("html" | "htm") => "text/html",
        Some("xml") => "application/xml",
        Some("pdf") => "application/pdf",
        Some("zip") => "application/zip",
        Some("gz") => "application/gzip",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("svg") => "image/svg+xml",
        Some("mp3") => "audio/mpeg",
        Some("mp4") => "video/mp4",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn object(members: &[(&str, JsonValue)]) -> JsonValue {
        JsonValue::Object(
            members
                .iter()
                .map(|(k, v)| ((*k).to_owned(), v.clone()))
                .collect(),
        )
    }

    #[test]
    fn json_keeps_member_order_and_every_value_type() {
        let v = object(&[
            ("z", JsonValue::String("s".into())),
            ("n", JsonValue::Number(3.into())),
            ("o", object(&[("b", JsonValue::Bool(true))])),
            (
                "a",
                JsonValue::Array(vec![JsonValue::Null, JsonValue::Number(1.into())]),
            ),
        ]);
        assert_eq!(
            v.to_json(),
            r#"{"z":"s","n":3,"o":{"b":true},"a":[null,1]}"#
        );
        assert_eq!(JsonValue::parse(&v.to_json()).unwrap(), v);
        assert!(v.to_json_pretty().contains("\n  \"n\": 3"));
    }

    #[test]
    fn body_kinds_set_their_content_type() {
        let cases = [
            (Body::Text("hi".into()), "text/plain"),
            (Body::Json(JsonValue::Null), "application/json"),
            (Body::JsonRaw("{}".into()), "application/json"),
            (
                Body::Form(vec![Field::new("a", "1")]),
                "application/x-www-form-urlencoded",
            ),
        ];
        for (body, content_type) in cases {
            assert_eq!(body.encode().unwrap().content_type, content_type);
        }
    }

    #[test]
    fn form_values_are_percent_encoded() {
        let body = Body::Form(vec![Field::new("q", "a b&c"), Field::new("é", "=")]);
        assert_eq!(
            String::from_utf8(body.encode().unwrap().bytes).unwrap(),
            "q=a+b%26c&%C3%A9=%3D"
        );
    }

    #[test]
    fn multipart_has_text_and_file_parts() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("pic.png");
        std::fs::write(&file, b"PNG").unwrap();
        let body = Body::Multipart(vec![
            Part::text("title", "hi"),
            Part::file("pic", file.to_string_lossy()),
        ]);
        let encoded = body.encode_with_boundary("XX").unwrap();
        assert_eq!(encoded.content_type, "multipart/form-data; boundary=XX");
        assert_eq!(
            String::from_utf8(encoded.bytes).unwrap(),
            "--XX\r\nContent-Disposition: form-data; name=\"title\"\r\n\r\nhi\r\n\
             --XX\r\nContent-Disposition: form-data; name=\"pic\"; filename=\"pic.png\"\r\n\
             Content-Type: image/png\r\n\r\nPNG\r\n--XX--\r\n"
        );
    }

    #[test]
    fn at_marks_a_file_and_double_at_escapes_it() {
        assert_eq!(PartValue::parse("@a/b"), PartValue::File("a/b".into()));
        assert_eq!(PartValue::parse("@@a"), PartValue::Text("@a".into()));
        assert_eq!(PartValue::parse("a@b"), PartValue::Text("a@b".into()));
        for input in ["@a/b", "@@a", "plain"] {
            assert_eq!(PartValue::parse(input).to_string(), input);
        }
    }

    #[test]
    fn a_file_body_is_sent_as_octet_stream() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("x.bin");
        std::fs::write(&file, [0u8, 1, 2]).unwrap();
        let body = Body::File(format!("@{}", file.display()));
        assert!(body.check().is_ok());
        let encoded = body.encode().unwrap();
        assert_eq!(encoded.content_type, "application/octet-stream");
        assert_eq!(encoded.bytes, [0, 1, 2]);
    }

    #[test]
    fn a_missing_file_is_an_error() {
        let body = Body::Multipart(vec![Part::file("f", "/definitely/not/here")]);
        assert!(body.check().is_err());
        assert!(body.encode().is_err());
    }

    #[test]
    fn bodies_survive_storage() {
        let bodies = [
            Body::Text("t".into()),
            Body::Json(object(&[("a", JsonValue::Null)])),
            Body::JsonRaw("{\"a\": 1}".into()),
            Body::Form(vec![Field::new("a", "b")]),
            Body::Multipart(vec![Part::text("a", "b"), Part::file("f", "/x")]),
            Body::File("/x".into()),
        ];
        for body in bodies {
            assert_eq!(Body::from_stored(&body.to_stored()), body);
        }
        // bodies stored before body types existed are plain text
        assert_eq!(Body::JsonRaw("{}".into()).to_stored(), "{}");
        assert_eq!(Body::from_stored("{}"), Body::JsonRaw("{}".into()));
    }
}
