use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::client::{Header, Method, Request};

#[derive(Debug, thiserror::Error)]
pub enum TemplateError {
    #[error("cannot read file: {0}")]
    Read(#[from] std::io::Error),
    #[error("{0}")]
    Parse(#[from] toml::de::Error),
}

/// A request template as stored in a project's TOML file.
///
/// `body` must come before any `[table]`, otherwise TOML makes it a key of that table.
///
/// ```text
/// name = "Create user"
/// method = "POST"
/// url = "https://api.example.com/users"
///
/// [headers]
/// Content-Type = "application/json"
///
/// body = '''
/// { "name": "ada" }
/// '''
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Template {
    pub name: String,
    pub method: Method,
    pub url: String,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    #[serde(default)]
    pub body: Option<String>,
}

impl Template {
    pub fn parse(text: &str) -> Result<Self, TemplateError> {
        Ok(toml::from_str(text)?)
    }

    pub fn from_file(path: &Path) -> Result<Self, TemplateError> {
        Self::parse(&std::fs::read_to_string(path)?)
    }

    pub fn to_request(&self) -> Request {
        Request {
            method: self.method,
            url: self.url.clone(),
            headers: self
                .headers
                .iter()
                .map(|(k, v)| Header::new(k, v))
                .collect(),
            body: self.body.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_template_with_multiline_json_body() {
        let t = Template::parse(
            r#"
name = "Create user"
method = "POST"
url = "https://api.example.com/users"
body = '''
{
  "name": "ada"
}
'''

[headers]
Content-Type = "application/json"
"#,
        )
        .unwrap();
        assert_eq!(t.method, Method::Post);
        assert_eq!(t.body.as_deref(), Some("{\n  \"name\": \"ada\"\n}\n"));
        let req = t.to_request();
        assert_eq!(
            req.headers,
            vec![Header::new("Content-Type", "application/json")]
        );
    }

    #[test]
    fn minimal_template_has_no_headers_or_body() {
        let t = Template::parse("name = \"x\"\nmethod = \"GET\"\nurl = \"http://x\"").unwrap();
        assert!(t.headers.is_empty());
        assert!(t.body.is_none());
    }

    #[test]
    fn unknown_method_and_missing_url_are_errors() {
        assert!(Template::parse("name=\"x\"\nmethod=\"TRACE\"\nurl=\"u\"").is_err());
        assert!(Template::parse("name=\"x\"\nmethod=\"GET\"").is_err());
    }
}
