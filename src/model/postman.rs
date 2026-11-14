//! Postman Collection v2.1 import and export.
//!
//! Limits: within one folder, subfolders are written before requests (the model keeps
//! them in separate lists), and a description given as `{content, type}` keeps only its
//! content.

use serde_json::{Map, Value, json};

use super::common::*;
use super::project::{ApiRequest, Directory, Project, Request};
use super::rest::*;
use super::util::*;

const DEFAULT_VERSION: &str = "2.1.0";

// ================================================================ import

pub fn import_postman(doc: &Value) -> Result<Project, ConvertError> {
    let mut root = doc
        .as_object()
        .ok_or_else(|| ConvertError::invalid("a Postman collection must be a JSON object"))?
        .clone();
    let mut info = take_object(&mut root, "info")
        .ok_or_else(|| ConvertError::invalid("missing `info` in Postman collection"))?;

    let schema = take_string(&mut info, "schema").unwrap_or_default();
    if !schema.contains("/v2.1") {
        return Err(ConvertError::Unsupported(format!(
            "Postman schema `{schema}`; only v2.1 is supported"
        )));
    }
    let version = schema
        .rsplit('/')
        .nth(1)
        .and_then(|s| s.strip_prefix('v'))
        .unwrap_or(DEFAULT_VERSION)
        .to_owned();

    let mut project = Project {
        name: take_string(&mut info, "name").unwrap_or_default(),
        source: Source {
            format: SourceFormat::Postman,
            version: Some(version),
        },
        ..Project::default()
    };
    project.id = take_string(&mut info, "_postman_id")
        .unwrap_or_else(|| derive_id(&["postman", &project.name]));
    project.description = take_description(&mut info);
    project.version = take_string(&mut info, "version");
    stash(&mut project.extensions, "info.", info);

    project.variables = import_variables(take_array(&mut root, "variable"));
    if let Some(v) = project
        .variables
        .iter()
        .find(|v| v.key == "baseUrl" && !v.value.is_empty())
    {
        project.servers.push(Server {
            url: v.value.clone(),
            ..Server::default()
        });
    }
    project.auth = root.remove("auth").and_then(import_auth);
    project.scripts = import_events(take_array(&mut root, "event"));

    let items = take_array(&mut root, "item");
    import_items(
        items,
        &project.id.clone(),
        &mut project.directories,
        &mut project.requests,
    );
    project.extensions.extend(root);
    Ok(project)
}

fn import_items(
    items: Vec<Value>,
    parent: &str,
    dirs: &mut Vec<Directory>,
    reqs: &mut Vec<Request>,
) {
    for (i, item) in items.into_iter().enumerate() {
        let Value::Object(mut m) = item else { continue };
        let name = take_string(&mut m, "name").unwrap_or_default();
        let key = format!("{parent}/{i}:{name}");
        let id = take_string(&mut m, "id").unwrap_or_else(|| derive_id(&["postman-item", &key]));
        if m.contains_key("item") {
            let mut dir = Directory {
                id,
                name,
                ..Directory::default()
            };
            dir.description = take_description(&mut m);
            dir.auth = m.remove("auth").and_then(import_auth);
            dir.scripts = import_events(take_array(&mut m, "event"));
            dir.variables = import_variables(take_array(&mut m, "variable"));
            let children = take_array(&mut m, "item");
            import_items(children, &key, &mut dir.directories, &mut dir.requests);
            dir.extensions = m;
            dirs.push(dir);
        } else {
            reqs.push(import_request(id, name, m));
        }
    }
}

fn import_request(id: Id, name: String, mut m: Map<String, Value>) -> Request {
    let mut rest = RestRequest::default();
    let mut ext = Extensions::new();

    rest.scripts = import_events(take_array(&mut m, "event"));
    rest.responses = take_array(&mut m, "response")
        .into_iter()
        .enumerate()
        .filter_map(|(i, v)| import_response(v, &id, i))
        .collect();
    if let Some(mut ppb) = take_object(&mut m, "protocolProfileBehavior") {
        rest.settings.follow_redirects = take_bool(&mut ppb, "followRedirects");
        rest.settings.verify_tls = take_bool(&mut ppb, "strictSSL");
        rest.settings.extensions = ppb;
    }
    let mut description = take_description(&mut m);

    match m.remove("request") {
        Some(Value::Object(mut r)) => {
            description = take_description(&mut r).or(description);
            if let Some(method) = take_string(&mut r, "method") {
                rest.method = Method::parse(&method);
            }
            let mut params = Vec::new();
            let url = r.remove("url");
            for h in take_array(&mut r, "header") {
                if let Some(p) = import_param(h, ParamLocation::Header) {
                    params.push(p);
                }
            }
            if let Some(Value::Object(b)) = r.remove("body") {
                rest.body = Some(import_body(b));
            }
            rest.auth = r.remove("auth").and_then(import_auth);
            rest.settings.proxy = r.remove("proxy");
            rest.settings.certificate = r.remove("certificate");
            stash(&mut ext, "request.", r);
            if let Some(url) = url {
                let u = import_url(url, &mut ext);
                rest.server = u.server;
                rest.path = u.path;
                params.extend(u.path_vars);
                params.extend(u.query);
            }
            // keep headers first, then query, like the usual reading order
            rest.params = params;
        }
        Some(Value::String(url)) => {
            let u = import_url(Value::String(url), &mut ext);
            rest.server = u.server;
            rest.path = u.path;
            rest.params = u.query;
        }
        Some(other) => {
            m.insert("request".to_owned(), other);
        }
        None => {}
    }
    ext.extend(m);

    Request {
        id,
        name,
        description,
        api: ApiRequest::Rest(rest),
        extensions: ext,
    }
}

struct UrlParts {
    server: Option<String>,
    path: String,
    query: Vec<RestParam>,
    path_vars: Vec<RestParam>,
}

fn import_url(url: Value, ext: &mut Extensions) -> UrlParts {
    let mut parts = UrlParts {
        server: None,
        path: String::new(),
        query: vec![],
        path_vars: vec![],
    };
    match url {
        Value::String(raw) => {
            let (server, path, query, hash) = split_raw(&raw);
            parts.server = server;
            parts.path = convert_path(&path);
            parts.query = parse_query(query.as_deref());
            if let Some(h) = hash {
                ext.insert("url.hash".to_owned(), Value::String(h));
            }
        }
        Value::Object(mut u) => {
            let raw = take_string(&mut u, "raw");
            let protocol = take_string(&mut u, "protocol");
            let port = take_string(&mut u, "port");
            let host = match u.remove("host") {
                Some(Value::String(s)) => Some(s),
                Some(Value::Array(a)) => {
                    let segs: Vec<String> = a.iter().map(value_to_string).collect();
                    Some(segs.join("."))
                }
                _ => None,
            };
            let from_raw = raw.as_deref().map(split_raw);
            parts.server = match host {
                Some(h) => Some(format!(
                    "{}{h}{}",
                    protocol.map(|p| format!("{p}://")).unwrap_or_default(),
                    port.map(|p| format!(":{p}")).unwrap_or_default()
                )),
                None => from_raw.as_ref().and_then(|r| r.0.clone()),
            };
            parts.path = match u.remove("path") {
                Some(Value::String(s)) => convert_path(&format!("/{}", s.trim_start_matches('/'))),
                Some(Value::Array(a)) => {
                    let segs: Vec<String> = a
                        .iter()
                        .map(|s| match s {
                            Value::Object(o) => {
                                o.get("value").map(value_to_string).unwrap_or_default()
                            }
                            other => value_to_string(other),
                        })
                        .map(|s| colon_to_brace(&s))
                        .collect();
                    if segs.is_empty() {
                        String::new()
                    } else {
                        format!("/{}", segs.join("/"))
                    }
                }
                _ => from_raw
                    .as_ref()
                    .map(|r| convert_path(&r.1))
                    .unwrap_or_default(),
            };
            for q in take_array(&mut u, "query") {
                if let Some(p) = import_param(q, ParamLocation::Query) {
                    parts.query.push(p);
                }
            }
            for v in take_array(&mut u, "variable") {
                if let Some(p) = import_param(v, ParamLocation::Path) {
                    parts.path_vars.push(p);
                }
            }
            stash(ext, "url.", u);
        }
        _ => {}
    }
    parts
}

/// Splits `raw` into (server, path, query, hash).
fn split_raw(raw: &str) -> (Option<String>, String, Option<String>, Option<String>) {
    let (rest, hash) = match raw.split_once('#') {
        Some((a, b)) => (a, Some(b.to_owned())),
        None => (raw, None),
    };
    let (rest, query) = match rest.split_once('?') {
        Some((a, b)) => (a, Some(b.to_owned())),
        None => (rest, None),
    };
    let authority_start = rest.find("://").map(|i| i + 3).unwrap_or(0);
    let (server, path) = if rest.is_empty() {
        (None, String::new())
    } else if rest.starts_with('/') {
        (None, rest.to_owned())
    } else {
        match rest[authority_start..].find('/') {
            Some(i) => {
                let at = authority_start + i;
                (Some(rest[..at].to_owned()), rest[at..].to_owned())
            }
            None => (Some(rest.to_owned()), String::new()),
        }
    };
    (server, path, query, hash)
}

fn parse_query(q: Option<&str>) -> Vec<RestParam> {
    q.map(|q| {
        q.split('&')
            .filter(|s| !s.is_empty())
            .map(|kv| {
                let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
                RestParam::new(ParamLocation::Query, k, v)
            })
            .collect()
    })
    .unwrap_or_default()
}

fn convert_path(path: &str) -> String {
    path.split('/')
        .map(colon_to_brace)
        .collect::<Vec<_>>()
        .join("/")
}

fn import_param(v: Value, location: ParamLocation) -> Option<RestParam> {
    let Value::Object(mut m) = v else { return None };
    let mut p = RestParam {
        location,
        ..RestParam::default()
    };
    p.pair.name = take_string(&mut m, "key").unwrap_or_default();
    p.pair.value = m
        .remove("value")
        .map(|v| value_to_string(&v))
        .unwrap_or_default();
    p.pair.description = take_description(&mut m);
    p.pair.disabled = take_bool(&mut m, "disabled").unwrap_or(false);
    p.pair.extensions = m;
    Some(p)
}

fn import_body(mut b: Map<String, Value>) -> RestBody {
    let mut body = RestBody::default();
    let mode = take_string(&mut b, "mode").unwrap_or_default();
    match mode.as_str() {
        "raw" => {
            let text = take_string(&mut b, "raw").unwrap_or_default();
            let mut language = None;
            if let Some(mut opts) = take_object(&mut b, "options") {
                if let Some(mut raw) = take_object(&mut opts, "raw") {
                    language = take_string(&mut raw, "language");
                    if !raw.is_empty() {
                        opts.insert("raw".to_owned(), Value::Object(raw));
                    }
                }
                if !opts.is_empty() {
                    b.insert("options".to_owned(), Value::Object(opts));
                }
            }
            body.content = BodyContent::Raw { text, language };
        }
        "urlencoded" => {
            body.content = BodyContent::FormUrlEncoded {
                fields: import_fields(take_array(&mut b, "urlencoded")),
            };
        }
        "formdata" => {
            body.content = BodyContent::Multipart {
                fields: import_fields(take_array(&mut b, "formdata")),
            };
        }
        "file" => {
            let mut file = None;
            if let Some(mut f) = take_object(&mut b, "file") {
                file = take_string(&mut f, "src");
                if !f.is_empty() {
                    b.insert("file".to_owned(), Value::Object(f));
                }
            }
            body.content = BodyContent::Binary { file };
        }
        other => {
            // graphql and anything newer: keep the mode and payload untouched
            b.insert("mode".to_owned(), Value::String(other.to_owned()));
        }
    }
    body.extensions = b;
    body
}

fn import_fields(items: Vec<Value>) -> Vec<FormField> {
    items
        .into_iter()
        .filter_map(|v| {
            let Value::Object(mut m) = v else { return None };
            let mut f = FormField::default();
            f.pair.name = take_string(&mut m, "key").unwrap_or_default();
            if take_string(&mut m, "type").as_deref() == Some("file") {
                f.kind = FieldKind::File;
                f.pair.value = take_string(&mut m, "src").unwrap_or_default();
                // keep an explicit `type: "file"` on export: it is implied by `kind`
            } else {
                f.pair.value = m
                    .remove("value")
                    .map(|v| value_to_string(&v))
                    .unwrap_or_default();
            }
            f.content_type = take_string(&mut m, "contentType");
            f.pair.description = take_description(&mut m);
            f.pair.disabled = take_bool(&mut m, "disabled").unwrap_or(false);
            f.pair.extensions = m;
            Some(f)
        })
        .collect()
}

fn import_response(v: Value, request_id: &str, i: usize) -> Option<RestResponse> {
    let Value::Object(mut m) = v else { return None };
    let mut r = RestResponse::default();
    r.name = take_string(&mut m, "name").unwrap_or_default();
    r.id = take_string(&mut m, "id")
        .unwrap_or_else(|| derive_id(&["postman-response", request_id, &i.to_string()]));
    r.status_text = take_string(&mut m, "status");
    r.code = match m.remove("code") {
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::String(s)) => s,
        Some(other) => {
            m.insert("code".to_owned(), other);
            String::new()
        }
        None => String::new(),
    };
    for h in take_array(&mut m, "header") {
        if let Some(p) = import_param(h, ParamLocation::Header) {
            r.headers.push(p);
        }
    }
    r.body = take_string(&mut m, "body");
    r.language = take_string(&mut m, "_postman_previewlanguage");
    r.original_request = m.remove("originalRequest");
    r.extensions = m;
    Some(r)
}

fn import_events(events: Vec<Value>) -> Vec<Script> {
    events
        .into_iter()
        .filter_map(|v| {
            let Value::Object(mut e) = v else { return None };
            let mut s = Script::default();
            s.kind = match take_string(&mut e, "listen").as_deref() {
                Some("prerequest") => ScriptKind::PreRequest,
                Some("test") => ScriptKind::Test,
                Some(other) => ScriptKind::Other(other.to_owned()),
                None => ScriptKind::PreRequest,
            };
            s.disabled = take_bool(&mut e, "disabled").unwrap_or(false);
            if let Some(mut script) = take_object(&mut e, "script") {
                s.language = take_string(&mut script, "type");
                s.source = match script.remove("exec") {
                    Some(Value::Array(lines)) => lines
                        .iter()
                        .map(value_to_string)
                        .collect::<Vec<_>>()
                        .join("\n"),
                    Some(Value::String(t)) => t,
                    _ => String::new(),
                };
                stash(&mut s.extensions, "script.", script);
            }
            s.extensions.extend(e);
            Some(s)
        })
        .collect()
}

fn import_variables(items: Vec<Value>) -> Vec<Variable> {
    items
        .into_iter()
        .filter_map(|v| {
            let Value::Object(mut m) = v else { return None };
            let mut var = Variable::default();
            var.key = take_string(&mut m, "key").unwrap_or_default();
            var.value = m
                .remove("value")
                .map(|v| value_to_string(&v))
                .unwrap_or_default();
            var.kind = take_string(&mut m, "type");
            var.description = take_description(&mut m);
            var.disabled = take_bool(&mut m, "disabled").unwrap_or(false);
            var.extensions = m;
            Some(var)
        })
        .collect()
}

/// Postman stores auth attributes as `[{key, value, type}]`.
fn attrs(auth: &Map<String, Value>, kind: &str) -> Map<String, Value> {
    let mut out = Map::new();
    if let Some(Value::Array(list)) = auth.get(kind) {
        for a in list {
            if let Some(k) = a.get("key").and_then(Value::as_str) {
                out.insert(k.to_owned(), a.get("value").cloned().unwrap_or(Value::Null));
            }
        }
    }
    out
}

fn import_auth(v: Value) -> Option<Auth> {
    let Value::Object(a) = v else { return None };
    let kind = a.get("type")?.as_str()?.to_owned();
    let s = |m: &Map<String, Value>, k: &str| m.get(k).map(value_to_string).unwrap_or_default();
    Some(match kind.as_str() {
        "noauth" => Auth::NoAuth,
        "basic" => {
            let m = attrs(&a, "basic");
            Auth::Basic {
                username: s(&m, "username"),
                password: s(&m, "password"),
            }
        }
        "bearer" => Auth::Bearer {
            token: s(&attrs(&a, "bearer"), "token"),
        },
        "apikey" => {
            let m = attrs(&a, "apikey");
            Auth::ApiKey {
                key: s(&m, "key"),
                value: s(&m, "value"),
                location: ApiKeyLocation::parse(&s(&m, "in")),
            }
        }
        other => {
            let mut params = Extensions::new();
            if let Some(list) = a.get(other) {
                params.insert("attrs".to_owned(), list.clone());
            }
            Auth::Other {
                kind: format!("postman:{other}"),
                params,
            }
        }
    })
}

// ================================================================ export

struct Cx<'a> {
    project: &'a Project,
    /// Keys of the project's own format are written back.
    foreign: bool,
    warnings: Vec<String>,
}

pub fn export_postman(project: &Project) -> Result<Exported, ConvertError> {
    let mut cx = Cx {
        project,
        foreign: project.source.format == SourceFormat::Postman,
        warnings: Vec::new(),
    };

    let mut info = Map::new();
    if !is_derived(&project.id) && !project.id.is_empty() {
        info.insert("_postman_id".into(), json!(project.id));
    }
    info.insert("name".into(), json!(project.name));
    if let Some(d) = &project.description {
        info.insert("description".into(), json!(d));
    }
    if let Some(v) = &project.version {
        info.insert("version".into(), json!(v));
    }
    let version = match (&project.source.format, &project.source.version) {
        (SourceFormat::Postman, Some(v)) => v.clone(),
        _ => DEFAULT_VERSION.to_owned(),
    };
    info.insert(
        "schema".into(),
        json!(format!(
            "https://schema.getpostman.com/json/collection/v{version}/collection.json"
        )),
    );
    emit_ext(&mut info, &project.extensions, "info.", &[], cx.foreign);

    let mut doc = Map::new();
    doc.insert("info".into(), Value::Object(info));
    let items = export_items(&project.directories, &project.requests, &mut cx);
    doc.insert("item".into(), Value::Array(items));

    let mut vars: Vec<Value> = project.variables.iter().map(export_variable).collect();
    for (i, s) in project.servers.iter().enumerate() {
        let key = if i == 0 {
            "baseUrl".to_owned()
        } else {
            format!("baseUrl{}", i + 1)
        };
        if !project.variables.iter().any(|v| v.key == key) {
            vars.push(json!({ "key": key, "value": s.url }));
        }
    }
    if !vars.is_empty() {
        doc.insert("variable".into(), Value::Array(vars));
    }
    if let Some(a) = &project.auth
        && let Some(v) = export_auth(a, &mut cx)
    {
        doc.insert("auth".into(), v);
    }
    if !project.scripts.is_empty() {
        doc.insert("event".into(), export_events(&project.scripts));
    }
    emit_ext(&mut doc, &project.extensions, "", &["info."], cx.foreign);
    Ok(Exported {
        document: Value::Object(doc),
        warnings: cx.warnings,
    })
}

fn export_items(dirs: &[Directory], reqs: &[Request], cx: &mut Cx) -> Vec<Value> {
    let mut out = Vec::new();
    for d in dirs {
        let mut m = Map::new();
        if !is_derived(&d.id) {
            m.insert("id".into(), json!(d.id));
        }
        m.insert("name".into(), json!(d.name));
        if let Some(desc) = &d.description {
            m.insert("description".into(), json!(desc));
        }
        m.insert(
            "item".into(),
            Value::Array(export_items(&d.directories, &d.requests, cx)),
        );
        if let Some(a) = &d.auth
            && let Some(v) = export_auth(a, cx)
        {
            m.insert("auth".into(), v);
        }
        if !d.scripts.is_empty() {
            m.insert("event".into(), export_events(&d.scripts));
        }
        if !d.variables.is_empty() {
            m.insert(
                "variable".into(),
                Value::Array(d.variables.iter().map(export_variable).collect()),
            );
        }
        emit_ext(&mut m, &d.extensions, "", &[], cx.foreign);
        out.push(Value::Object(m));
    }
    for r in reqs {
        out.push(export_request(r, cx));
    }
    out
}

fn export_request(r: &Request, cx: &mut Cx) -> Value {
    let ApiRequest::Rest(rest) = &r.api;
    let mut item = Map::new();
    if !is_derived(&r.id) {
        item.insert("id".into(), json!(r.id));
    }
    item.insert("name".into(), json!(r.name));
    if !rest.scripts.is_empty() {
        item.insert("event".into(), export_events(&rest.scripts));
    }
    if !rest.settings.is_empty() {
        let mut ppb = Map::new();
        if let Some(b) = rest.settings.follow_redirects {
            ppb.insert("followRedirects".into(), json!(b));
        }
        if let Some(b) = rest.settings.verify_tls {
            ppb.insert("strictSSL".into(), json!(b));
        }
        ppb.extend(rest.settings.extensions.clone());
        if !ppb.is_empty() {
            item.insert("protocolProfileBehavior".into(), Value::Object(ppb));
        }
    }

    let mut req = Map::new();
    req.insert("method".into(), json!(rest.method.as_str()));
    let mut headers: Vec<Value> = rest
        .params_in(ParamLocation::Header)
        .map(export_param)
        .collect();
    let cookies: Vec<String> = rest
        .params_in(ParamLocation::Cookie)
        .filter(|p| !p.pair.disabled)
        .map(|p| format!("{}={}", p.pair.name, p.pair.value))
        .collect();
    if !cookies.is_empty() {
        headers.push(json!({ "key": "Cookie", "value": cookies.join("; ") }));
    }
    if !headers.is_empty() {
        req.insert("header".into(), Value::Array(headers));
    }
    if let Some(b) = &rest.body {
        req.insert("body".into(), export_body(b, cx));
    }
    req.insert("url".into(), export_url(rest, r, cx));
    if let Some(d) = &r.description {
        req.insert("description".into(), json!(d));
    }
    if let Some(a) = &rest.auth
        && let Some(v) = export_auth(a, cx)
    {
        req.insert("auth".into(), v);
    }
    if let Some(p) = &rest.settings.proxy {
        req.insert("proxy".into(), p.clone());
    }
    if let Some(c) = &rest.settings.certificate {
        req.insert("certificate".into(), c.clone());
    }
    emit_ext(&mut req, &r.extensions, "request.", &[], cx.foreign);
    item.insert("request".into(), Value::Object(req));

    if !rest.responses.is_empty() {
        item.insert(
            "response".into(),
            Value::Array(rest.responses.iter().map(export_response).collect()),
        );
    }
    emit_ext(
        &mut item,
        &r.extensions,
        "",
        &["request.", "url."],
        cx.foreign,
    );
    Value::Object(item)
}

fn export_param(p: &RestParam) -> Value {
    let mut m = Map::new();
    m.insert("key".into(), json!(p.pair.name));
    m.insert("value".into(), json!(p.pair.value));
    if let Some(d) = &p.pair.description {
        m.insert("description".into(), json!(d));
    }
    if p.pair.disabled {
        m.insert("disabled".into(), json!(true));
    }
    // extensions of a Postman param are Postman keys; foreign ones are not meaningful
    for (k, v) in &p.pair.extensions {
        m.insert(k.clone(), v.clone());
    }
    Value::Object(m)
}

fn export_url(rest: &RestRequest, r: &Request, cx: &Cx) -> Value {
    let server = rest
        .server
        .clone()
        .or_else(|| (!cx.project.servers.is_empty()).then(|| "{{baseUrl}}".to_owned()));

    let mut protocol = None;
    let mut host = None;
    let mut port = None;
    let mut prefix: Vec<String> = Vec::new();
    let mut origin = None;
    if let Some(s) = &server {
        let (proto, rest_s) = match s.split_once("://") {
            Some((p, r)) => (Some(p.to_owned()), r),
            None => (None, s.as_str()),
        };
        let (authority, tail) = match rest_s.split_once('/') {
            Some((a, t)) => (a, Some(t)),
            None => (rest_s, None),
        };
        let (h, p) = match authority.rsplit_once(':') {
            Some((h, p)) if !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()) => {
                (h, Some(p.to_owned()))
            }
            _ => (authority, None),
        };
        origin = Some(match &proto {
            Some(pr) => format!("{pr}://{authority}"),
            None => authority.to_owned(),
        });
        protocol = proto;
        port = p;
        host = Some(h.split('.').map(str::to_owned).collect::<Vec<_>>());
        if let Some(t) = tail {
            prefix = t
                .split('/')
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .collect();
        }
    }
    let mut segs = prefix;
    segs.extend(
        rest.path
            .split('/')
            .filter(|s| !s.is_empty())
            .map(brace_to_colon),
    );

    let queries: Vec<&RestParam> = rest.params_in(ParamLocation::Query).collect();
    let query_text: Vec<String> = queries
        .iter()
        .filter(|p| !p.pair.disabled)
        .map(|p| format!("{}={}", p.pair.name, p.pair.value))
        .collect();
    let mut raw = origin.unwrap_or_default();
    if !segs.is_empty() {
        raw.push('/');
        raw.push_str(&segs.join("/"));
    }
    if !query_text.is_empty() {
        raw.push('?');
        raw.push_str(&query_text.join("&"));
    }

    let mut u = Map::new();
    u.insert("raw".into(), json!(raw));
    if let Some(p) = protocol {
        u.insert("protocol".into(), json!(p));
    }
    if let Some(h) = host {
        u.insert("host".into(), json!(h));
    }
    if let Some(p) = port {
        u.insert("port".into(), json!(p));
    }
    if !segs.is_empty() {
        u.insert("path".into(), json!(segs));
    }
    if !queries.is_empty() {
        u.insert(
            "query".into(),
            Value::Array(queries.into_iter().map(export_param).collect()),
        );
    }
    let vars: Vec<Value> = rest
        .params_in(ParamLocation::Path)
        .map(export_param)
        .collect();
    if !vars.is_empty() {
        u.insert("variable".into(), Value::Array(vars));
    }
    emit_ext(&mut u, &r.extensions, "url.", &[], cx.foreign);
    Value::Object(u)
}

fn export_body(b: &RestBody, cx: &Cx) -> Value {
    let mut m = Map::new();
    let language_of =
        |ct: &Option<String>| ct.as_deref().map(language_for).filter(|s| !s.is_empty());
    match &b.content {
        BodyContent::Raw { text, language } => {
            if b.extensions
                .get("mode")
                .and_then(Value::as_str)
                .is_some_and(|m| m != "raw")
            {
                // an unknown Postman mode (graphql, ...) kept as it was
            } else {
                m.insert("mode".into(), json!("raw"));
                m.insert("raw".into(), json!(text));
                let lang = language
                    .clone()
                    .or_else(|| language_of(&b.content_type).map(str::to_owned));
                if let Some(l) = lang {
                    m.insert("options".into(), json!({ "raw": { "language": l } }));
                }
            }
        }
        BodyContent::FormUrlEncoded { fields } => {
            m.insert("mode".into(), json!("urlencoded"));
            m.insert(
                "urlencoded".into(),
                Value::Array(fields.iter().map(|f| export_field(f, false)).collect()),
            );
        }
        BodyContent::Multipart { fields } => {
            m.insert("mode".into(), json!("formdata"));
            m.insert(
                "formdata".into(),
                Value::Array(fields.iter().map(|f| export_field(f, true)).collect()),
            );
        }
        BodyContent::Binary { file } => {
            m.insert("mode".into(), json!("file"));
            m.insert("file".into(), json!({ "src": file }));
        }
    }
    for (k, v) in &b.extensions {
        // `options` may hold more than the raw language: merge instead of overwriting
        if k == "options"
            && let (Some(Value::Object(have)), Value::Object(extra)) = (m.get_mut("options"), v)
        {
            have.extend(extra.clone());
            continue;
        }
        if cx.foreign || k.starts_with("x-") {
            m.insert(k.clone(), v.clone());
        }
    }
    Value::Object(m)
}

fn export_field(f: &FormField, multipart: bool) -> Value {
    let mut m = Map::new();
    m.insert("key".into(), json!(f.pair.name));
    match f.kind {
        FieldKind::File => {
            m.insert("src".into(), json!(f.pair.value));
            m.insert("type".into(), json!("file"));
        }
        FieldKind::Text => {
            m.insert("value".into(), json!(f.pair.value));
            if multipart {
                m.insert("type".into(), json!("text"));
            }
        }
    }
    if let Some(c) = &f.content_type {
        m.insert("contentType".into(), json!(c));
    }
    if let Some(d) = &f.pair.description {
        m.insert("description".into(), json!(d));
    }
    if f.pair.disabled {
        m.insert("disabled".into(), json!(true));
    }
    for (k, v) in &f.pair.extensions {
        m.insert(k.clone(), v.clone());
    }
    Value::Object(m)
}

fn export_response(r: &RestResponse) -> Value {
    let mut m = Map::new();
    if !is_derived(&r.id) {
        m.insert("id".into(), json!(r.id));
    }
    m.insert("name".into(), json!(r.name));
    if let Some(o) = &r.original_request {
        m.insert("originalRequest".into(), o.clone());
    }
    if let Some(s) = &r.status_text {
        m.insert("status".into(), json!(s));
    }
    if !r.code.is_empty() {
        match r.code.parse::<u16>() {
            Ok(n) => m.insert("code".into(), json!(n)),
            Err(_) => m.insert("code".into(), json!(r.code)),
        };
    }
    if !r.headers.is_empty() {
        m.insert(
            "header".into(),
            Value::Array(r.headers.iter().map(export_param).collect()),
        );
    }
    if let Some(b) = &r.body {
        m.insert("body".into(), json!(b));
    }
    if let Some(l) = r.language.clone().or_else(|| {
        r.content_type
            .as_deref()
            .map(language_for)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    }) {
        m.insert("_postman_previewlanguage".into(), json!(l));
    }
    for (k, v) in &r.extensions {
        m.insert(k.clone(), v.clone());
    }
    Value::Object(m)
}

fn export_events(scripts: &[Script]) -> Value {
    Value::Array(
        scripts
            .iter()
            .map(|s| {
                let mut e = Map::new();
                e.insert(
                    "listen".into(),
                    json!(match &s.kind {
                        ScriptKind::PreRequest => "prerequest",
                        ScriptKind::Test => "test",
                        ScriptKind::Other(o) => o.as_str(),
                    }),
                );
                let mut script = Map::new();
                script.insert(
                    "type".into(),
                    json!(
                        s.language
                            .clone()
                            .unwrap_or_else(|| "text/javascript".to_owned())
                    ),
                );
                let lines: Vec<&str> = if s.source.is_empty() {
                    Vec::new()
                } else {
                    s.source.split('\n').collect()
                };
                script.insert("exec".into(), json!(lines));
                emit_ext(&mut script, &s.extensions, "script.", &[], true);
                e.insert("script".into(), Value::Object(script));
                if s.disabled {
                    e.insert("disabled".into(), json!(true));
                }
                emit_ext(&mut e, &s.extensions, "", &["script."], true);
                Value::Object(e)
            })
            .collect(),
    )
}

fn export_variable(v: &Variable) -> Value {
    let mut m = Map::new();
    m.insert("key".into(), json!(v.key));
    m.insert("value".into(), json!(v.value));
    if let Some(t) = &v.kind {
        m.insert("type".into(), json!(t));
    }
    if let Some(d) = &v.description {
        m.insert("description".into(), json!(d));
    }
    if v.disabled {
        m.insert("disabled".into(), json!(true));
    }
    for (k, val) in &v.extensions {
        m.insert(k.clone(), val.clone());
    }
    Value::Object(m)
}

fn attr_list(pairs: &[(&str, &str)]) -> Value {
    Value::Array(
        pairs
            .iter()
            .map(|(k, v)| json!({ "key": k, "value": v, "type": "string" }))
            .collect(),
    )
}

fn export_auth(a: &Auth, cx: &mut Cx) -> Option<Value> {
    Some(match a {
        Auth::NoAuth => json!({ "type": "noauth" }),
        Auth::Basic { username, password } => json!({
            "type": "basic",
            "basic": attr_list(&[("username", username), ("password", password)]),
        }),
        Auth::Bearer { token } => json!({
            "type": "bearer",
            "bearer": attr_list(&[("token", token)]),
        }),
        Auth::ApiKey {
            key,
            value,
            location,
        } => {
            let loc = match location {
                ApiKeyLocation::Query => "query",
                _ => "header",
            };
            if *location == ApiKeyLocation::Cookie {
                cx.warnings
                    .push(format!("API key `{key}` in a cookie exported as a header"));
            }
            json!({
                "type": "apikey",
                "apikey": attr_list(&[("key", key), ("value", value), ("in", loc)]),
            })
        }
        Auth::Reference { scheme, .. } => {
            let found = cx
                .project
                .security_schemes
                .iter()
                .find(|s| &s.name == scheme);
            match found {
                Some(s) => {
                    let auth = s.auth.clone();
                    if matches!(auth, Auth::Reference { .. }) {
                        return None;
                    }
                    return export_auth(&auth, cx);
                }
                None => {
                    cx.warnings.push(format!(
                        "security scheme `{scheme}` is not defined; auth dropped"
                    ));
                    return None;
                }
            }
        }
        Auth::Other { kind, params } => match kind.strip_prefix("postman:") {
            Some(t) => {
                let mut m = Map::new();
                m.insert("type".into(), json!(t));
                if let Some(list) = params.get("attrs") {
                    m.insert(t.to_owned(), list.clone());
                }
                Value::Object(m)
            }
            None => {
                cx.warnings.push(format!(
                    "auth of type `{kind}` has no Postman equivalent; dropped"
                ));
                return None;
            }
        },
    })
}

/// Preview language for a media type.
pub(crate) fn language_for(content_type: &str) -> &'static str {
    let ct = content_type.to_ascii_lowercase();
    if ct.contains("json") {
        "json"
    } else if ct.contains("xml") {
        "xml"
    } else if ct.contains("html") {
        "html"
    } else if ct.starts_with("text/") {
        "text"
    } else {
        ""
    }
}
