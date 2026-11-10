//! Project variables: values captured from a response (`name` = a jq-style path into
//! its JSON body) and substituted into the next request as `$name`.
//!
//! The path language is the jq subset that selects a value: `.`, `.a.b`, `.items[0].id`,
//! `.["odd key"]`, `.[-1]`. A `$name` that no variable matches is left as written.

use serde_json::Value;

use crate::client::{Body, JsonValue, Part, PartValue};

/// The captured values, in the order they were first set.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Vars {
    values: Vec<(String, String)>,
}

impl Vars {
    pub fn set(&mut self, name: &str, value: String) {
        match self.values.iter_mut().find(|(n, _)| n == name) {
            Some((_, v)) => *v = value,
            None => self.values.push((name.to_owned(), value)),
        }
    }

    pub fn get(&self, name: &str) -> Option<&str> {
        self.values
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }

    pub fn remove(&mut self, name: &str) {
        self.values.retain(|(n, _)| n != name);
    }

    pub fn clear(&mut self) {
        self.values.clear();
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.values.iter().map(|(n, v)| (n.as_str(), v.as_str()))
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    /// The body with `$name` replaced in its text, JSON strings, and form and multipart
    /// values. Keys, field names and file paths are left alone.
    pub fn apply(&self, body: Body) -> Body {
        match body {
            Body::Text(t) => Body::Text(self.substitute(&t, false)),
            Body::JsonRaw(t) => Body::JsonRaw(self.substitute(&t, true)),
            Body::Json(v) => Body::Json(self.apply_json(v)),
            Body::Form(mut fields) => {
                for f in &mut fields {
                    f.value = self.substitute(&f.value, false);
                }
                Body::Form(fields)
            }
            Body::Multipart(parts) => Body::Multipart(
                parts
                    .into_iter()
                    .map(|Part { name, value }| Part {
                        name,
                        value: match value {
                            PartValue::Text(t) => PartValue::Text(self.substitute(&t, false)),
                            file => file,
                        },
                    })
                    .collect(),
            ),
            file @ Body::File(_) => file,
        }
    }

    fn apply_json(&self, value: JsonValue) -> JsonValue {
        match value {
            JsonValue::String(s) => JsonValue::String(self.substitute(&s, false)),
            JsonValue::Object(fields) => JsonValue::Object(
                fields
                    .into_iter()
                    .map(|(k, v)| (k, self.apply_json(v)))
                    .collect(),
            ),
            JsonValue::Array(items) => {
                JsonValue::Array(items.into_iter().map(|v| self.apply_json(v)).collect())
            }
            other => other,
        }
    }

    /// Replaces every `$name` of a defined variable in `text`. With `json`, `text` is a
    /// JSON document and a value put inside one of its strings is escaped.
    pub fn substitute(&self, text: &str, json: bool) -> String {
        if self.values.is_empty() || !text.contains('$') {
            return text.to_owned();
        }
        let mut out = String::with_capacity(text.len());
        let mut in_string = false;
        let mut escaped = false;
        let mut chars = text.char_indices().peekable();
        while let Some((i, c)) = chars.next() {
            if json && in_string {
                if escaped {
                    escaped = false;
                } else if c == '\\' {
                    escaped = true;
                } else if c == '"' {
                    in_string = false;
                }
            } else if json && c == '"' {
                in_string = true;
            }
            if c != '$' {
                out.push(c);
                continue;
            }
            let rest = &text[i + 1..];
            let len = name_len(rest);
            match self.get(&rest[..len]).filter(|_| len > 0) {
                Some(value) if json && in_string => out.push_str(&escape_json(value)),
                Some(value) => out.push_str(value),
                None => {
                    out.push('$');
                    continue;
                }
            }
            for _ in 0..rest[..len].chars().count() {
                chars.next();
            }
        }
        out
    }
}

/// Bytes of the identifier (`[A-Za-z_][A-Za-z0-9_]*`) at the start of `s`.
fn name_len(s: &str) -> usize {
    let mut len = 0;
    for (i, c) in s.char_indices() {
        let ok = if i == 0 {
            c.is_ascii_alphabetic() || c == '_'
        } else {
            c.is_ascii_alphanumeric() || c == '_'
        };
        if !ok {
            break;
        }
        len = i + c.len_utf8();
    }
    len
}

fn escape_json(value: &str) -> String {
    let quoted = serde_json::to_string(value).unwrap_or_default();
    quoted
        .get(1..quoted.len().saturating_sub(1))
        .unwrap_or("")
        .to_owned()
}

/// Whether `name` can be a variable name.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty() && name_len(name) == name.len()
}

#[derive(Debug, PartialEq, Eq)]
enum Step {
    Key(String),
    Index(i64),
}

fn parse_path(expr: &str) -> Result<Vec<Step>, String> {
    let expr = expr.trim();
    let mut rest = expr.strip_prefix('.').unwrap_or(expr);
    let mut steps = Vec::new();
    while !rest.is_empty() {
        if let Some(inner) = rest.strip_prefix('[') {
            let end = inner.find(']').ok_or("missing `]`")?;
            let item = inner[..end].trim();
            steps.push(
                if let Some(key) = item.strip_prefix('"').and_then(|k| k.strip_suffix('"')) {
                    Step::Key(key.to_owned())
                } else {
                    Step::Index(item.parse().map_err(|_| format!("bad index `{item}`"))?)
                },
            );
            rest = &inner[end + 1..];
        } else {
            let end = rest.find(['.', '[']).unwrap_or(rest.len());
            if end == 0 {
                return Err(format!("unexpected `{}`", &rest[..1]));
            }
            steps.push(Step::Key(rest[..end].to_owned()));
            rest = &rest[end..];
        }
        rest = rest.strip_prefix('.').unwrap_or(rest);
    }
    Ok(steps)
}

/// The value a capture row gives: a path into `body` when `expr` starts with `.`, the
/// text between the quotes when it is wrapped in `"` or `'`, otherwise `expr` as written.
/// The body is parsed only for a path.
pub fn capture(body: &str, expr: &str) -> Result<String, String> {
    let expr = expr.trim();
    if expr.starts_with('.') {
        return query(body, expr);
    }
    for quote in ['"', '\''] {
        if expr.len() >= 2
            && let Some(inner) = expr.strip_prefix(quote).and_then(|e| e.strip_suffix(quote))
        {
            return Ok(inner.to_owned());
        }
    }
    Ok(expr.to_owned())
}

/// Selects the value `expr` points to in the JSON `body`. A string comes back as it is
/// (like `jq -r`), anything else as compact JSON.
pub fn query(body: &str, expr: &str) -> Result<String, String> {
    let steps = parse_path(expr)?;
    let root: Value = serde_json::from_str(body).map_err(|_| "the body is not JSON")?;
    let mut node = &root;
    for step in &steps {
        let next = match (step, node) {
            (Step::Key(k), Value::Object(map)) => map.get(k),
            (Step::Index(i), Value::Array(items)) => {
                let i = if *i < 0 { items.len() as i64 + i } else { *i };
                usize::try_from(i).ok().and_then(|i| items.get(i))
            }
            _ => None,
        };
        node = next.ok_or_else(|| format!("nothing at `{}`", expr.trim()))?;
    }
    match node {
        Value::String(s) => Ok(s.clone()),
        Value::Null => Err(format!("`{}` is null", expr.trim())),
        other => Ok(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(pairs: &[(&str, &str)]) -> Vars {
        let mut v = Vars::default();
        for (n, x) in pairs {
            v.set(n, (*x).to_owned());
        }
        v
    }

    #[test]
    fn query_selects_values() {
        let body = r#"{"data":{"token":"abc","items":[{"id":1},{"id":2}]},"a b":true}"#;
        assert_eq!(query(body, ".data.token").unwrap(), "abc");
        assert_eq!(query(body, "data.items[1].id").unwrap(), "2");
        assert_eq!(query(body, ".data.items[-2]").unwrap(), r#"{"id":1}"#);
        assert_eq!(query(body, r#".["a b"]"#).unwrap(), "true");
        assert!(query(body, ".data.nope").is_err());
        assert!(query("nope", ".a").is_err());
    }

    #[test]
    fn capture_paths_start_with_a_dot_and_quotes_are_literals() {
        let body = r#"{"a":"x"}"#;
        assert_eq!(capture(body, ".a").unwrap(), "x");
        assert_eq!(capture(body, r#"".a""#).unwrap(), ".a");
        assert_eq!(capture(body, "'.a'").unwrap(), ".a");
        assert_eq!(capture("not json", "'v'").unwrap(), "v");
        assert_eq!(capture(body, "plain").unwrap(), "plain");
        assert!(capture(body, ".b").is_err());
    }

    #[test]
    fn unknown_variables_stay_as_written() {
        let v = vars(&[("tok", "abc")]);
        assert_eq!(v.substitute("$tok $other $ $1", false), "abc $other $ $1");
        assert_eq!(v.substitute("$tokx", false), "$tokx");
    }

    #[test]
    fn json_strings_get_escaped_values() {
        let v = vars(&[("t", r#"a"b"#), ("n", "5")]);
        assert_eq!(
            v.substitute(r#"{"k":"x-$t","n":$n}"#, true),
            r#"{"k":"x-a\"b","n":5}"#
        );
    }

    #[test]
    fn names_are_validated() {
        assert!(valid_name("auth_token"));
        assert!(!valid_name("1a"));
        assert!(!valid_name("a-b"));
        assert!(!valid_name(""));
    }
}
