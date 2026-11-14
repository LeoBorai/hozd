//! Types shared by every API kind, plus the small helpers the converters use.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use thiserror::Error;

/// Stable identifier of a model entity.
pub type Id = String;

/// Fields the model does not cover. Converters store them here on import and write
/// them back on export.
///
/// Keys are the original key names, at the location the entity maps to. A key that
/// belongs to a nested object of the source is prefixed with that object's name
/// (`request.`, `url.`, `script.`, `info.`, `pathItem.`, `responses.`).
pub type Extensions = Map<String, Value>;

/// Prefix of ids derived from the source document instead of read from it.
pub const DERIVED_PREFIX: &str = "gen-";

/// A random, UUID-shaped id for entities created natively.
pub fn new_id() -> Id {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    let mut b = [0u8; 16];
    for chunk in b.chunks_mut(8) {
        let mut h = RandomState::new().build_hasher();
        h.write_u64(COUNTER.fetch_add(1, Ordering::Relaxed));
        chunk.copy_from_slice(&h.finish().to_be_bytes());
    }
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h = |r: std::ops::Range<usize>| b[r].iter().map(|x| format!("{x:02x}")).collect::<String>();
    format!(
        "{}-{}-{}-{}-{}",
        h(0..4),
        h(4..6),
        h(6..8),
        h(8..10),
        h(10..16)
    )
}

/// A deterministic id for an entity whose source has none. The same document always
/// yields the same ids, so they are stable across imports.
pub fn derive_id(parts: &[&str]) -> Id {
    let mut hasher = Sha256::new();
    for p in parts {
        hasher.update(p.as_bytes());
        hasher.update([0u8]);
    }
    let digest = hasher.finalize();
    let hex: String = digest.iter().take(12).map(|b| format!("{b:02x}")).collect();
    format!("{DERIVED_PREFIX}{hex}")
}

/// Whether `id` came from [`derive_id`]. Derived ids are not written to documents:
/// importing the result derives the same value again.
pub fn is_derived(id: &str) -> bool {
    id.starts_with(DERIVED_PREFIX)
}

#[derive(Debug, Error)]
pub enum ConvertError {
    #[error("invalid document: {0}")]
    Invalid(String),
    #[error("unsupported format: {0}")]
    Unsupported(String),
    #[error("invalid JSON: {0}")]
    Json(#[from] serde_json::Error),
}

/// A document produced by an exporter, with notes on what could not be represented.
#[derive(Debug, Clone, PartialEq)]
pub struct Exported {
    pub document: Value,
    pub warnings: Vec<String>,
}

/// The format a project was imported from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceFormat {
    /// Created in hozd, not imported.
    #[default]
    Native,
    Postman,
    OpenApi,
    Swagger,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Source {
    pub format: SourceFormat,
    /// Version of the source format, e.g. `2.1.0`, `3.0.3`, `2.0`.
    pub version: Option<String>,
}

/// A name/value pair: header, query parameter, form field, variable value.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct NameValue {
    pub name: String,
    pub value: String,
    pub description: Option<String>,
    pub disabled: bool,
    pub extensions: Extensions,
}

impl NameValue {
    pub fn new(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: value.into(),
            ..Self::default()
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApiKeyLocation {
    #[default]
    Header,
    Query,
    Cookie,
}

impl ApiKeyLocation {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Header => "header",
            Self::Query => "query",
            Self::Cookie => "cookie",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "query" => Self::Query,
            "cookie" => Self::Cookie,
            _ => Self::Header,
        }
    }
}

/// Authentication. `None` where an `Option<Auth>` is held means "inherit from the
/// parent"; [`Auth::NoAuth`] means "no authentication".
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Auth {
    NoAuth,
    Basic {
        username: String,
        password: String,
    },
    Bearer {
        token: String,
    },
    ApiKey {
        key: String,
        value: String,
        location: ApiKeyLocation,
    },
    /// Use the named entry of `Project::security_schemes` (OpenAPI `security`).
    Reference {
        scheme: String,
        #[serde(default)]
        scopes: Vec<String>,
    },
    /// Anything else. `kind` is the OpenAPI scheme type (`oauth2`, `openIdConnect`,
    /// `http`, ...) or `postman:<type>`; `params` keeps the remaining fields.
    Other {
        kind: String,
        #[serde(default)]
        params: Extensions,
    },
}

/// A named security scheme (`components.securitySchemes` / `securityDefinitions`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SecurityScheme {
    pub name: String,
    pub description: Option<String>,
    pub auth: Auth,
    pub extensions: Extensions,
}

impl Default for SecurityScheme {
    fn default() -> Self {
        Self {
            name: String::new(),
            description: None,
            auth: Auth::NoAuth,
            extensions: Extensions::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScriptKind {
    #[default]
    PreRequest,
    Test,
    Other(String),
}

/// A script, kept as one source string. Formats that store lines as arrays are
/// joined with `\n` on import and split on export.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Script {
    pub kind: ScriptKind,
    /// Media type or language name, e.g. `text/javascript`.
    pub language: Option<String>,
    pub source: String,
    pub disabled: bool,
    pub extensions: Extensions,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Variable {
    pub key: String,
    pub value: String,
    /// Declared type (`string`, `secret`, ...), if any.
    pub kind: Option<String>,
    pub description: Option<String>,
    pub disabled: bool,
    /// Allowed values (OpenAPI server variable `enum`).
    pub allowed: Vec<String>,
    pub extensions: Extensions,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Server {
    pub url: String,
    pub name: Option<String>,
    pub description: Option<String>,
    pub variables: Vec<Variable>,
    pub extensions: Extensions,
}
