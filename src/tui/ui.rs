//! Rendering. `layout` decides where things go, `components` draws them.

use ratatui::Frame;

use crate::client::Kind;

use super::app::App;

mod components;
mod layout;

pub use components::{
    HELP_GRAPHQL, HELP_GRPC, HELP_REST, HELP_WS, REPO_URL, VERSION, contribute_area,
    status_text_width, suggestions_area,
};
pub use layout::*;
pub(crate) use layout::{center_areas, main_area};

use components::*;

/// Draws the whole screen: panels first, then the popups over them.
pub fn draw(frame: &mut Frame, app: &App) {
    let (main, status) = main_area(frame.area());
    let [left, center, right] = panel_areas(main);

    let view = app.view();
    draw_left(frame, app, left);
    draw_center(frame, app, &view.request, view.previewing, center);
    draw_right(frame, app, view.outcome, right);
    draw_status(frame, app, status);
    if !app.suggestions.items.is_empty() {
        let [_, center, _] = panel_areas(main);
        draw_suggestions(frame, app, center_areas(center, Kind::Grpc).schema);
    }
    if app.picker.is_some() {
        draw_picker(frame, app, frame.area());
    }
    if app.help.is_some() {
        draw_help(frame, app, frame.area());
    }
    if let Some(modal) = &app.modal {
        draw_modal(frame, modal, frame.area());
    }
}
