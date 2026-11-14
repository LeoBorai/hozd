//! Rendering. `layout` decides where things go, `components` draws them.

use ratatui::Frame;

use super::app::{App, CenterTab};

mod components;
mod layout;

pub use components::{
    AuthLayout, HELP_GRAPHQL, HELP_GRPC, HELP_REST, HELP_WS, REPO_URL, VERSION, auth_layout,
    contribute_area, status_text_width, suggestions_anchor, suggestions_area, token_index,
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
    match app.center_tab() {
        None => draw_center(frame, app, &view.request, view.previewing, center),
        Some(tab) => {
            let (tabs, rest) = center_split(center);
            draw_center_tabs(frame, app, tabs);
            match tab {
                CenterTab::Request => draw_center(frame, app, &view.request, view.previewing, rest),
                CenterTab::Metadata => draw_metadata(frame, app, rest),
            }
        }
    }
    draw_right(frame, app, view.outcome, right);
    draw_status(frame, app, status);
    if !app.suggestions.items.is_empty() {
        let [_, center, _] = panel_areas(main);
        let center = if app.center_tab().is_some() {
            center_split(center).1
        } else {
            center
        };
        let areas = center_areas(center, app.kind);
        draw_suggestions(
            frame,
            app,
            suggestions_anchor(app, areas.schema, areas.body),
        );
    }
    if app.picker.is_some() {
        draw_picker(frame, app, frame.area());
    }
    if let Some(confirm) = &app.confirm {
        draw_confirm(frame, confirm, frame.area());
    }
    if app.help.is_some() {
        draw_help(frame, app, frame.area());
    }
    if let Some(modal) = &app.modal {
        draw_modal(frame, modal, frame.area());
    }
}
