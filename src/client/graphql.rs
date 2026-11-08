//! GraphQL over HTTP: a request is a JSON `POST`; the schema comes from introspection.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use super::common::{add_missing_header, body_snippet};
use super::{Header, HttpRequest, HttpResponse, Method};

/// What the user asked to execute.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct GraphqlRequest {
    /// The endpoint, `http://` or `https://`.
    pub url: String,
    #[serde(default)]
    pub headers: Vec<Header>,
    /// The GraphQL document.
    pub query: String,
    /// A JSON object, as typed; empty or absent means none.
    #[serde(default)]
    pub variables: Option<String>,
}

impl GraphqlRequest {
    /// The JSON body of the `POST`; fails when the variables are not a JSON object.
    pub fn payload(&self) -> Result<String, String> {
        let mut payload = Map::new();
        payload.insert("query".into(), Value::String(self.query.clone()));
        if let Some(text) = self.variables.as_deref().filter(|v| !v.trim().is_empty()) {
            match serde_json::from_str::<Value>(text) {
                Ok(value @ Value::Object(_)) => {
                    payload.insert("variables".into(), value);
                }
                Ok(_) => return Err("variables must be a JSON object".into()),
                Err(e) => return Err(format!("variables are not valid JSON: {e}")),
            }
        }
        Ok(Value::Object(payload).to_string())
    }

    /// The HTTP request that carries this one.
    pub fn to_http(&self) -> Result<HttpRequest, String> {
        let mut headers = self.headers.clone();
        add_missing_header(&mut headers, "Content-Type", "application/json");
        add_missing_header(&mut headers, "Accept", "application/json");
        Ok(HttpRequest {
            method: Method::Post,
            url: self.url.clone(),
            headers,
            body: Some(self.payload()?.into()),
        })
    }

    /// The `(query, variables)` pair as stored in history (`request_body`).
    pub fn to_stored(&self) -> String {
        serde_json::to_string(&Stored {
            query: self.query.clone(),
            variables: self.variables.clone(),
        })
        .expect("stored request serializes")
    }

    /// Inverse of [`to_stored`](Self::to_stored); a body that is not that JSON is taken as
    /// the query itself.
    pub fn from_stored(url: String, headers: Vec<Header>, stored: Option<&str>) -> Self {
        let stored = stored.unwrap_or_default();
        let (query, variables) = match serde_json::from_str::<Stored>(stored) {
            Ok(s) => (s.query, s.variables),
            Err(_) => (stored.to_owned(), None),
        };
        Self {
            url,
            headers,
            query,
            variables,
        }
    }
}

#[derive(Serialize, Deserialize)]
struct Stored {
    query: String,
    #[serde(default)]
    variables: Option<String>,
}

// ---- introspection ----------------------------------------------------------------

/// The standard introspection query.
pub const INTROSPECTION_QUERY: &str = "query IntrospectionQuery { __schema { queryType { name } \
mutationType { name } subscriptionType { name } types { ...FullType } } } \
fragment FullType on __Type { kind name fields(includeDeprecated: true) { name \
args { ...InputValue } type { ...TypeRef } } inputFields { ...InputValue } \
enumValues(includeDeprecated: true) { name } possibleTypes { name } } \
fragment InputValue on __InputValue { name type { ...TypeRef } } \
fragment TypeRef on __Type { kind name ofType { kind name ofType { kind name ofType { kind name \
ofType { kind name ofType { kind name ofType { kind name ofType { kind name } } } } } } } }";

/// The `POST` that asks `url` for its schema.
pub fn introspection_request(url: &str, headers: &[Header]) -> HttpRequest {
    GraphqlRequest {
        url: url.to_owned(),
        headers: headers.to_vec(),
        query: INTROSPECTION_QUERY.to_owned(),
        variables: None,
    }
    .to_http()
    .expect("introspection payload is valid")
}

/// Whether an operation reads (`query`) or writes (`mutation`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationKind {
    Query,
    Mutation,
}

impl OperationKind {
    /// The keyword that starts a document of this kind.
    pub fn keyword(self) -> &'static str {
        match self {
            OperationKind::Query => "query",
            OperationKind::Mutation => "mutation",
        }
    }

    /// `Query` or `Mutation`, the group an operation is listed under.
    pub fn group(self) -> &'static str {
        match self {
            OperationKind::Query => "Query",
            OperationKind::Mutation => "Mutation",
        }
    }

    fn from_group(group: &str) -> Option<Self> {
        match group {
            "Query" => Some(OperationKind::Query),
            "Mutation" => Some(OperationKind::Mutation),
            _ => None,
        }
    }
}

/// One query or mutation of an introspected schema.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphqlOperation {
    pub kind: OperationKind,
    pub name: String,
}

impl GraphqlOperation {
    /// `Query/field` or `Mutation/field`.
    pub fn full_name(&self) -> String {
        format!("{}/{}", self.kind.group(), self.name)
    }

    /// Inverse of [`full_name`](Self::full_name).
    pub fn parse(full_name: &str) -> Option<Self> {
        let (group, name) = full_name.split_once('/')?;
        Some(Self {
            kind: OperationKind::from_group(group)?,
            name: name.to_owned(),
        })
    }
}

/// A type as written in a schema: `[Int!]!`.
#[derive(Debug, Clone, PartialEq, Eq)]
enum TypeRef {
    Named(String),
    List(Box<TypeRef>),
    NonNull(Box<TypeRef>),
}

impl TypeRef {
    fn from_json(value: &Value) -> Option<Self> {
        let inner = || TypeRef::from_json(value.get("ofType")?).map(Box::new);
        match value.get("kind")?.as_str()? {
            "NON_NULL" => Some(TypeRef::NonNull(inner()?)),
            "LIST" => Some(TypeRef::List(inner()?)),
            _ => Some(TypeRef::Named(value.get("name")?.as_str()?.to_owned())),
        }
    }

    fn named(&self) -> &str {
        match self {
            TypeRef::Named(n) => n,
            TypeRef::List(t) | TypeRef::NonNull(t) => t.named(),
        }
    }

    fn is_required(&self) -> bool {
        matches!(self, TypeRef::NonNull(_))
    }
}

impl std::fmt::Display for TypeRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TypeRef::Named(n) => f.write_str(n),
            TypeRef::List(t) => write!(f, "[{t}]"),
            TypeRef::NonNull(t) => write!(f, "{t}!"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TypeKind {
    Scalar,
    Object,
    Interface,
    Union,
    Enum,
    InputObject,
}

#[derive(Debug, Clone)]
struct Input {
    name: String,
    ty: TypeRef,
}

#[derive(Debug, Clone)]
struct Field {
    name: String,
    args: Vec<Input>,
    ty: TypeRef,
}

#[derive(Debug, Clone)]
struct TypeDef {
    kind: TypeKind,
    fields: Vec<Field>,
    inputs: Vec<Input>,
    values: Vec<String>,
}

/// How deep a generated selection set goes.
const SELECTION_DEPTH: usize = 2;
/// How many fields of one type a generated selection set lists.
const SELECTION_WIDTH: usize = 12;

/// What introspection told: the root operations and the types they use.
#[derive(Debug, Clone, Default)]
pub struct GraphqlSchema {
    types: BTreeMap<String, TypeDef>,
    /// `(operation keyword, root type name)`, queries first.
    roots: Vec<(OperationKind, String)>,
    operations: Vec<GraphqlOperation>,
}

impl GraphqlSchema {
    /// Parses the body of an introspection response.
    pub fn from_response(response: &HttpResponse) -> Result<Self, String> {
        if !(200..300).contains(&response.status) {
            return Err(format!(
                "introspection failed: HTTP {}{}",
                response.status,
                body_snippet(&response.body)
            ));
        }
        let body: Value = serde_json::from_str(&response.body)
            .map_err(|e| format!("introspection reply is not JSON: {e}"))?;
        if let Some(message) = body
            .pointer("/errors/0/message")
            .and_then(Value::as_str)
            .filter(|_| body.pointer("/data/__schema").is_none())
        {
            return Err(format!("introspection failed: {message}"));
        }
        let schema = body
            .pointer("/data/__schema")
            .ok_or("introspection reply has no `data.__schema`; is introspection disabled?")?;
        Self::from_schema_json(schema)
    }

    fn from_schema_json(schema: &Value) -> Result<Self, String> {
        let mut types = BTreeMap::new();
        for t in schema["types"].as_array().into_iter().flatten() {
            let Some(name) = t["name"].as_str() else {
                continue;
            };
            let kind = match t["kind"].as_str() {
                Some("SCALAR") => TypeKind::Scalar,
                Some("OBJECT") => TypeKind::Object,
                Some("INTERFACE") => TypeKind::Interface,
                Some("UNION") => TypeKind::Union,
                Some("ENUM") => TypeKind::Enum,
                Some("INPUT_OBJECT") => TypeKind::InputObject,
                _ => continue,
            };
            let inputs = |v: &Value| -> Vec<Input> {
                v.as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|i| {
                        Some(Input {
                            name: i["name"].as_str()?.to_owned(),
                            ty: TypeRef::from_json(&i["type"])?,
                        })
                    })
                    .collect()
            };
            let fields = t["fields"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|f| {
                    Some(Field {
                        name: f["name"].as_str()?.to_owned(),
                        args: inputs(&f["args"]),
                        ty: TypeRef::from_json(&f["type"])?,
                    })
                })
                .collect();
            let values = t["enumValues"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|v| v["name"].as_str().map(str::to_owned))
                .collect();
            types.insert(
                name.to_owned(),
                TypeDef {
                    kind,
                    fields,
                    inputs: inputs(&t["inputFields"]),
                    values,
                },
            );
        }

        let mut roots = Vec::new();
        for (kind, key) in [
            (OperationKind::Query, "queryType"),
            (OperationKind::Mutation, "mutationType"),
        ] {
            if let Some(name) = schema[key]["name"].as_str() {
                roots.push((kind, name.to_owned()));
            }
        }
        if roots.is_empty() {
            return Err("the schema has no query or mutation type".into());
        }

        let mut operations = Vec::new();
        for (kind, root) in &roots {
            let Some(def) = types.get(root) else { continue };
            let mut fields: Vec<_> = def
                .fields
                .iter()
                .filter(|f| !f.name.starts_with("__"))
                .collect();
            fields.sort_by(|a, b| a.name.cmp(&b.name));
            operations.extend(fields.into_iter().map(|f| GraphqlOperation {
                kind: *kind,
                name: f.name.clone(),
            }));
        }
        Ok(Self {
            types,
            roots,
            operations,
        })
    }

    /// The queries and mutations (subscriptions need a WebSocket transport and are left
    /// out), queries first.
    pub fn operations(&self) -> &[GraphqlOperation] {
        &self.operations
    }

    pub fn type_count(&self) -> usize {
        self.types.keys().filter(|n| !n.starts_with("__")).count()
    }

    /// `(query, variables)` to start from for `operation`: the document with
    /// every argument as a variable and a shallow selection set, and a JSON object with a
    /// placeholder for each variable (`None` when there are none).
    pub fn skeleton(&self, operation: &GraphqlOperation) -> Option<(String, Option<String>)> {
        let name = operation.name.as_str();
        let (kind, root) = self.roots.iter().find(|(k, _)| *k == operation.kind)?;
        let keyword = kind.keyword();
        let field = self
            .types
            .get(root)?
            .fields
            .iter()
            .find(|f| f.name == name)?;

        let defs: Vec<String> = field
            .args
            .iter()
            .map(|a| format!("${}: {}", a.name, a.ty))
            .collect();
        let call: Vec<String> = field
            .args
            .iter()
            .map(|a| format!("{0}: ${0}", a.name))
            .collect();
        let mut query = format!("{keyword} {}", capitalize(name));
        if !defs.is_empty() {
            query.push_str(&format!("({})", defs.join(", ")));
        }
        query.push_str(" {\n  ");
        query.push_str(name);
        if !call.is_empty() {
            query.push_str(&format!("({})", call.join(", ")));
        }
        let selection = self.selection(field.ty.named(), SELECTION_DEPTH, 1);
        if !selection.is_empty() {
            query.push_str(" {\n");
            query.push_str(&selection);
            query.push_str("  }");
        }
        query.push_str("\n}\n");

        let variables = (!field.args.is_empty()).then(|| {
            let mut object = Map::new();
            for a in &field.args {
                object.insert(a.name.clone(), self.placeholder(&a.ty, 3));
            }
            serde_json::to_string_pretty(&Value::Object(object)).expect("json serializes")
        });
        Some((query, variables))
    }

    /// The selection set (without braces) of type `name`, indented `indent` levels.
    fn selection(&self, name: &str, depth: usize, indent: usize) -> String {
        let Some(def) = self.types.get(name) else {
            return String::new();
        };
        let pad = "  ".repeat(indent + 1);
        match def.kind {
            TypeKind::Object | TypeKind::Interface => {}
            TypeKind::Union => return format!("{pad}__typename\n"),
            _ => return String::new(),
        }
        let mut out = String::new();
        let mut count = 0;
        // Leaves first, then objects while there is depth left.
        for want_leaf in [true, false] {
            for f in &def.fields {
                if count == SELECTION_WIDTH {
                    break;
                }
                // A field that needs arguments cannot be selected without values.
                if f.name.starts_with("__") || f.args.iter().any(|a| a.ty.is_required()) {
                    continue;
                }
                let target = self.types.get(f.ty.named());
                let leaf =
                    target.is_none_or(|t| matches!(t.kind, TypeKind::Scalar | TypeKind::Enum));
                if leaf != want_leaf {
                    continue;
                }
                if leaf {
                    out.push_str(&format!("{pad}{}\n", f.name));
                    count += 1;
                } else if depth > 1 {
                    let inner = self.selection(f.ty.named(), depth - 1, indent + 1);
                    if !inner.is_empty() {
                        out.push_str(&format!("{pad}{} {{\n{inner}{pad}}}\n", f.name));
                        count += 1;
                    }
                }
            }
        }
        if out.is_empty() {
            out.push_str(&format!("{pad}__typename\n"));
        }
        out
    }

    /// A JSON value to start from for a variable of type `ty`.
    fn placeholder(&self, ty: &TypeRef, depth: usize) -> Value {
        match ty {
            TypeRef::NonNull(t) => self.placeholder(t, depth),
            TypeRef::List(_) => json!([]),
            TypeRef::Named(name) => match name.as_str() {
                "Int" => json!(0),
                "Float" => json!(0.0),
                "Boolean" => json!(false),
                "String" | "ID" => json!(""),
                _ => match self.types.get(name) {
                    Some(def) if def.kind == TypeKind::Enum => def
                        .values
                        .first()
                        .map_or(Value::Null, |v| Value::String(v.clone())),
                    Some(def) if def.kind == TypeKind::InputObject && depth > 0 => {
                        let mut object = Map::new();
                        for i in &def.inputs {
                            object.insert(i.name.clone(), self.placeholder(&i.ty, depth - 1));
                        }
                        Value::Object(object)
                    }
                    _ => Value::Null,
                },
            },
        }
    }
}

fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    chars
        .next()
        .map(|c| c.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    pub(crate) fn sample_response() -> HttpResponse {
        let named = |kind: &str, name: &str| json!({"kind": kind, "name": name, "ofType": null});
        let non_null = |inner: Value| json!({"kind": "NON_NULL", "name": null, "ofType": inner});
        let list = |inner: Value| json!({"kind": "LIST", "name": null, "ofType": inner});
        let body = json!({"data": {"__schema": {
            "queryType": {"name": "Query"},
            "mutationType": {"name": "Mutation"},
            "subscriptionType": null,
            "types": [
                {"kind": "OBJECT", "name": "Query", "fields": [
                    {"name": "user", "args": [
                        {"name": "id", "type": non_null(named("SCALAR", "ID"))}
                    ], "type": named("OBJECT", "User")},
                    {"name": "users", "args": [], "type": list(non_null(named("OBJECT", "User")))},
                    {"name": "__typename", "args": [], "type": named("SCALAR", "String")}
                ]},
                {"kind": "OBJECT", "name": "Mutation", "fields": [
                    {"name": "addUser", "args": [
                        {"name": "input", "type": non_null(named("INPUT_OBJECT", "NewUser"))}
                    ], "type": named("OBJECT", "User")}
                ]},
                {"kind": "OBJECT", "name": "User", "fields": [
                    {"name": "id", "args": [], "type": non_null(named("SCALAR", "ID"))},
                    {"name": "name", "args": [], "type": named("SCALAR", "String")},
                    {"name": "role", "args": [], "type": named("ENUM", "Role")},
                    {"name": "friends", "args": [], "type": list(named("OBJECT", "User"))},
                    {"name": "secret", "args": [
                        {"name": "key", "type": non_null(named("SCALAR", "String"))}
                    ], "type": named("SCALAR", "String")}
                ]},
                {"kind": "INPUT_OBJECT", "name": "NewUser", "inputFields": [
                    {"name": "name", "type": non_null(named("SCALAR", "String"))},
                    {"name": "role", "type": named("ENUM", "Role")}
                ]},
                {"kind": "ENUM", "name": "Role", "enumValues": [{"name": "ADMIN"}, {"name": "USER"}]},
                {"kind": "SCALAR", "name": "ID"},
                {"kind": "SCALAR", "name": "String"}
            ]
        }}});
        HttpResponse {
            status: 200,
            headers: vec![],
            body: body.to_string(),
            elapsed: Duration::ZERO,
        }
    }

    #[test]
    fn payload_carries_query_and_variables() {
        let req = GraphqlRequest {
            url: "http://x/graphql".into(),
            query: "{ a }".into(),
            variables: Some("{\"a\": 1}".into()),
            ..Default::default()
        };
        let http = req.to_http().unwrap();
        assert_eq!(http.method, Method::Post);
        let body: Value = serde_json::from_str(
            http.body
                .as_ref()
                .and_then(crate::client::Body::as_text)
                .unwrap(),
        )
        .unwrap();
        assert_eq!(body, json!({"query": "{ a }", "variables": {"a": 1}}));
        assert!(
            http.headers
                .contains(&Header::new("Content-Type", "application/json"))
        );
    }

    #[test]
    fn user_headers_win_over_defaults() {
        let req = GraphqlRequest {
            headers: vec![Header::new("content-type", "application/graphql+json")],
            query: "{ a }".into(),
            ..Default::default()
        };
        let http = req.to_http().unwrap();
        assert_eq!(
            http.headers
                .iter()
                .filter(|h| h.name.eq_ignore_ascii_case("content-type"))
                .count(),
            1
        );
    }

    #[test]
    fn bad_variables_are_refused() {
        let mut req = GraphqlRequest {
            query: "{ a }".into(),
            variables: Some("{nope".into()),
            ..Default::default()
        };
        assert!(req.payload().unwrap_err().contains("not valid JSON"));
        req.variables = Some("[1]".into());
        assert!(req.payload().unwrap_err().contains("JSON object"));
        req.variables = Some("  ".into());
        assert!(!req.payload().unwrap().contains("variables"));
    }

    #[test]
    fn stored_form_round_trips() {
        let req = GraphqlRequest {
            url: "http://x".into(),
            headers: vec![Header::new("A", "1")],
            query: "{ a }".into(),
            variables: Some("{}".into()),
        };
        let back = GraphqlRequest::from_stored(
            req.url.clone(),
            req.headers.clone(),
            Some(&req.to_stored()),
        );
        assert_eq!(back, req);
        let raw = GraphqlRequest::from_stored("u".into(), vec![], Some("{ plain }"));
        assert_eq!(raw.query, "{ plain }");
    }

    #[test]
    fn introspection_lists_queries_and_mutations() {
        let schema = GraphqlSchema::from_response(&sample_response()).unwrap();
        let names: Vec<_> = schema
            .operations()
            .iter()
            .map(GraphqlOperation::full_name)
            .collect();
        assert_eq!(names, ["Query/user", "Query/users", "Mutation/addUser"]);
        assert_eq!(schema.type_count(), 7);
    }

    #[test]
    fn skeleton_for_a_query_with_arguments() {
        let schema = GraphqlSchema::from_response(&sample_response()).unwrap();
        let (query, variables) = schema
            .skeleton(&GraphqlOperation::parse("Query/user").unwrap())
            .unwrap();
        assert_eq!(
            query,
            "query User($id: ID!) {\n  user(id: $id) {\n    id\n    name\n    role\n    friends {\n      id\n      name\n      role\n    }\n  }\n}\n"
        );
        assert_eq!(variables.as_deref(), Some("{\n  \"id\": \"\"\n}"));
    }

    #[test]
    fn skeleton_fills_input_objects_and_enums() {
        let schema = GraphqlSchema::from_response(&sample_response()).unwrap();
        let (query, variables) = schema
            .skeleton(&GraphqlOperation::parse("Mutation/addUser").unwrap())
            .unwrap();
        assert!(
            query.starts_with("mutation AddUser($input: NewUser!) {\n  addUser(input: $input) {")
        );
        let v: Value = serde_json::from_str(&variables.unwrap()).unwrap();
        assert_eq!(v, json!({"input": {"name": "", "role": "ADMIN"}}));
        let (query, variables) = schema
            .skeleton(&GraphqlOperation::parse("Query/users").unwrap())
            .unwrap();
        assert!(query.starts_with("query Users {\n  users {"));
        assert!(variables.is_none());
    }

    #[test]
    fn errors_are_reported() {
        let mut res = sample_response();
        res.body = r#"{"errors":[{"message":"introspection is disabled"}]}"#.into();
        let err = GraphqlSchema::from_response(&res).unwrap_err();
        assert!(err.contains("introspection is disabled"), "{err}");
        res.status = 401;
        let err = GraphqlSchema::from_response(&res).unwrap_err();
        assert!(err.contains("HTTP 401"), "{err}");
    }
}
