//! REST request types.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::common::{Auth, Extensions, Id, NameValue, Script};

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Method {
    #[default]
    #[serde(rename = "GET")]
    Get,
    #[serde(rename = "POST")]
    Post,
    #[serde(rename = "PUT")]
    Put,
    #[serde(rename = "PATCH")]
    Patch,
    #[serde(rename = "DELETE")]
    Delete,
    #[serde(rename = "HEAD")]
    Head,
    #[serde(rename = "OPTIONS")]
    Options,
    #[serde(rename = "TRACE")]
    Trace,
    /// Any other verb (Postman allows them), upper-cased.
    #[serde(rename = "CUSTOM")]
    Custom(String),
}

impl Method {
    pub fn parse(s: &str) -> Self {
        match s.to_ascii_uppercase().as_str() {
            "GET" => Self::Get,
            "POST" => Self::Post,
            "PUT" => Self::Put,
            "PATCH" => Self::Patch,
            "DELETE" => Self::Delete,
            "HEAD" => Self::Head,
            "OPTIONS" => Self::Options,
            "TRACE" => Self::Trace,
            other => Self::Custom(other.to_owned()),
        }
    }

    pub fn as_str(&self) -> &str {
        match self {
            Self::Get => "GET",
            Self::Post => "POST",
            Self::Put => "PUT",
            Self::Patch => "PATCH",
            Self::Delete => "DELETE",
            Self::Head => "HEAD",
            Self::Options => "OPTIONS",
            Self::Trace => "TRACE",
            Self::Custom(s) => s,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParamLocation {
    #[default]
    Query,
    Path,
    Header,
    Cookie,
}

impl ParamLocation {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Query => "query",
            Self::Path => "path",
            Self::Header => "header",
            Self::Cookie => "cookie",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "query" => Self::Query,
            "path" => Self::Path,
            "header" => Self::Header,
            "cookie" => Self::Cookie,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct RestParam {
    pub location: ParamLocation,
    /// Name, value (example), description and the `disabled` flag.
    pub pair: NameValue,
    pub required: bool,
    /// JSON schema of the value, as written in the source.
    pub schema: Option<Value>,
}

impl RestParam {
    pub fn new(location: ParamLocation, name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            location,
            pair: NameValue::new(name, value),
            ..Self::default()
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FieldKind {
    #[default]
    Text,
    File,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct FormField {
    /// For a file field, `pair.value` is the file path.
    pub pair: NameValue,
    pub kind: FieldKind,
    pub content_type: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum BodyContent {
    Raw {
        text: String,
        /// `json`, `xml`, `html`, `text`, ...
        language: Option<String>,
    },
    FormUrlEncoded {
        fields: Vec<FormField>,
    },
    Multipart {
        fields: Vec<FormField>,
    },
    Binary {
        file: Option<String>,
    },
}

impl Default for BodyContent {
    fn default() -> Self {
        Self::Raw {
            text: String::new(),
            language: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct RestBody {
    pub content: BodyContent,
    /// Media type, when the source states one (OpenAPI always does).
    pub content_type: Option<String>,
    pub schema: Option<Value>,
    pub description: Option<String>,
    pub required: bool,
    /// Further media types of the source, keyed by media type, as written.
    pub other_media: Map<String, Value>,
    /// Unknown keys of the primary media type object (`examples`, `encoding`, ...).
    pub media_extensions: Extensions,
    pub extensions: Extensions,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct RestResponse {
    pub id: Id,
    pub name: String,
    /// Status code as text: `200`, `2XX`, `default`.
    pub code: String,
    /// Reason phrase (Postman `status`) or OpenAPI `description`.
    pub status_text: Option<String>,
    pub headers: Vec<RestParam>,
    pub content_type: Option<String>,
    pub body: Option<String>,
    pub schema: Option<Value>,
    /// Postman `_postman_previewlanguage`.
    pub language: Option<String>,
    /// Postman `originalRequest`, as written.
    pub original_request: Option<Value>,
    pub other_media: Map<String, Value>,
    pub media_extensions: Extensions,
    pub extensions: Extensions,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct RestSettings {
    pub follow_redirects: Option<bool>,
    pub verify_tls: Option<bool>,
    /// Postman `request.proxy`, as written.
    pub proxy: Option<Value>,
    /// Postman `request.certificate`, as written.
    pub certificate: Option<Value>,
    pub extensions: Extensions,
}

impl RestSettings {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct OperationMeta {
    pub operation_id: Option<String>,
    pub summary: Option<String>,
    pub deprecated: bool,
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct RestRequest {
    pub method: Method,
    /// Base URL as written, e.g. `{{baseUrl}}` or `https://api.example.com`. `None`
    /// uses the project's servers.
    pub server: Option<String>,
    /// Path in `{id}` form, starting with `/`.
    pub path: String,
    /// Query, path, header and cookie parameters, in source order.
    pub params: Vec<RestParam>,
    pub body: Option<RestBody>,
    /// `None` inherits from the parent.
    pub auth: Option<Auth>,
    pub scripts: Vec<Script>,
    pub responses: Vec<RestResponse>,
    pub settings: RestSettings,
    pub meta: OperationMeta,
}

impl RestRequest {
    pub fn new(method: Method, path: impl Into<String>) -> Self {
        Self {
            method,
            path: path.into(),
            ..Self::default()
        }
    }

    pub fn params_in(&self, location: ParamLocation) -> impl Iterator<Item = &RestParam> {
        self.params.iter().filter(move |p| p.location == location)
    }
}
