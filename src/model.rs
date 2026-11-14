#![allow(clippy::field_reassign_with_default)]

//! Format-neutral data model for API projects.
//!
//! A [`Project`] holds [`Directory`]s (nestable) and [`Request`]s. Each request carries
//! an [`ApiRequest`] that says which kind of API it talks to; only REST exists today.
//! Projects import from Postman Collection v2.1, OpenAPI 3.0.x/3.1.x and Swagger 2.0,
//! and export to Postman v2.1 and OpenAPI 3.x.
//!
//! ```
//! use hozd::model::{Method, Project, Request, RestRequest};
//!
//! let mut project = Project::new("Demo");
//! project
//!     .requests
//!     .push(Request::rest("Ping", RestRequest::new(Method::Get, "/ping")));
//! assert_eq!(project.all_requests().len(), 1);
//! ```
//!
//! # Conventions
//!
//! * `auth: None` inherits from the parent; [`Auth::NoAuth`] disables authentication.
//! * Anything the model does not cover lands in `extensions` and is written back on
//!   export. Ids from the source are kept; missing ones are derived deterministically
//!   (`gen-…`) and not written back, since importing again derives the same value.
//!   Natively created entities get random UUID-shaped ids ([`new_id`]).
//! * Extensions of a format are written back only to the same format family; `x-`
//!   keys are written everywhere.
//! * Custom HTTP methods are kept ([`Method::Custom`]); OpenAPI export skips them with
//!   a warning. Postman scripts and settings are exported to OpenAPI as `x-hozd-*`
//!   extensions. OpenAPI schemas are kept as `serde_json::Value`.
//!
//! # Adding an API kind
//!
//! 1. Create `model/<kind>.rs` with the kind's request type (for example
//!    `GraphqlRequest`) and its supporting types. Derive `Serialize`, `Deserialize`,
//!    `Clone`, `Debug`, `PartialEq`, and give them an `extensions` field if the source
//!    formats carry extra keys.
//! 2. Add a variant to [`ApiKind`] and a matching one to [`ApiRequest`]
//!    (`Graphql(GraphqlRequest)`) in `model/project.rs`, and extend `kind()`.
//! 3. Fix the exhaustive `match`es the compiler points at: the converters decide what
//!    a request of the new kind becomes in each format (skip it with a warning if the
//!    format has no place for it).
//! 4. Re-export the new types below. Existing variants and types do not change.
//!
//! Both enums are `#[non_exhaustive]`, so code outside this crate already handles the
//! unknown case.

mod common;
mod openapi;
mod postman;
mod project;
mod rest;
mod send;
mod util;

pub use common::{
    ApiKeyLocation, Auth, ConvertError, DERIVED_PREFIX, Exported, Extensions, Id, NameValue,
    Script, ScriptKind, SecurityScheme, Server, Source, SourceFormat, Variable, derive_id,
    is_derived, new_id,
};
pub use openapi::{OpenApiVersion, export_openapi, import_openapi};
pub use postman::{export_postman, import_postman};
pub use project::{ApiKind, ApiRequest, Directory, Project, Request};
pub use rest::{
    BodyContent, FieldKind, FormField, Method, OperationMeta, ParamLocation, RestBody, RestParam,
    RestRequest, RestResponse, RestSettings,
};
pub use send::{from_client_request, to_client_request};

/// Imports a Postman v2.1, OpenAPI 3.x or Swagger 2.0 document, detecting which.
pub fn import(doc: &serde_json::Value) -> Result<Project, ConvertError> {
    if doc.get("openapi").is_some() || doc.get("swagger").is_some() {
        import_openapi(doc)
    } else if doc.pointer("/info/schema").is_some() {
        import_postman(doc)
    } else {
        Err(ConvertError::Unsupported(
            "not a Postman collection, OpenAPI or Swagger document".to_owned(),
        ))
    }
}
