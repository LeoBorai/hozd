use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::client::{GrpcRequest, Header, HttpRequest, Method, Request, SchemaSource};

#[derive(Debug, thiserror::Error)]
pub enum TemplateError {
    #[error("cannot read file: {0}")]
    Read(#[from] std::io::Error),
    #[error("{0}")]
    Parse(#[from] toml::de::Error),
    #[error("{0}")]
    Invalid(String),
}

/// A request template as stored in a project's TOML file: HTTP (the default) or gRPC.
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
///
/// A gRPC template has `kind = "grpc"`, the server `address`, the `method` as
/// `package.Service/Method`, and exactly one schema source: `reflection = true`, or `schema`
/// with the path of a `.proto` file or descriptor set, relative to the template file.
///
/// ```text
/// name = "Get user"
/// kind = "grpc"
/// address = "http://localhost:50051"
/// method = "user.v1.UserService/GetUser"
/// reflection = true
/// insecure = false
/// body = '''
/// { "id": 7 }
/// '''
///
/// [metadata]
/// authorization = "Bearer token"
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Template {
    pub name: String,
    pub kind: TemplateKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TemplateKind {
    Http(HttpTemplate),
    Grpc(GrpcTemplate),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpTemplate {
    pub method: Method,
    pub url: String,
    pub headers: BTreeMap<String, String>,
    pub body: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrpcTemplate {
    pub address: String,
    /// `package.Service/Method`.
    pub method: String,
    pub schema: SchemaSource,
    pub insecure: bool,
    pub metadata: BTreeMap<String, String>,
    pub body: Option<String>,
}

/// Every key of either kind; `Template::parse` decides which ones apply.
#[derive(Debug, Deserialize)]
struct Raw {
    name: String,
    kind: Option<String>,
    method: Option<String>,
    url: Option<String>,
    address: Option<String>,
    #[serde(default)]
    headers: BTreeMap<String, String>,
    #[serde(default)]
    metadata: BTreeMap<String, String>,
    reflection: Option<bool>,
    schema: Option<PathBuf>,
    #[serde(default)]
    insecure: bool,
    body: Option<String>,
}

impl Template {
    /// Parses a template; a gRPC `schema` path stays as written.
    pub fn parse(text: &str) -> Result<Self, TemplateError> {
        Self::parse_in(text, None)
    }

    /// Like [`parse`](Self::parse), resolving a relative gRPC `schema` path against `dir`.
    pub fn parse_in(text: &str, dir: Option<&Path>) -> Result<Self, TemplateError> {
        let raw: Raw = toml::from_str(text)?;
        let invalid = |message: String| TemplateError::Invalid(message);
        let missing = |key: &str| invalid(format!("missing field `{key}`"));

        let kind = match raw.kind.as_deref().unwrap_or("http") {
            "http" => TemplateKind::Http(HttpTemplate {
                method: raw
                    .method
                    .ok_or_else(|| missing("method"))?
                    .parse()
                    .map_err(invalid)?,
                url: raw.url.ok_or_else(|| missing("url"))?,
                headers: raw.headers,
                body: raw.body,
            }),
            "grpc" => {
                let method = raw.method.ok_or_else(|| missing("method"))?;
                if !method.contains('/') {
                    return Err(invalid(format!(
                        "`{method}` is not a gRPC method; write it as `package.Service/Method`"
                    )));
                }
                let schema = match (raw.reflection.unwrap_or(false), raw.schema) {
                    (true, None) => SchemaSource::Reflection,
                    (false, Some(path)) => SchemaSource::File(match dir {
                        Some(dir) if path.is_relative() => dir.join(path),
                        _ => path,
                    }),
                    (true, Some(_)) => {
                        return Err(invalid(
                            "set either `reflection = true` or `schema`, not both".into(),
                        ));
                    }
                    (false, None) => {
                        return Err(invalid(
                            "a gRPC template needs a schema source: `reflection = true` or \
                             `schema = \"path\"`"
                                .into(),
                        ));
                    }
                };
                TemplateKind::Grpc(GrpcTemplate {
                    address: raw.address.ok_or_else(|| missing("address"))?,
                    method,
                    schema,
                    insecure: raw.insecure,
                    metadata: raw.metadata,
                    body: raw.body,
                })
            }
            other => {
                return Err(invalid(format!(
                    "unknown template kind `{other}`; expected `http` or `grpc`"
                )));
            }
        };
        Ok(Template {
            name: raw.name,
            kind,
        })
    }

    pub fn from_file(path: &Path) -> Result<Self, TemplateError> {
        Self::parse_in(&std::fs::read_to_string(path)?, path.parent())
    }

    pub fn to_request(&self) -> Request {
        match &self.kind {
            TemplateKind::Http(t) => Request::Http(HttpRequest {
                method: t.method,
                url: t.url.clone(),
                headers: pairs(&t.headers),
                body: t.body.clone(),
            }),
            TemplateKind::Grpc(t) => Request::Grpc(GrpcRequest {
                address: t.address.clone(),
                method: t.method.clone(),
                schema: t.schema.clone(),
                insecure: t.insecure,
                metadata: pairs(&t.metadata),
                body: t.body.clone(),
            }),
        }
    }
}

fn pairs(map: &BTreeMap<String, String>) -> Vec<Header> {
    map.iter().map(|(k, v)| Header::new(k, v)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn http(t: &Template) -> &HttpTemplate {
        match &t.kind {
            TemplateKind::Http(h) => h,
            other => panic!("expected an HTTP template, got {other:?}"),
        }
    }

    fn grpc(t: &Template) -> &GrpcTemplate {
        match &t.kind {
            TemplateKind::Grpc(g) => g,
            other => panic!("expected a gRPC template, got {other:?}"),
        }
    }

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
        assert_eq!(http(&t).method, Method::Post);
        assert_eq!(
            http(&t).body.as_deref(),
            Some("{\n  \"name\": \"ada\"\n}\n")
        );
        let Request::Http(req) = t.to_request() else {
            panic!("expected an HTTP request");
        };
        assert_eq!(
            req.headers,
            vec![Header::new("Content-Type", "application/json")]
        );
    }

    #[test]
    fn minimal_template_is_http_with_no_headers_or_body() {
        let t = Template::parse("name = \"x\"\nmethod = \"GET\"\nurl = \"http://x\"").unwrap();
        assert!(http(&t).headers.is_empty());
        assert!(http(&t).body.is_none());
    }

    #[test]
    fn explicit_http_kind_is_the_same_as_none() {
        let t = Template::parse("name=\"x\"\nkind=\"http\"\nmethod=\"GET\"\nurl=\"u\"").unwrap();
        assert!(matches!(t.kind, TemplateKind::Http(_)));
    }

    #[test]
    fn unknown_method_and_missing_url_are_errors() {
        assert!(Template::parse("name=\"x\"\nmethod=\"TRACE\"\nurl=\"u\"").is_err());
        assert!(Template::parse("name=\"x\"\nmethod=\"GET\"").is_err());
    }

    const GRPC: &str = r#"
name = "Get user"
kind = "grpc"
address = "http://localhost:50051"
method = "user.v1.UserService/GetUser"
"#;

    #[test]
    fn grpc_template_with_reflection() {
        let t = Template::parse(&format!(
            "{GRPC}reflection = true\nbody = '''\n{{ \"id\": 7 }}\n'''\n\n[metadata]\nauthorization = \"Bearer t\"\n"
        ))
        .unwrap();
        let g = grpc(&t);
        assert_eq!(g.address, "http://localhost:50051");
        assert_eq!(g.method, "user.v1.UserService/GetUser");
        assert_eq!(g.schema, SchemaSource::Reflection);
        assert!(!g.insecure);
        assert_eq!(g.body.as_deref(), Some("{ \"id\": 7 }\n"));

        let Request::Grpc(req) = t.to_request() else {
            panic!("expected a gRPC request");
        };
        assert_eq!(req.metadata, vec![Header::new("authorization", "Bearer t")]);
        assert_eq!(req.schema, SchemaSource::Reflection);
    }

    #[test]
    fn schema_path_is_resolved_against_the_template_directory() {
        let text = format!("{GRPC}schema = \"../protos/user.proto\"\ninsecure = true\n");
        let t = Template::parse_in(&text, Some(Path::new("/p/api/users"))).unwrap();
        assert_eq!(
            grpc(&t).schema,
            SchemaSource::File(PathBuf::from("/p/api/users/../protos/user.proto"))
        );
        assert!(grpc(&t).insecure);

        let absolute = format!("{GRPC}schema = \"/abs/user.proto\"\n");
        let t = Template::parse_in(&absolute, Some(Path::new("/p"))).unwrap();
        assert_eq!(
            grpc(&t).schema,
            SchemaSource::File(PathBuf::from("/abs/user.proto"))
        );
    }

    #[test]
    fn schema_from_a_file_on_disk_is_relative_to_that_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("get.toml");
        std::fs::write(&path, format!("{GRPC}schema = \"user.proto\"\n")).unwrap();
        let t = Template::from_file(&path).unwrap();
        assert_eq!(
            grpc(&t).schema,
            SchemaSource::File(tmp.path().join("user.proto"))
        );
    }

    #[test]
    fn grpc_schema_source_must_be_exactly_one() {
        let neither = Template::parse(GRPC).unwrap_err().to_string();
        assert!(neither.contains("schema source"), "{neither}");

        let false_only = Template::parse(&format!("{GRPC}reflection = false\n"));
        assert!(false_only.is_err());

        let both = Template::parse(&format!("{GRPC}reflection = true\nschema = \"a.proto\"\n"))
            .unwrap_err()
            .to_string();
        assert!(both.contains("not both"), "{both}");
    }

    #[test]
    fn grpc_method_must_name_a_service() {
        let text = GRPC.replace("user.v1.UserService/GetUser", "GetUser");
        let err = Template::parse(&format!("{text}reflection = true\n"))
            .unwrap_err()
            .to_string();
        assert!(err.contains("package.Service/Method"), "{err}");
    }

    #[test]
    fn unknown_kind_is_an_error_naming_it() {
        let err = Template::parse("name=\"x\"\nkind=\"graphql\"\nurl=\"u\"")
            .unwrap_err()
            .to_string();
        assert!(err.contains("graphql"), "{err}");
    }
}
