//! Turns a stored project request into something the client can send.

use crate::client::{
    Body, Field, Header, HttpRequest, Method as ClientMethod, Part, PartValue, Request, split_query,
};

use super::common::{NameValue, Server};
use super::project::{ApiRequest, Project, Request as ProjectRequest};
use super::rest::{
    BodyContent, FieldKind, FormField, Method, ParamLocation, RestParam, RestRequest,
};

/// The HTTP request `request` describes, with the project's first server as the base URL
/// when the request names none. Disabled parameters and fields are left out.
pub fn to_client_request(project: &Project, request: &ProjectRequest) -> Request {
    let ApiRequest::Rest(rest) = &request.api;

    let base = rest
        .server
        .clone()
        .or_else(|| project.servers.first().map(|s: &Server| s.url.clone()))
        .unwrap_or_default();
    let mut path = rest.path.clone();
    let mut query = Vec::new();
    let mut headers = Vec::new();
    for p in rest.params.iter().filter(|p| !p.pair.disabled) {
        match p.location {
            ParamLocation::Path => {
                path = path.replace(&format!("{{{}}}", p.pair.name), &p.pair.value);
            }
            ParamLocation::Query => query.push(format!("{}={}", p.pair.name, p.pair.value)),
            ParamLocation::Header => headers.push(Header::new(&p.pair.name, &p.pair.value)),
            ParamLocation::Cookie => {}
        }
    }
    let mut url = format!("{}{}", base.trim_end_matches('/'), path);
    if !query.is_empty() {
        url.push('?');
        url.push_str(&query.join("&"));
    }

    let body = rest.body.as_ref().and_then(|b| {
        let json = b
            .content_type
            .as_deref()
            .is_some_and(|t| t.contains("json"));
        match &b.content {
            BodyContent::Raw { text, language } => {
                if text.is_empty() {
                    None
                } else if json || language.as_deref() == Some("json") {
                    Some(Body::JsonRaw(text.clone()))
                } else {
                    Some(Body::Text(text.clone()))
                }
            }
            BodyContent::FormUrlEncoded { fields } => Some(Body::Form(
                fields
                    .iter()
                    .filter(|f| !f.pair.disabled)
                    .map(|f| Field::new(&f.pair.name, &f.pair.value))
                    .collect(),
            )),
            BodyContent::Multipart { fields } => Some(Body::Multipart(
                fields
                    .iter()
                    .filter(|f| !f.pair.disabled && f.kind == FieldKind::Text)
                    .map(|f| Part::text(&f.pair.name, &f.pair.value))
                    .collect(),
            )),
            BodyContent::Binary { file } => file.clone().map(Body::File),
        }
    });

    Request::Http(HttpRequest {
        method: rest
            .method
            .as_str()
            .parse::<ClientMethod>()
            .unwrap_or_default(),
        url,
        headers,
        body,
    })
}

/// The stored request `existing` with the edits in `edited` applied: the inverse of
/// [`to_client_request`]. Parts of the stored request the client has no notion of
/// (descriptions, schemas, scripts, responses, disabled parameters) are kept.
pub fn from_client_request(existing: &RestRequest, edited: &HttpRequest) -> RestRequest {
    let mut rest = existing.clone();
    rest.method = Method::parse(edited.method.as_str());

    let (url, query) = split_query(&edited.url);
    let (server, path) = split_server(&url);
    rest.server = server;
    rest.path = path;

    // Rebuilds the enabled parameters of `location` from `pairs`, keeping what the stored
    // parameter of the same name carries beyond its value.
    let rebuild =
        |rest: &mut RestRequest, location: ParamLocation, pairs: Vec<(String, String)>| {
            let old = std::mem::take(&mut rest.params);
            let (mut same, other): (Vec<_>, Vec<_>) =
                old.into_iter().partition(|p| p.location == location);
            let disabled: Vec<_> = same.iter().filter(|p| p.pair.disabled).cloned().collect();
            same.retain(|p| !p.pair.disabled);
            let mut params = Vec::new();
            for (name, value) in pairs {
                let mut param = match same.iter().position(|p| p.pair.name == name) {
                    Some(i) => same.remove(i),
                    None => RestParam::new(location, &name, ""),
                };
                param.pair.value = value;
                params.push(param);
            }
            // keep the original order: the other locations first, as stored
            rest.params = other;
            rest.params.extend(params);
            rest.params.extend(disabled);
        };
    rebuild(&mut rest, ParamLocation::Query, query);
    let headers = edited
        .headers
        .iter()
        .map(|h| (h.name.clone(), h.value.clone()))
        .collect();
    rebuild(&mut rest, ParamLocation::Header, headers);

    rest.body = match &edited.body {
        None => None,
        Some(body) => {
            let mut stored = rest.body.take().unwrap_or_default();
            let language = match &stored.content {
                BodyContent::Raw { language, .. } => language.clone(),
                _ => None,
            };
            let text_field = |name: &str, value: &str, kind| FormField {
                pair: NameValue::new(name, value),
                kind,
                content_type: None,
            };
            stored.content = match body {
                Body::Text(t) => BodyContent::Raw {
                    text: t.clone(),
                    language: language.filter(|l| l != "json"),
                },
                Body::JsonRaw(t) => BodyContent::Raw {
                    text: t.clone(),
                    language: Some("json".into()),
                },
                Body::Json(v) => BodyContent::Raw {
                    text: v.to_json_pretty(),
                    language: Some("json".into()),
                },
                Body::Form(fields) => BodyContent::FormUrlEncoded {
                    fields: fields
                        .iter()
                        .map(|f| text_field(&f.name, &f.value, FieldKind::Text))
                        .collect(),
                },
                Body::Multipart(parts) => BodyContent::Multipart {
                    fields: parts
                        .iter()
                        .map(|p| match &p.value {
                            PartValue::Text(t) => text_field(&p.name, t, FieldKind::Text),
                            PartValue::File(f) => text_field(&p.name, f, FieldKind::File),
                        })
                        .collect(),
                },
                Body::File(path) => BodyContent::Binary {
                    file: Some(path.clone()),
                },
            };
            Some(stored)
        }
    };
    rest
}

/// `http://host:80/a/b` -> (`http://host:80`, `/a/b`); a URL starting with `/` has no
/// server; `{{baseUrl}}/a` -> (`{{baseUrl}}`, `/a`).
fn split_server(url: &str) -> (Option<String>, String) {
    if url.starts_with('/') {
        return (None, url.to_owned());
    }
    let after_scheme = url.find("://").map_or(0, |i| i + 3);
    match url[after_scheme..].find('/') {
        Some(i) => (
            Some(url[..after_scheme + i].to_owned()),
            url[after_scheme + i..].to_owned(),
        ),
        None => (
            Some(url.to_owned()).filter(|u| !u.is_empty()),
            String::new(),
        ),
    }
}
