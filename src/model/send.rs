//! Turns a stored project request into something the client can send.

use crate::client::{Body, Field, Header, HttpRequest, Method, Part, Request};

use super::common::Server;
use super::project::{ApiRequest, Project, Request as ProjectRequest};
use super::rest::{BodyContent, FieldKind, ParamLocation};

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
        method: rest.method.as_str().parse::<Method>().unwrap_or_default(),
        url,
        headers,
        body,
    })
}
