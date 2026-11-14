//! Helpers the converters share.

use serde_json::{Map, Value};

use super::common::{ConvertError, Extensions};

impl ConvertError {
    pub(crate) fn invalid(msg: impl Into<String>) -> Self {
        Self::Invalid(msg.into())
    }
}

/// Removes `key` and returns it when it is a string. A value of another type stays in
/// the map, so it ends up in `extensions` instead of being dropped.
pub(crate) fn take_string(m: &mut Map<String, Value>, key: &str) -> Option<String> {
    match m.remove(key) {
        Some(Value::String(s)) => Some(s),
        Some(other) => {
            m.insert(key.to_owned(), other);
            None
        }
        None => None,
    }
}

pub(crate) fn take_bool(m: &mut Map<String, Value>, key: &str) -> Option<bool> {
    match m.remove(key) {
        Some(Value::Bool(b)) => Some(b),
        Some(other) => {
            m.insert(key.to_owned(), other);
            None
        }
        None => None,
    }
}

pub(crate) fn take_array(m: &mut Map<String, Value>, key: &str) -> Vec<Value> {
    match m.remove(key) {
        Some(Value::Array(a)) => a,
        Some(other) => {
            m.insert(key.to_owned(), other);
            Vec::new()
        }
        None => Vec::new(),
    }
}

pub(crate) fn take_object(m: &mut Map<String, Value>, key: &str) -> Option<Map<String, Value>> {
    match m.remove(key) {
        Some(Value::Object(o)) => Some(o),
        Some(other) => {
            m.insert(key.to_owned(), other);
            None
        }
        None => None,
    }
}

/// A description given as text or as `{ "content": ... }`.
pub(crate) fn take_description(m: &mut Map<String, Value>) -> Option<String> {
    match m.remove("description") {
        Some(Value::String(s)) => Some(s),
        Some(Value::Object(o)) => o.get("content").and_then(Value::as_str).map(str::to_owned),
        _ => None,
    }
}

/// Any scalar as text; `null` as empty.
pub(crate) fn value_to_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// Moves `rest` into `ext`, each key prefixed with `prefix`.
pub(crate) fn stash(ext: &mut Extensions, prefix: &str, rest: Map<String, Value>) {
    for (k, v) in rest {
        ext.insert(format!("{prefix}{k}"), v);
    }
}

/// Writes extensions into `target`.
///
/// With an empty `prefix`, keys that start with one of `skip` (they belong to a nested
/// object) are left out. With a prefix, only keys that carry it are written, without it.
/// Keys of a foreign format are written only when `foreign` is set; `x-` keys always are.
pub(crate) fn emit_ext(
    target: &mut Map<String, Value>,
    ext: &Extensions,
    prefix: &str,
    skip: &[&str],
    foreign: bool,
) {
    for (k, v) in ext {
        let key = if prefix.is_empty() {
            if skip.iter().any(|p| k.starts_with(p)) {
                continue;
            }
            k.as_str()
        } else {
            match k.strip_prefix(prefix) {
                Some(s) => s,
                None => continue,
            }
        };
        if foreign || key.starts_with("x-") {
            target.insert(key.to_owned(), v.clone());
        }
    }
}

/// `:id` -> `{id}` (Postman path variable to the model's form).
pub(crate) fn colon_to_brace(seg: &str) -> String {
    match seg.strip_prefix(':') {
        Some(name) if !name.is_empty() && name.chars().all(is_ident) => format!("{{{name}}}"),
        _ => seg.to_owned(),
    }
}

/// `{id}` -> `:id`. Segments that mix text and variables stay as they are.
pub(crate) fn brace_to_colon(seg: &str) -> String {
    match seg.strip_prefix('{').and_then(|s| s.strip_suffix('}')) {
        Some(name) if !name.is_empty() && name.chars().all(is_ident) => format!(":{name}"),
        _ => seg.to_owned(),
    }
}

fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | '-' | '.')
}
