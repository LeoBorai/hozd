//! OpenAPI 3.0.x / 3.1.x and Swagger 2.0 import, OpenAPI 3.x export.
//!
//! Swagger 2.0 is first rewritten as an OpenAPI 3.0 document, then imported by the
//! same code, so both end up as the same model.
//!
//! Fields of hozd with no OpenAPI home (scripts, settings, directory variables) are
//! written as `x-hozd-*` vendor extensions and read back on import.
//!
//! Limits: `$ref` parameters, request bodies and responses are inlined on import;
//! a `security` requirement with more than one scheme or alternative is kept as-is in
//! `extensions["security"]`; nested directories are exported as one tag each, named by
//! the joined path (`a/b`) and tagged `x-hozd-path`.

use std::collections::HashMap;

use serde::de::DeserializeOwned;
use serde_json::{Map, Value, json};

use super::common::*;
use super::postman::language_for;
use super::project::{ApiRequest, Directory, Project, Request};
use super::rest::*;
use super::util::*;

const METHODS: [&str; 8] = [
    "get", "put", "post", "delete", "options", "head", "patch", "trace",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OpenApiVersion {
    #[default]
    V3_0,
    V3_1,
}

impl OpenApiVersion {
    fn default_string(self) -> &'static str {
        match self {
            Self::V3_0 => "3.0.3",
            Self::V3_1 => "3.1.0",
        }
    }
    fn minor_prefix(self) -> &'static str {
        match self {
            Self::V3_0 => "3.0",
            Self::V3_1 => "3.1",
        }
    }
}

// ================================================================ import

/// Imports an OpenAPI 3.0.x/3.1.x or Swagger 2.0 document.
pub fn import_openapi(doc: &Value) -> Result<Project, ConvertError> {
    if doc.get("swagger").is_some() {
        let version = doc.get("swagger").map(value_to_string);
        if version.as_deref() != Some("2.0") {
            return Err(ConvertError::Unsupported(format!(
                "Swagger version `{}`; only 2.0 is supported",
                version.unwrap_or_default()
            )));
        }
        let converted = swagger_to_oas3(doc)?;
        let mut p = import_oas3(&converted)?;
        p.source = Source {
            format: SourceFormat::Swagger,
            version: Some("2.0".to_owned()),
        };
        return Ok(p);
    }
    match doc.get("openapi").and_then(Value::as_str) {
        Some(v) if v.starts_with("3.0") || v.starts_with("3.1") => import_oas3(doc),
        Some(v) => Err(ConvertError::Unsupported(format!("OpenAPI version `{v}`"))),
        None => Err(ConvertError::invalid(
            "missing `openapi` or `swagger` field",
        )),
    }
}

fn resolve<'a>(doc: &'a Value, v: &'a Value) -> &'a Value {
    let mut cur = v;
    for _ in 0..8 {
        match cur
            .get("$ref")
            .and_then(Value::as_str)
            .and_then(|r| r.strip_prefix('#'))
        {
            Some(ptr) => match doc.pointer(ptr) {
                Some(t) => cur = t,
                None => return cur,
            },
            None => return cur,
        }
    }
    cur
}

/// Removes `key` from `m` and parses it, leaving it in place when it does not parse.
fn take_hozd<T: DeserializeOwned>(m: &mut Map<String, Value>, key: &str) -> Option<T> {
    let parsed = serde_json::from_value::<T>(m.get(key)?.clone()).ok()?;
    m.remove(key);
    Some(parsed)
}

fn import_oas3(doc: &Value) -> Result<Project, ConvertError> {
    let mut root = doc
        .as_object()
        .ok_or_else(|| ConvertError::invalid("an OpenAPI document must be a JSON object"))?
        .clone();

    let mut p = Project {
        source: Source {
            format: SourceFormat::OpenApi,
            version: take_string(&mut root, "openapi"),
        },
        ..Project::default()
    };
    let mut info = take_object(&mut root, "info").unwrap_or_default();
    p.name = take_string(&mut info, "title").unwrap_or_default();
    p.description = take_string(&mut info, "description");
    p.version = take_string(&mut info, "version");
    stash(&mut p.extensions, "info.", info);
    p.id = derive_id(&["openapi", &p.name]);

    p.servers = take_array(&mut root, "servers")
        .into_iter()
        .filter_map(import_server)
        .collect();

    if let Some(mut components) = take_object(&mut root, "components") {
        if let Some(Value::Object(schemes)) = components.remove("securitySchemes") {
            for (name, v) in schemes {
                if let Value::Object(m) = resolve(doc, &v).clone() {
                    p.security_schemes.push(import_scheme(name, m));
                }
            }
        }
        if !components.is_empty() {
            p.extensions
                .insert("components".to_owned(), Value::Object(components));
        }
    }

    if let Some(sec) = root.remove("security") {
        match parse_security(&sec) {
            Some(a) => p.auth = Some(a),
            None => {
                p.extensions.insert("security".to_owned(), sec);
            }
        }
    }
    p.variables = take_hozd(&mut root, "x-hozd-variables").unwrap_or_default();
    p.scripts = take_hozd(&mut root, "x-hozd-scripts").unwrap_or_default();

    // tags -> directories
    let mut tag_paths: HashMap<String, Vec<String>> = HashMap::new();
    for t in take_array(&mut root, "tags") {
        let Value::Object(mut m) = t else { continue };
        let Some(name) = take_string(&mut m, "name") else {
            continue;
        };
        let path: Vec<String> =
            take_hozd(&mut m, "x-hozd-path").unwrap_or_else(|| vec![name.clone()]);
        if path.is_empty() {
            continue;
        }
        let dir = ensure_dir(&mut p.directories, &path, "");
        dir.description = take_string(&mut m, "description");
        dir.variables = take_hozd(&mut m, "x-hozd-variables").unwrap_or_default();
        dir.scripts = take_hozd(&mut m, "x-hozd-scripts").unwrap_or_default();
        dir.auth = take_hozd(&mut m, "x-hozd-auth");
        dir.extensions = m;
        tag_paths.insert(name, path);
    }

    for (path, item) in take_object(&mut root, "paths").unwrap_or_default() {
        let Value::Object(mut item) = item else {
            continue;
        };
        let common: Vec<Value> = take_array(&mut item, "parameters");
        let mut path_ext = Extensions::new();
        let mut ops = Vec::new();
        for (k, v) in item {
            if METHODS.contains(&k.as_str()) {
                ops.push((k, v));
            } else {
                path_ext.insert(format!("pathItem.{k}"), v);
            }
        }
        for (method, op) in ops {
            let Value::Object(op) = op else { continue };
            let (req, tag) = import_operation(doc, &method, &path, op, &common, &path_ext);
            match tag.map(|t| tag_paths.get(&t).cloned().unwrap_or_else(|| vec![t])) {
                Some(dir_path) => ensure_dir(&mut p.directories, &dir_path, "")
                    .requests
                    .push(req),
                None => p.requests.push(req),
            }
        }
    }

    p.extensions.extend(root);
    Ok(p)
}

fn ensure_dir<'a>(
    dirs: &'a mut Vec<Directory>,
    path: &[String],
    prefix: &str,
) -> &'a mut Directory {
    let full = if prefix.is_empty() {
        path[0].clone()
    } else {
        format!("{prefix}/{}", path[0])
    };
    let idx = match dirs.iter().position(|d| d.name == path[0]) {
        Some(i) => i,
        None => {
            dirs.push(Directory {
                id: derive_id(&["openapi-tag", &full]),
                name: path[0].clone(),
                ..Directory::default()
            });
            dirs.len() - 1
        }
    };
    if path.len() == 1 {
        &mut dirs[idx]
    } else {
        ensure_dir(&mut dirs[idx].directories, &path[1..], &full)
    }
}

fn import_server(v: Value) -> Option<Server> {
    let Value::Object(mut m) = v else { return None };
    let mut s = Server {
        url: take_string(&mut m, "url")?,
        ..Server::default()
    };
    s.description = take_string(&mut m, "description");
    if let Some(vars) = take_object(&mut m, "variables") {
        for (key, v) in vars {
            let Value::Object(mut vm) = v else { continue };
            let mut var = Variable {
                key,
                ..Variable::default()
            };
            var.value = vm
                .remove("default")
                .map(|d| value_to_string(&d))
                .unwrap_or_default();
            var.description = take_string(&mut vm, "description");
            var.allowed = take_array(&mut vm, "enum")
                .iter()
                .map(value_to_string)
                .collect();
            var.extensions = vm;
            s.variables.push(var);
        }
    }
    s.extensions = m;
    Some(s)
}

fn import_scheme(name: String, mut m: Map<String, Value>) -> SecurityScheme {
    let ty = take_string(&mut m, "type").unwrap_or_default();
    let description = take_string(&mut m, "description");
    let mut ext = Extensions::new();
    let auth = match ty.as_str() {
        "http" => match m
            .get("scheme")
            .and_then(Value::as_str)
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("basic") => {
                m.remove("scheme");
                ext = std::mem::take(&mut m);
                Auth::Basic {
                    username: String::new(),
                    password: String::new(),
                }
            }
            Some("bearer") => {
                m.remove("scheme");
                ext = std::mem::take(&mut m);
                Auth::Bearer {
                    token: String::new(),
                }
            }
            _ => Auth::Other {
                kind: ty,
                params: std::mem::take(&mut m),
            },
        },
        "apiKey" => {
            let key = take_string(&mut m, "name").unwrap_or_default();
            let location = ApiKeyLocation::parse(&take_string(&mut m, "in").unwrap_or_default());
            ext = std::mem::take(&mut m);
            Auth::ApiKey {
                key,
                value: String::new(),
                location,
            }
        }
        _ => Auth::Other {
            kind: ty,
            params: std::mem::take(&mut m),
        },
    };
    SecurityScheme {
        name,
        description,
        auth,
        extensions: ext,
    }
}

/// A `security` value that names exactly one scheme, or the empty list.
fn parse_security(v: &Value) -> Option<Auth> {
    let arr = v.as_array()?;
    match arr.as_slice() {
        [] => Some(Auth::NoAuth),
        [Value::Object(req)] if req.len() == 1 => {
            let (scheme, scopes) = req.iter().next()?;
            let scopes = scopes.as_array()?.iter().map(value_to_string).collect();
            Some(Auth::Reference {
                scheme: scheme.clone(),
                scopes,
            })
        }
        _ => None,
    }
}

fn import_operation(
    doc: &Value,
    method: &str,
    path: &str,
    mut op: Map<String, Value>,
    common: &[Value],
    path_ext: &Extensions,
) -> (Request, Option<String>) {
    let mut rest = RestRequest::new(Method::parse(method), path);

    let tags: Vec<String> = take_array(&mut op, "tags")
        .iter()
        .map(value_to_string)
        .collect();
    rest.meta.operation_id = take_string(&mut op, "operationId");
    rest.meta.summary = take_string(&mut op, "summary");
    rest.meta.deprecated = take_bool(&mut op, "deprecated").unwrap_or(false);
    rest.meta.tags = tags.clone();
    let description = take_string(&mut op, "description");

    let own: Vec<RestParam> = take_array(&mut op, "parameters")
        .iter()
        .filter_map(|v| import_param(doc, v))
        .collect();
    for c in common.iter().filter_map(|v| import_param(doc, v)) {
        let overridden = own
            .iter()
            .any(|o| o.pair.name == c.pair.name && o.location == c.location);
        if !overridden {
            rest.params.push(c);
        }
    }
    rest.params.extend(own);

    if let Some(b) = op.remove("requestBody") {
        rest.body = import_body(doc, &b);
    }
    let mut ext = Extensions::new();
    if let Some(r) = op.remove("responses") {
        let (responses, x) = import_responses(doc, r, method, path);
        rest.responses = responses;
        ext.extend(x);
    }
    if let Some(sec) = op.remove("security") {
        match parse_security(&sec) {
            Some(a) => rest.auth = Some(a),
            None => {
                ext.insert("security".to_owned(), sec);
            }
        }
    }
    rest.scripts = take_hozd(&mut op, "x-hozd-scripts").unwrap_or_default();
    rest.settings = take_hozd(&mut op, "x-hozd-settings").unwrap_or_default();

    ext.extend(op);
    ext.extend(path_ext.clone());

    let name = rest
        .meta
        .summary
        .clone()
        .or_else(|| rest.meta.operation_id.clone())
        .unwrap_or_else(|| default_name(&rest));
    let req = Request {
        id: derive_id(&["openapi", &method.to_ascii_uppercase(), path]),
        name,
        description,
        api: ApiRequest::Rest(rest),
        extensions: ext,
    };
    (req, tags.into_iter().next())
}

fn default_name(rest: &RestRequest) -> String {
    rest.meta
        .operation_id
        .clone()
        .unwrap_or_else(|| format!("{} {}", rest.method.as_str(), rest.path))
}

fn import_param(doc: &Value, v: &Value) -> Option<RestParam> {
    let Value::Object(mut m) = resolve(doc, v).clone() else {
        return None;
    };
    let location = ParamLocation::parse(&take_string(&mut m, "in")?)?;
    let mut p = RestParam {
        location,
        ..RestParam::default()
    };
    p.pair.name = take_string(&mut m, "name")?;
    p.required = take_bool(&mut m, "required").unwrap_or(false) || location == ParamLocation::Path;
    p.pair.description = take_string(&mut m, "description");
    p.schema = m.remove("schema");
    match m.remove("example") {
        Some(Value::String(s)) => p.pair.value = s,
        Some(other) => {
            p.pair.value = other.to_string();
            m.insert("example".to_owned(), other);
        }
        None => {}
    }
    p.pair.extensions = m;
    Some(p)
}

fn is_json(ct: &str) -> bool {
    ct.to_ascii_lowercase().contains("json")
}

/// JSON media types keep the example as JSON text (a string example is quoted);
/// other media types keep strings as they are.
fn example_to_text(ct: &str, v: &Value) -> String {
    if is_json(ct) {
        serde_json::to_string_pretty(v).unwrap_or_default()
    } else {
        value_to_string(v)
    }
}

fn text_to_example(ct: &str, text: &str) -> Value {
    if is_json(ct) {
        serde_json::from_str(text).unwrap_or_else(|_| Value::String(text.to_owned()))
    } else {
        Value::String(text.to_owned())
    }
}

fn is_binary_schema(s: &Option<Value>) -> bool {
    s.as_ref().is_some_and(|s| {
        s.get("type").and_then(Value::as_str) == Some("string")
            && s.get("format").and_then(Value::as_str) == Some("binary")
    })
}

fn is_file_property(p: &Value) -> bool {
    let bin = |v: &Value| v.get("format").and_then(Value::as_str) == Some("binary");
    bin(p) || p.get("items").is_some_and(bin)
}

fn form_fields(
    schema: &Option<Value>,
    example: Option<Value>,
    ext: &mut Extensions,
) -> Vec<FormField> {
    let props = schema
        .as_ref()
        .and_then(|s| s.get("properties"))
        .and_then(Value::as_object);
    let kind_of = |name: &str| {
        if props
            .and_then(|p| p.get(name))
            .is_some_and(is_file_property)
        {
            FieldKind::File
        } else {
            FieldKind::Text
        }
    };
    match example {
        Some(Value::Object(o)) => o
            .iter()
            .map(|(k, v)| FormField {
                pair: NameValue::new(k, value_to_string(v)),
                kind: kind_of(k),
                content_type: None,
            })
            .collect(),
        other => {
            if let Some(o) = other {
                ext.insert("example".to_owned(), o);
            }
            props
                .map(|p| {
                    p.keys()
                        .map(|k| FormField {
                            pair: NameValue::new(k, ""),
                            kind: kind_of(k),
                            content_type: None,
                        })
                        .collect()
                })
                .unwrap_or_default()
        }
    }
}

fn import_body(doc: &Value, v: &Value) -> Option<RestBody> {
    let Value::Object(mut m) = resolve(doc, v).clone() else {
        return None;
    };
    let mut body = RestBody::default();
    body.required = take_bool(&mut m, "required").unwrap_or(false);
    body.description = take_string(&mut m, "description");
    let content = take_object(&mut m, "content").unwrap_or_default();
    body.extensions = m;

    let mut it = content.into_iter();
    if let Some((ct, media)) = it.next() {
        let mut media = media.as_object().cloned().unwrap_or_default();
        body.schema = media.remove("schema");
        let example = media.remove("example");
        let lower = ct.to_ascii_lowercase();
        body.content = if lower.starts_with("application/x-www-form-urlencoded") {
            BodyContent::FormUrlEncoded {
                fields: form_fields(&body.schema, example, &mut media),
            }
        } else if lower.starts_with("multipart/") {
            BodyContent::Multipart {
                fields: form_fields(&body.schema, example, &mut media),
            }
        } else if is_binary_schema(&body.schema) {
            if let Some(e) = example {
                media.insert("example".to_owned(), e);
            }
            BodyContent::Binary { file: None }
        } else {
            let text = example
                .as_ref()
                .map(|e| example_to_text(&ct, e))
                .unwrap_or_default();
            let language = Some(language_for(&ct))
                .filter(|l| !l.is_empty())
                .map(str::to_owned);
            BodyContent::Raw { text, language }
        };
        body.content_type = Some(ct);
        body.media_extensions = media;
    }
    body.other_media = it.collect();
    Some(body)
}

fn import_responses(
    doc: &Value,
    v: Value,
    method: &str,
    path: &str,
) -> (Vec<RestResponse>, Extensions) {
    let mut out = Vec::new();
    let mut ext = Extensions::new();
    let Value::Object(map) = v else {
        return (out, ext);
    };
    for (code, r) in map {
        if code.starts_with("x-") {
            ext.insert(format!("responses.{code}"), r);
            continue;
        }
        let Value::Object(mut m) = resolve(doc, &r).clone() else {
            continue;
        };
        let mut resp = RestResponse {
            id: derive_id(&[
                "openapi-response",
                &method.to_ascii_uppercase(),
                path,
                &code,
            ]),
            code: code.clone(),
            ..RestResponse::default()
        };
        resp.status_text = take_string(&mut m, "description");
        resp.name = resp
            .status_text
            .clone()
            .filter(|s| !s.is_empty())
            .unwrap_or(code);
        if let Some(headers) = take_object(&mut m, "headers") {
            for (name, h) in headers {
                let Value::Object(mut hm) = resolve(doc, &h).clone() else {
                    continue;
                };
                let mut p = RestParam {
                    location: ParamLocation::Header,
                    ..RestParam::default()
                };
                p.pair.name = name;
                p.required = take_bool(&mut hm, "required").unwrap_or(false);
                p.pair.description = take_string(&mut hm, "description");
                p.schema = hm.remove("schema");
                match hm.remove("example") {
                    Some(Value::String(s)) => p.pair.value = s,
                    Some(other) => {
                        p.pair.value = other.to_string();
                        hm.insert("example".to_owned(), other);
                    }
                    None => {}
                }
                p.pair.extensions = hm;
                resp.headers.push(p);
            }
        }
        let content = take_object(&mut m, "content").unwrap_or_default();
        let mut it = content.into_iter();
        if let Some((ct, media)) = it.next() {
            let mut media = media.as_object().cloned().unwrap_or_default();
            resp.schema = media.remove("schema");
            resp.body = media.remove("example").map(|e| example_to_text(&ct, &e));
            resp.language = Some(language_for(&ct))
                .filter(|l| !l.is_empty())
                .map(str::to_owned);
            resp.content_type = Some(ct);
            resp.media_extensions = media;
        }
        resp.other_media = it.collect();
        resp.extensions = m;
        out.push(resp);
    }
    (out, ext)
}

// ================================================================ Swagger 2.0 -> OpenAPI 3.0

fn rewrite_refs(v: &Value) -> Value {
    match v {
        Value::Object(m) => Value::Object(
            m.iter()
                .map(|(k, v)| {
                    let nv = match (k.as_str(), v) {
                        ("$ref", Value::String(s)) => {
                            Value::String(s.replacen("#/definitions/", "#/components/schemas/", 1))
                        }
                        _ => rewrite_refs(v),
                    };
                    (k.clone(), nv)
                })
                .collect(),
        ),
        Value::Array(a) => Value::Array(a.iter().map(rewrite_refs).collect()),
        other => other.clone(),
    }
}

fn str_list(v: Option<&Value>) -> Vec<String> {
    v.and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|s| s.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

/// The schema of a Swagger non-body parameter or header: its type keywords.
fn simple_schema(m: &Map<String, Value>) -> Value {
    const NOT_SCHEMA: [&str; 6] = [
        "name",
        "in",
        "required",
        "description",
        "allowEmptyValue",
        "collectionFormat",
    ];
    let mut s = Map::new();
    for (k, v) in m {
        if NOT_SCHEMA.contains(&k.as_str()) || k.starts_with("x-") {
            continue;
        }
        let v = if k == "type" && v == "file" {
            json!("string")
        } else {
            rewrite_refs(v)
        };
        s.insert(k.clone(), v);
    }
    if m.get("type").and_then(Value::as_str) == Some("file") {
        s.insert("format".to_owned(), json!("binary"));
    }
    Value::Object(s)
}

fn swagger_to_oas3(sw: &Value) -> Result<Value, ConvertError> {
    let root = sw
        .as_object()
        .ok_or_else(|| ConvertError::invalid("a Swagger document must be a JSON object"))?;
    let mut out = Map::new();
    out.insert("openapi".to_owned(), json!("3.0.3"));

    let consumes_root = str_list(root.get("consumes"));
    let produces_root = str_list(root.get("produces"));

    for (k, v) in root {
        match k.as_str() {
            "swagger"
            | "host"
            | "basePath"
            | "schemes"
            | "consumes"
            | "produces"
            | "definitions"
            | "parameters"
            | "responses"
            | "securityDefinitions"
            | "paths" => {}
            _ => {
                out.insert(k.clone(), rewrite_refs(v));
            }
        }
    }

    // servers
    let host = root.get("host").and_then(Value::as_str);
    let base = root.get("basePath").and_then(Value::as_str).unwrap_or("");
    let schemes = {
        let s = str_list(root.get("schemes"));
        if s.is_empty() {
            vec!["https".to_owned()]
        } else {
            s
        }
    };
    let servers: Vec<Value> = match host {
        Some(h) => schemes
            .iter()
            .map(|s| json!({ "url": format!("{s}://{h}{base}") }))
            .collect(),
        None if !base.is_empty() => vec![json!({ "url": base })],
        None => Vec::new(),
    };
    if !servers.is_empty() {
        out.insert("servers".to_owned(), Value::Array(servers));
    }

    // components
    let mut components = Map::new();
    if let Some(d) = root.get("definitions") {
        components.insert("schemas".to_owned(), rewrite_refs(d));
    }
    if let Some(Value::Object(defs)) = root.get("securityDefinitions") {
        let schemes: Map<String, Value> = defs
            .iter()
            .map(|(k, v)| (k.clone(), convert_swagger_scheme(v)))
            .collect();
        components.insert("securitySchemes".to_owned(), Value::Object(schemes));
    }
    if !components.is_empty() {
        out.insert("components".to_owned(), Value::Object(components));
    }

    // paths
    let mut paths = Map::new();
    for (p, item) in root
        .get("paths")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
    {
        let Some(item) = item.as_object() else {
            continue;
        };
        let mut new_item = Map::new();
        for (k, v) in item {
            if METHODS.contains(&k.as_str()) {
                if let Some(op) = v.as_object() {
                    new_item.insert(
                        k.clone(),
                        convert_swagger_op(sw, op, &consumes_root, &produces_root),
                    );
                }
            } else if k == "parameters" {
                let (params, _) =
                    convert_swagger_params(sw, v.as_array().map(Vec::as_slice).unwrap_or(&[]), &[]);
                if !params.is_empty() {
                    new_item.insert(k.clone(), Value::Array(params));
                }
            } else {
                new_item.insert(k.clone(), rewrite_refs(v));
            }
        }
        paths.insert(p.clone(), Value::Object(new_item));
    }
    out.insert("paths".to_owned(), Value::Object(paths));
    Ok(Value::Object(out))
}

fn convert_swagger_scheme(v: &Value) -> Value {
    let Some(m) = v.as_object() else {
        return v.clone();
    };
    let mut out = Map::new();
    match m.get("type").and_then(Value::as_str) {
        Some("basic") => {
            out.insert("type".into(), json!("http"));
            out.insert("scheme".into(), json!("basic"));
        }
        Some("oauth2") => {
            out.insert("type".into(), json!("oauth2"));
            let scopes = m.get("scopes").cloned().unwrap_or_else(|| json!({}));
            let mut flow = Map::new();
            let (name, urls): (&str, &[(&str, &str)]) = match m.get("flow").and_then(Value::as_str)
            {
                Some("implicit") => ("implicit", &[("authorizationUrl", "authorizationUrl")]),
                Some("password") => ("password", &[("tokenUrl", "tokenUrl")]),
                Some("application") => ("clientCredentials", &[("tokenUrl", "tokenUrl")]),
                _ => (
                    "authorizationCode",
                    &[
                        ("authorizationUrl", "authorizationUrl"),
                        ("tokenUrl", "tokenUrl"),
                    ],
                ),
            };
            for (from, to) in urls {
                if let Some(u) = m.get(*from) {
                    flow.insert((*to).to_owned(), u.clone());
                }
            }
            flow.insert("scopes".into(), scopes);
            out.insert("flows".into(), json!({ name: Value::Object(flow) }));
        }
        _ => {
            out.insert("type".into(), m.get("type").cloned().unwrap_or(Value::Null));
            for k in ["name", "in"] {
                if let Some(v) = m.get(k) {
                    out.insert(k.into(), v.clone());
                }
            }
        }
    }
    for (k, v) in m {
        if k == "description" || k.starts_with("x-") {
            out.insert(k.clone(), v.clone());
        }
    }
    Value::Object(out)
}

/// Converts Swagger parameters; returns the OpenAPI parameters and a request body.
fn convert_swagger_params(
    sw: &Value,
    params: &[Value],
    consumes: &[String],
) -> (Vec<Value>, Option<Value>) {
    let mut out = Vec::new();
    let mut body = None;
    let mut form_props = Map::new();
    let mut form_required = Vec::new();
    let mut has_file = false;

    for p in params {
        let Some(m) = resolve(sw, p).as_object() else {
            continue;
        };
        let name = m.get("name").and_then(Value::as_str).unwrap_or_default();
        match m.get("in").and_then(Value::as_str) {
            Some("body") => {
                let mut rb = Map::new();
                if let Some(d) = m.get("description") {
                    rb.insert("description".into(), d.clone());
                }
                if m.get("required") == Some(&json!(true)) {
                    rb.insert("required".into(), json!(true));
                }
                let schema = m
                    .get("schema")
                    .map(rewrite_refs)
                    .unwrap_or_else(|| json!({}));
                let cts = if consumes.is_empty() {
                    vec!["application/json".to_owned()]
                } else {
                    consumes.to_vec()
                };
                let content: Map<String, Value> = cts
                    .into_iter()
                    .map(|ct| (ct, json!({ "schema": schema })))
                    .collect();
                rb.insert("content".into(), Value::Object(content));
                body = Some(Value::Object(rb));
            }
            Some("formData") => {
                let mut schema = simple_schema(m);
                if m.get("type").and_then(Value::as_str) == Some("file") {
                    has_file = true;
                }
                if let (Some(d), Some(s)) = (m.get("description"), schema.as_object_mut()) {
                    s.insert("description".into(), d.clone());
                }
                form_props.insert(name.to_owned(), schema);
                if m.get("required") == Some(&json!(true)) {
                    form_required.push(json!(name));
                }
            }
            Some(loc) => {
                let mut o = Map::new();
                o.insert("name".into(), json!(name));
                o.insert("in".into(), json!(loc));
                if m.get("required") == Some(&json!(true)) {
                    o.insert("required".into(), json!(true));
                }
                if let Some(d) = m.get("description") {
                    o.insert("description".into(), d.clone());
                }
                o.insert("schema".into(), simple_schema(m));
                for (k, v) in m {
                    if k.starts_with("x-") {
                        o.insert(k.clone(), v.clone());
                    }
                }
                out.push(Value::Object(o));
            }
            None => {}
        }
    }

    if !form_props.is_empty() {
        let mut schema = Map::new();
        schema.insert("type".into(), json!("object"));
        schema.insert("properties".into(), Value::Object(form_props));
        if !form_required.is_empty() {
            schema.insert("required".into(), Value::Array(form_required));
        }
        let form_cts: Vec<String> = consumes
            .iter()
            .filter(|c| {
                c.starts_with("multipart/") || c.starts_with("application/x-www-form-urlencoded")
            })
            .cloned()
            .collect();
        let cts = if !form_cts.is_empty() {
            form_cts
        } else if has_file {
            vec!["multipart/form-data".to_owned()]
        } else {
            vec!["application/x-www-form-urlencoded".to_owned()]
        };
        let content: Map<String, Value> = cts
            .into_iter()
            .map(|ct| (ct, json!({ "schema": Value::Object(schema.clone()) })))
            .collect();
        body = Some(json!({ "content": Value::Object(content) }));
    }
    (out, body)
}

fn convert_swagger_op(
    sw: &Value,
    op: &Map<String, Value>,
    consumes_root: &[String],
    produces_root: &[String],
) -> Value {
    let consumes = if op.contains_key("consumes") {
        str_list(op.get("consumes"))
    } else {
        consumes_root.to_vec()
    };
    let produces = if op.contains_key("produces") {
        str_list(op.get("produces"))
    } else {
        produces_root.to_vec()
    };
    let produces = if produces.is_empty() {
        vec!["application/json".to_owned()]
    } else {
        produces
    };

    let mut out = Map::new();
    for (k, v) in op {
        if !matches!(
            k.as_str(),
            "parameters" | "consumes" | "produces" | "responses" | "schemes"
        ) {
            out.insert(k.clone(), rewrite_refs(v));
        }
    }
    let params = op
        .get("parameters")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let (params, body) = convert_swagger_params(sw, params, &consumes);
    if !params.is_empty() {
        out.insert("parameters".into(), Value::Array(params));
    }
    if let Some(b) = body {
        out.insert("requestBody".into(), b);
    }

    let mut responses = Map::new();
    for (code, r) in op
        .get("responses")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
    {
        if code.starts_with("x-") {
            responses.insert(code.clone(), r.clone());
            continue;
        }
        let Some(m) = resolve(sw, r).as_object() else {
            continue;
        };
        let mut o = Map::new();
        o.insert(
            "description".into(),
            m.get("description").cloned().unwrap_or_else(|| json!("")),
        );
        if let Some(Value::Object(hs)) = m.get("headers") {
            let headers: Map<String, Value> = hs
                .iter()
                .map(|(n, h)| {
                    let hm = h.as_object().cloned().unwrap_or_default();
                    let mut ho = Map::new();
                    if let Some(d) = hm.get("description") {
                        ho.insert("description".into(), d.clone());
                    }
                    ho.insert("schema".into(), simple_schema(&hm));
                    (n.clone(), Value::Object(ho))
                })
                .collect();
            o.insert("headers".into(), Value::Object(headers));
        }
        let examples = m.get("examples").and_then(Value::as_object);
        if m.contains_key("schema") || examples.is_some() {
            let mut content = Map::new();
            for ct in &produces {
                let mut media = Map::new();
                if let Some(s) = m.get("schema") {
                    media.insert("schema".into(), rewrite_refs(s));
                }
                if let Some(e) = examples.and_then(|e| e.get(ct)) {
                    media.insert("example".into(), e.clone());
                }
                content.insert(ct.clone(), Value::Object(media));
            }
            o.insert("content".into(), Value::Object(content));
        }
        for (k, v) in m {
            if k.starts_with("x-") {
                o.insert(k.clone(), v.clone());
            }
        }
        responses.insert(code.clone(), Value::Object(o));
    }
    if !responses.is_empty() {
        out.insert("responses".into(), Value::Object(responses));
    }
    Value::Object(out)
}

// ================================================================ export

struct Schemes {
    list: Vec<SecurityScheme>,
}

impl Schemes {
    /// The `security` requirement list for `auth`, registering a scheme if needed.
    fn security(&mut self, auth: &Auth, warnings: &mut Vec<String>) -> Option<Value> {
        let name = match auth {
            Auth::NoAuth => return Some(json!([])),
            Auth::Reference { scheme, scopes } => {
                return Some(json!([{ scheme: scopes }]));
            }
            Auth::Other { kind, .. } if kind.starts_with("postman:") => {
                warnings.push(format!("auth `{kind}` has no OpenAPI equivalent; dropped"));
                return None;
            }
            a => {
                if let Some(s) = self.list.iter().find(|s| same_scheme(&s.auth, a)) {
                    s.name.clone()
                } else {
                    let base = match a {
                        Auth::Basic { .. } => "basicAuth".to_owned(),
                        Auth::Bearer { .. } => "bearerAuth".to_owned(),
                        Auth::ApiKey { .. } => "apiKeyAuth".to_owned(),
                        Auth::Other { kind, .. } => format!("{kind}Auth"),
                        _ => "auth".to_owned(),
                    };
                    let mut name = base.clone();
                    let mut n = 2;
                    while self.list.iter().any(|s| s.name == name) {
                        name = format!("{base}{n}");
                        n += 1;
                    }
                    self.list.push(SecurityScheme {
                        name: name.clone(),
                        auth: a.clone(),
                        ..SecurityScheme::default()
                    });
                    name
                }
            }
        };
        Some(json!([{ name: [] }]))
    }
}

fn same_scheme(a: &Auth, b: &Auth) -> bool {
    match (a, b) {
        (Auth::Basic { .. }, Auth::Basic { .. }) | (Auth::Bearer { .. }, Auth::Bearer { .. }) => {
            true
        }
        (
            Auth::ApiKey {
                key: k1,
                location: l1,
                ..
            },
            Auth::ApiKey {
                key: k2,
                location: l2,
                ..
            },
        ) => k1 == k2 && l1 == l2,
        (
            Auth::Other {
                kind: k1,
                params: p1,
            },
            Auth::Other {
                kind: k2,
                params: p2,
            },
        ) => k1 == k2 && p1 == p2,
        _ => false,
    }
}

fn scheme_json(s: &SecurityScheme, foreign: bool) -> Option<Value> {
    let mut m = Map::new();
    match &s.auth {
        Auth::Basic { .. } => {
            m.insert("type".into(), json!("http"));
            m.insert("scheme".into(), json!("basic"));
        }
        Auth::Bearer { .. } => {
            m.insert("type".into(), json!("http"));
            m.insert("scheme".into(), json!("bearer"));
        }
        Auth::ApiKey { key, location, .. } => {
            m.insert("type".into(), json!("apiKey"));
            m.insert("name".into(), json!(key));
            m.insert("in".into(), json!(location.as_str()));
        }
        Auth::Other { kind, params } if !kind.starts_with("postman:") => {
            m.insert("type".into(), json!(kind));
            m.extend(params.clone());
        }
        _ => return None,
    }
    if let Some(d) = &s.description {
        m.insert("description".into(), json!(d));
    }
    emit_ext(&mut m, &s.extensions, "", &[], foreign);
    Some(Value::Object(m))
}

struct Cx<'a> {
    project: &'a Project,
    foreign: bool,
    schemes: Schemes,
    warnings: Vec<String>,
    paths: Map<String, Value>,
    tags: Vec<Value>,
}

pub fn export_openapi(
    project: &Project,
    version: OpenApiVersion,
) -> Result<Exported, ConvertError> {
    let foreign = matches!(
        project.source.format,
        SourceFormat::OpenApi | SourceFormat::Swagger
    );
    let mut cx = Cx {
        project,
        foreign,
        schemes: Schemes {
            list: project.security_schemes.clone(),
        },
        warnings: Vec::new(),
        paths: Map::new(),
        tags: Vec::new(),
    };

    let openapi = match (&project.source.format, &project.source.version) {
        (SourceFormat::OpenApi, Some(v)) if v.starts_with(version.minor_prefix()) => v.clone(),
        _ => version.default_string().to_owned(),
    };

    let mut info = Map::new();
    info.insert("title".into(), json!(project.name));
    info.insert(
        "version".into(),
        json!(
            project
                .version
                .clone()
                .unwrap_or_else(|| "1.0.0".to_owned())
        ),
    );
    if let Some(d) = &project.description {
        info.insert("description".into(), json!(d));
    }
    emit_ext(&mut info, &project.extensions, "info.", &[], foreign);

    let mut doc = Map::new();
    doc.insert("openapi".into(), json!(openapi));
    doc.insert("info".into(), Value::Object(info));
    if !project.servers.is_empty() {
        doc.insert(
            "servers".into(),
            Value::Array(
                project
                    .servers
                    .iter()
                    .map(|s| export_server(s, foreign))
                    .collect(),
            ),
        );
    }

    let root_reqs = &project.requests;
    for r in root_reqs {
        export_request(r, None, &mut cx);
    }
    walk_dirs(&project.directories, &mut Vec::new(), &mut cx);

    let project_security = project.auth.as_ref().and_then(|a| {
        let mut w = Vec::new();
        let s = cx.schemes.security(a, &mut w);
        cx.warnings.extend(w);
        s
    });

    doc.insert("paths".into(), Value::Object(std::mem::take(&mut cx.paths)));

    let mut components = match project.extensions.get("components") {
        Some(Value::Object(c)) if foreign => c.clone(),
        _ => Map::new(),
    };
    let schemes: Map<String, Value> = cx
        .schemes
        .list
        .iter()
        .filter_map(|s| scheme_json(s, foreign).map(|v| (s.name.clone(), v)))
        .collect();
    if !schemes.is_empty() {
        components.insert("securitySchemes".into(), Value::Object(schemes));
    }
    if !components.is_empty() {
        doc.insert("components".into(), Value::Object(components));
    }
    if let Some(s) = project_security {
        doc.insert("security".into(), s);
    }
    if !cx.tags.is_empty() {
        doc.insert("tags".into(), Value::Array(std::mem::take(&mut cx.tags)));
    }
    if !project.variables.is_empty() {
        doc.insert("x-hozd-variables".into(), json!(project.variables));
    }
    if !project.scripts.is_empty() {
        doc.insert("x-hozd-scripts".into(), json!(project.scripts));
    }
    for (k, v) in &project.extensions {
        if k.starts_with("info.") || k == "components" {
            continue;
        }
        if foreign || k.starts_with("x-") {
            doc.entry(k.clone()).or_insert_with(|| v.clone());
        }
    }
    Ok(Exported {
        document: Value::Object(doc),
        warnings: cx.warnings,
    })
}

fn export_server(s: &Server, foreign: bool) -> Value {
    let mut m = Map::new();
    m.insert("url".into(), json!(s.url));
    if let Some(d) = &s.description {
        m.insert("description".into(), json!(d));
    }
    if !s.variables.is_empty() {
        let vars: Map<String, Value> = s
            .variables
            .iter()
            .map(|v| {
                let mut o = Map::new();
                o.insert("default".into(), json!(v.value));
                if !v.allowed.is_empty() {
                    o.insert("enum".into(), json!(v.allowed));
                }
                if let Some(d) = &v.description {
                    o.insert("description".into(), json!(d));
                }
                emit_ext(&mut o, &v.extensions, "", &[], foreign);
                (v.key.clone(), Value::Object(o))
            })
            .collect();
        m.insert("variables".into(), Value::Object(vars));
    }
    emit_ext(&mut m, &s.extensions, "", &[], foreign);
    Value::Object(m)
}

fn walk_dirs(dirs: &[Directory], stack: &mut Vec<String>, cx: &mut Cx) {
    for d in dirs {
        stack.push(d.name.clone());
        let tag = stack.join("/");
        let mut t = Map::new();
        t.insert("name".into(), json!(tag));
        if let Some(desc) = &d.description {
            t.insert("description".into(), json!(desc));
        }
        if stack.len() > 1 {
            t.insert("x-hozd-path".into(), json!(stack));
        }
        if !d.variables.is_empty() {
            t.insert("x-hozd-variables".into(), json!(d.variables));
        }
        if !d.scripts.is_empty() {
            t.insert("x-hozd-scripts".into(), json!(d.scripts));
        }
        if let Some(a) = &d.auth {
            t.insert("x-hozd-auth".into(), json!(a));
        }
        emit_ext(&mut t, &d.extensions, "", &[], cx.foreign);
        cx.tags.push(Value::Object(t));
        for r in &d.requests {
            export_request(r, Some(&tag), cx);
        }
        walk_dirs(&d.directories, stack, cx);
        stack.pop();
    }
}

fn export_request(r: &Request, dir_tag: Option<&str>, cx: &mut Cx) {
    let ApiRequest::Rest(rest) = &r.api;
    let method = match &rest.method {
        Method::Custom(m) => {
            cx.warnings.push(format!(
                "request `{}`: method {m} cannot be expressed in OpenAPI 3.x; skipped",
                r.name
            ));
            return;
        }
        m => m.as_str().to_ascii_lowercase(),
    };
    let path = if rest.path.starts_with('/') {
        rest.path.clone()
    } else {
        format!("/{}", rest.path)
    };

    let mut op = Map::new();
    let mut tags = rest.meta.tags.clone();
    if let Some(t) = dir_tag
        && tags.first().map(String::as_str) != Some(t)
    {
        tags.retain(|x| x != t);
        tags.insert(0, t.to_owned());
    }
    if !tags.is_empty() {
        op.insert("tags".into(), json!(tags));
    }
    let summary = rest.meta.summary.clone().or_else(|| {
        let name_is_default = r.name.is_empty() || r.name == default_name(rest);
        (!name_is_default).then(|| r.name.clone())
    });
    if let Some(s) = summary {
        op.insert("summary".into(), json!(s));
    }
    if let Some(d) = &r.description {
        op.insert("description".into(), json!(d));
    }
    if let Some(id) = &rest.meta.operation_id {
        op.insert("operationId".into(), json!(id));
    }
    if rest.meta.deprecated {
        op.insert("deprecated".into(), json!(true));
    }

    let mut params: Vec<Value> = rest
        .params
        .iter()
        .map(|p| param_object(p, true, cx.foreign))
        .collect();
    for name in path_placeholders(&path) {
        if !rest
            .params_in(ParamLocation::Path)
            .any(|p| p.pair.name == name)
        {
            params.push(json!({ "name": name, "in": "path", "required": true, "schema": { "type": "string" } }));
        }
    }
    if !params.is_empty() {
        op.insert("parameters".into(), Value::Array(params));
    }
    if let Some(b) = &rest.body {
        op.insert("requestBody".into(), export_body(b, cx.foreign));
    }

    let mut responses = Map::new();
    for resp in &rest.responses {
        let code = if resp.code.is_empty() {
            "default".to_owned()
        } else {
            resp.code.clone()
        };
        if responses.contains_key(&code) {
            cx.warnings.push(format!(
                "request `{}`: second response for `{code}` skipped",
                r.name
            ));
            continue;
        }
        responses.insert(code, export_response(resp, cx.foreign));
    }
    if responses.is_empty() {
        responses.insert("default".into(), json!({ "description": "" }));
    }
    emit_ext(&mut responses, &r.extensions, "responses.", &[], cx.foreign);
    op.insert("responses".into(), Value::Object(responses));

    if let Some(a) = &rest.auth {
        let mut w = Vec::new();
        if let Some(s) = cx.schemes.security(a, &mut w) {
            op.insert("security".into(), s);
        }
        cx.warnings.extend(w);
    }
    if let Some(server) = &rest.server {
        let covered = server.contains("{{") || cx.project.servers.iter().any(|s| &s.url == server);
        if !covered {
            op.insert("servers".into(), json!([{ "url": server }]));
        }
    }
    if !rest.scripts.is_empty() {
        op.insert("x-hozd-scripts".into(), json!(rest.scripts));
    }
    if !rest.settings.is_empty() {
        op.insert("x-hozd-settings".into(), json!(rest.settings));
    }
    emit_ext(
        &mut op,
        &r.extensions,
        "",
        &["pathItem.", "responses.", "request.", "url."],
        cx.foreign,
    );

    let item = cx
        .paths
        .entry(path.clone())
        .or_insert_with(|| Value::Object(Map::new()));
    let Value::Object(item) = item else { return };
    if item.contains_key(&method) {
        cx.warnings.push(format!(
            "request `{}`: {} {path} already defined; skipped",
            r.name,
            method.to_uppercase()
        ));
        return;
    }
    item.insert(method, Value::Object(op));
    emit_ext(item, &r.extensions, "pathItem.", &[], cx.foreign);
}

/// An OpenAPI parameter (`with_name_in`) or header object.
fn param_object(p: &RestParam, with_name_in: bool, foreign: bool) -> Value {
    let mut m = Map::new();
    if with_name_in {
        m.insert("name".into(), json!(p.pair.name));
        m.insert("in".into(), json!(p.location.as_str()));
    }
    if p.required || (with_name_in && p.location == ParamLocation::Path) {
        m.insert("required".into(), json!(true));
    }
    if let Some(d) = &p.pair.description {
        m.insert("description".into(), json!(d));
    }
    m.insert(
        "schema".into(),
        p.schema
            .clone()
            .unwrap_or_else(|| json!({ "type": "string" })),
    );
    if !p.pair.value.is_empty() {
        m.insert("example".into(), json!(p.pair.value));
    }
    emit_ext(&mut m, &p.pair.extensions, "", &[], foreign);
    Value::Object(m)
}

fn default_content_type(c: &BodyContent) -> &'static str {
    match c {
        BodyContent::Raw { language, .. } => match language.as_deref() {
            Some("json") => "application/json",
            Some("xml") => "application/xml",
            Some("html") => "text/html",
            _ => "text/plain",
        },
        BodyContent::FormUrlEncoded { .. } => "application/x-www-form-urlencoded",
        BodyContent::Multipart { .. } => "multipart/form-data",
        BodyContent::Binary { .. } => "application/octet-stream",
    }
}

fn export_body(b: &RestBody, foreign: bool) -> Value {
    let ct = b
        .content_type
        .clone()
        .unwrap_or_else(|| default_content_type(&b.content).to_owned());
    let mut media = Map::new();
    let mut schema = b.schema.clone();
    let form = |fields: &[FormField],
                schema: &mut Option<Value>,
                media: &mut Map<String, Value>| {
        if schema.is_none() && !fields.is_empty() {
            let props: Map<String, Value> = fields
                .iter()
                .map(|f| {
                    let s = if f.kind == FieldKind::File {
                        json!({ "type": "string", "format": "binary" })
                    } else {
                        json!({ "type": "string" })
                    };
                    (f.pair.name.clone(), s)
                })
                .collect();
            *schema = Some(json!({ "type": "object", "properties": Value::Object(props) }));
        }
        let example: Map<String, Value> = fields
            .iter()
            .filter(|f| f.kind == FieldKind::Text && !f.pair.value.is_empty() && !f.pair.disabled)
            .map(|f| (f.pair.name.clone(), json!(f.pair.value)))
            .collect();
        if !example.is_empty() {
            media.insert("example".into(), Value::Object(example));
        }
    };
    match &b.content {
        BodyContent::Raw { text, .. } => {
            if !text.is_empty() {
                media.insert("example".into(), text_to_example(&ct, text));
            }
        }
        BodyContent::FormUrlEncoded { fields } | BodyContent::Multipart { fields } => {
            form(fields, &mut schema, &mut media);
        }
        BodyContent::Binary { .. } => {
            if schema.is_none() {
                schema = Some(json!({ "type": "string", "format": "binary" }));
            }
        }
    }
    if let Some(s) = schema {
        media.insert("schema".into(), s);
    }
    media.extend(b.media_extensions.clone());

    let mut content = Map::new();
    content.insert(ct, Value::Object(media));
    content.extend(b.other_media.clone());

    let mut m = Map::new();
    if let Some(d) = &b.description {
        m.insert("description".into(), json!(d));
    }
    if b.required {
        m.insert("required".into(), json!(true));
    }
    m.insert("content".into(), Value::Object(content));
    emit_ext(&mut m, &b.extensions, "", &[], foreign);
    Value::Object(m)
}

fn export_response(r: &RestResponse, foreign: bool) -> Value {
    let mut m = Map::new();
    let description = r
        .status_text
        .clone()
        .or_else(|| Some(r.name.clone()))
        .unwrap_or_default();
    m.insert("description".into(), json!(description));
    if !r.headers.is_empty() {
        let hs: Map<String, Value> = r
            .headers
            .iter()
            .map(|h| (h.pair.name.clone(), param_object(h, false, foreign)))
            .collect();
        m.insert("headers".into(), Value::Object(hs));
    }
    if r.body.is_some() || r.schema.is_some() || r.content_type.is_some() {
        let ct = r.content_type.clone().unwrap_or_else(|| {
            let looks_json = r.body.as_deref().is_some_and(|b| {
                serde_json::from_str::<Value>(b).is_ok_and(|v| v.is_object() || v.is_array())
            });
            match r.language.as_deref() {
                Some("json") => "application/json",
                Some("xml") => "application/xml",
                Some("html") => "text/html",
                Some("text") => "text/plain",
                _ if looks_json => "application/json",
                _ => "text/plain",
            }
            .to_owned()
        });
        let mut media = Map::new();
        if let Some(s) = &r.schema {
            media.insert("schema".into(), s.clone());
        }
        if let Some(b) = &r.body {
            media.insert("example".into(), text_to_example(&ct, b));
        }
        media.extend(r.media_extensions.clone());
        let mut content = Map::new();
        content.insert(ct, Value::Object(media));
        content.extend(r.other_media.clone());
        m.insert("content".into(), Value::Object(content));
    }
    emit_ext(&mut m, &r.extensions, "", &[], foreign);
    Value::Object(m)
}

/// Names of the `{name}` placeholders in an OpenAPI path.
pub(crate) fn path_placeholders(path: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = path;
    while let Some(start) = rest.find('{') {
        let Some(len) = rest[start..].find('}') else {
            break;
        };
        let name = &rest[start + 1..start + len];
        if !name.is_empty() && !name.contains('{') {
            out.push(name.to_owned());
        }
        rest = &rest[start + len + 1..];
    }
    out
}
