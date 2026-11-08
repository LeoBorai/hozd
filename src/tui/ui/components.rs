//! Atomic TUI components: each draws one box or popup from the app state.

mod address;
mod auth;
mod center;
mod editor;
mod frame;
mod help;
mod log;
mod modal;
mod picker;
mod request_box;
mod requests;
mod response;
mod schema;
mod spinner;
mod status;
mod suggestions;

pub(crate) use address::*;
pub(crate) use auth::*;
pub(crate) use center::*;
pub(crate) use editor::*;
pub(crate) use frame::*;
pub(crate) use help::*;
pub(crate) use log::*;
pub(crate) use modal::*;
pub(crate) use picker::*;
pub(crate) use request_box::*;
pub(crate) use requests::*;
pub(crate) use response::*;
pub(crate) use schema::*;
pub(crate) use spinner::*;
pub(crate) use status::*;
pub(crate) use suggestions::*;

pub use auth::{AuthLayout, auth_layout, token_index};
pub use status::{
    HELP_GRAPHQL, HELP_GRPC, HELP_REST, HELP_WS, REPO_URL, VERSION, contribute_area,
    status_text_width,
};
pub use suggestions::{suggestions_anchor, suggestions_area};
