use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Margin, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, List, ListItem, ListState, Paragraph, Tabs};
use ratatui_textarea::TextArea;

use super::app::{App, Focus, Tab};
use super::json::highlight_line;
use crate::client::Method;
use crate::history::Outcome;

/// Left, center and right panels on a 12-column grid: 3-6-3.
pub fn panel_areas(area: Rect) -> [Rect; 3] {
    Layout::horizontal([
        Constraint::Ratio(3, 12),
        Constraint::Ratio(6, 12),
        Constraint::Ratio(3, 12),
    ])
    .areas(area)
}

/// Content area inside a bordered block.
pub fn inner(area: Rect) -> Rect {
    area.inner(Margin::new(1, 1))
}

/// Every clickable region, as drawn. `draw` and mouse hit-testing both use this so they
/// can never disagree. Rects marked "outer" include the border.
#[derive(Debug, Clone, Copy)]
pub struct Areas {
    pub tabs: Rect,
    pub list: Rect,
    pub method: Rect,
    pub url: Rect,
    pub body: Rect,
    pub response: Rect,
    pub response_body: Rect,
}

/// Splits the left panel's outer rect into its tab row and list.
fn left_areas(left: Rect) -> (Rect, Rect) {
    let [tabs, list] =
        Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).areas(inner(left));
    (tabs, list)
}

/// Outer rects of the method box, URL box and body box.
fn center_areas(center: Rect) -> (Rect, Rect, Rect) {
    let [top, body] = Layout::vertical([Constraint::Length(3), Constraint::Min(3)]).areas(center);
    let [method, url] =
        Layout::horizontal([Constraint::Length(12), Constraint::Min(10)]).areas(top);
    (method, url, body)
}

/// Outer rects of the status, headers and body boxes.
fn right_areas(right: Rect) -> (Rect, Rect, Rect) {
    Layout::vertical([
        Constraint::Length(3),
        Constraint::Percentage(30),
        Constraint::Min(3),
    ])
    .areas::<3>(right)
    .into()
}

fn main_area(area: Rect) -> (Rect, Rect) {
    let [main, status] = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(area);
    (main, status)
}

pub fn areas(area: Rect) -> Areas {
    let (main, _) = main_area(area);
    let [left, center, right] = panel_areas(main);
    let (tabs, list) = left_areas(left);
    let (method, url, body) = center_areas(center);
    let (response, _, response_body) = right_areas(right);
    Areas {
        tabs,
        list,
        method,
        url,
        body,
        response,
        response_body,
    }
}

const STORED_TAB_WIDTH: u16 = " Stored ".len() as u16;

/// Which tab title sits under column `x` of the tab row. Matches `Tabs` default padding
/// (one space each side of a title) and the one-column `|` divider.
pub fn tab_at(tabs: Rect, x: u16) -> Option<Tab> {
    let offset = x.checked_sub(tabs.x)?;
    let history_width = " History ".len() as u16;
    match offset {
        o if o < STORED_TAB_WIDTH => Some(Tab::Stored),
        o if o == STORED_TAB_WIDTH => None, // divider
        o if o <= STORED_TAB_WIDTH + history_width => Some(Tab::History),
        _ => None,
    }
}

/// First visible list row when `selected_row` must stay in view: the same rule `List`
/// applies to a fresh state, set explicitly so clicks map to the rows that were drawn.
pub fn list_offset(selected_row: Option<usize>, height: u16) -> usize {
    selected_row.map_or(0, |row| (row + 1).saturating_sub(height as usize))
}

/// For each row of the Stored list, the template it shows (`None` for project headers).
pub fn stored_row_map(app: &App) -> Vec<Option<usize>> {
    let mut rows = Vec::new();
    let mut last_project: Option<&str> = None;
    for (i, t) in app.templates.iter().enumerate() {
        if last_project != Some(t.project.as_str()) {
            rows.push(None);
            last_project = Some(&t.project);
        }
        rows.push(Some(i));
    }
    rows
}

/// Scroll offsets (rows, columns) that keep the cursor inside `area`.
pub fn editor_scroll(cursor: (usize, usize), area: Rect) -> (usize, usize) {
    (
        cursor
            .0
            .saturating_sub((area.height as usize).saturating_sub(1)),
        cursor
            .1
            .saturating_sub((area.width as usize).saturating_sub(1)),
    )
}

pub fn draw(frame: &mut Frame, app: &App) {
    let (main, status) = main_area(frame.area());
    let [left, center, right] = panel_areas(main);

    let view = app.view();
    draw_left(frame, app, left);
    draw_center(frame, app, &view.request, view.previewing, center);
    draw_right(frame, app, view.outcome, right);
    draw_status(frame, app, status);
}

fn block(title: &str, focused: bool) -> Block<'static> {
    let color = if focused {
        Color::Green
    } else {
        Color::DarkGray
    };
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(color))
        .title(format!(" {title} "))
}

// ---- left ----------------------------------------------------------------

fn draw_left(frame: &mut Frame, app: &App, area: Rect) {
    let focused = app.focus == Focus::List;
    let outer = block("Requests", focused);
    frame.render_widget(outer, area);

    let (tabs, list) = left_areas(area);
    let selected = match app.tab {
        Tab::Stored => 0,
        Tab::History => 1,
    };
    frame.render_widget(
        Tabs::new(["Stored", "History"])
            .select(selected)
            .highlight_style(
                Style::new()
                    .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
                    .fg(Color::Yellow),
            )
            .divider("|"),
        tabs,
    );

    let (items, selected_row) = match app.tab {
        Tab::Stored => stored_rows(app),
        Tab::History => history_rows(app),
    };
    let mut state = ListState::default()
        .with_selected(selected_row)
        .with_offset(list_offset(selected_row, list.height));
    let highlight = if focused {
        Style::new()
            .bg(Color::DarkGray)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::new().add_modifier(Modifier::BOLD)
    };
    frame.render_stateful_widget(
        List::new(items).highlight_style(highlight),
        list,
        &mut state,
    );
}

fn stored_rows(app: &App) -> (Vec<ListItem<'static>>, Option<usize>) {
    if app.templates.is_empty() {
        return (vec![dim("no templates in ~/.hozd/projects")], None);
    }
    let mut rows = Vec::new();
    let mut selected = None;
    let mut last_project: Option<&str> = None;
    for (i, t) in app.templates.iter().enumerate() {
        if last_project != Some(t.project.as_str()) {
            rows.push(ListItem::new(Line::styled(
                format!("{}/", t.project),
                Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD),
            )));
            last_project = Some(&t.project);
        }
        if i == app.stored_sel {
            selected = Some(rows.len());
        }
        rows.push(ListItem::new(Line::from(vec![
            Span::raw("  "),
            method_span(t.template.method),
            Span::raw(format!(" {}", t.template.name)),
        ])));
    }
    (rows, selected)
}

fn history_rows(app: &App) -> (Vec<ListItem<'static>>, Option<usize>) {
    if app.history.is_empty() {
        return (vec![dim("no requests sent yet")], None);
    }
    let rows = app
        .history
        .iter()
        .map(|s| {
            let status = match s.status {
                Some(code) => Span::styled(code.to_string(), status_style(code)),
                None => Span::styled("ERR", Style::new().fg(Color::Red)),
            };
            ListItem::new(Line::from(vec![
                status,
                Span::raw(" "),
                method_span(s.method),
                Span::raw(format!(" {}", s.url)),
            ]))
        })
        .collect();
    (rows, Some(app.history_sel))
}

fn dim(text: &str) -> ListItem<'static> {
    ListItem::new(Line::styled(
        text.to_owned(),
        Style::new().fg(Color::DarkGray),
    ))
}

fn method_span(method: Method) -> Span<'static> {
    let color = match method {
        Method::Get => Color::Green,
        Method::Post => Color::Yellow,
        Method::Put | Method::Patch => Color::Blue,
        Method::Delete => Color::Red,
        Method::Head | Method::Options => Color::Magenta,
    };
    Span::styled(format!("{:<6}", method.as_str()), Style::new().fg(color))
}

fn status_style(code: u16) -> Style {
    Style::new().fg(match code {
        200..=299 => Color::Green,
        300..=399 => Color::Cyan,
        400..=499 => Color::Yellow,
        _ => Color::Red,
    })
}

// ---- center --------------------------------------------------------------

fn draw_center(
    frame: &mut Frame,
    app: &App,
    request: &super::app::RequestView,
    previewing: bool,
    area: Rect,
) {
    let (method_area, url_area, body_area) = center_areas(area);

    let method_block = block("Method", false);
    let method_inner = method_block.inner(method_area);
    frame.render_widget(method_block, method_area);
    frame.render_widget(Paragraph::new(method_span(request.method)), method_inner);

    let url_title = if previewing { "URL (preview)" } else { "URL" };
    let url_block = block(url_title, app.focus == Focus::Url);
    let url_inner = url_block.inner(url_area);
    frame.render_widget(url_block, url_area);
    if previewing {
        frame.render_widget(Paragraph::new(request.url.clone()), url_inner);
    } else {
        render_editor(frame, &app.url, url_inner, app.focus == Focus::Url, false);
    }

    let headers = if request.headers.is_empty() {
        String::new()
    } else {
        format!(" - {} header(s)", request.headers.len())
    };
    let body_block = block(&format!("Body (JSON){headers}"), app.focus == Focus::Body);
    let body_inner = body_block.inner(body_area);
    frame.render_widget(body_block, body_area);
    if previewing {
        let lines: Vec<Line> = request.body.lines().map(highlight_line).collect();
        frame.render_widget(Paragraph::new(lines), body_inner);
    } else {
        render_editor(frame, &app.body, body_inner, app.focus == Focus::Body, true);
    }
}

/// Draws a `TextArea`'s lines ourselves (it has no syntax-highlight hook) and places
/// the terminal cursor when the editor has focus.
fn render_editor(frame: &mut Frame, ta: &TextArea<'static>, area: Rect, focused: bool, json: bool) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let cursor = ta.cursor();
    let (row, col) = (cursor.0, cursor.1);
    let (row_scroll, col_scroll) = editor_scroll((row, col), area);

    let lines: Vec<Line> = ta
        .lines()
        .iter()
        .map(|l| {
            if json {
                highlight_line(l)
            } else {
                Line::raw(l.clone())
            }
        })
        .collect();
    frame.render_widget(
        Paragraph::new(lines).scroll((row_scroll as u16, col_scroll as u16)),
        area,
    );
    if focused {
        frame.set_cursor_position(Position::new(
            area.x + (col - col_scroll) as u16,
            area.y + (row - row_scroll) as u16,
        ));
    }
}

// ---- right ---------------------------------------------------------------

fn draw_right(frame: &mut Frame, app: &App, outcome: Option<&Outcome>, area: Rect) {
    let (status_area, headers_area, body_area) = right_areas(area);

    let status_block = block("Response", false);
    let status_inner = status_block.inner(status_area);
    frame.render_widget(status_block, status_area);
    let status_line = match outcome {
        _ if app.sending => Line::styled("sending...", Style::new().fg(Color::Yellow)),
        Some(Outcome::Response(r)) => Line::from(vec![
            Span::styled(
                r.status.to_string(),
                status_style(r.status).add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!("  {} ms", r.elapsed.as_millis())),
        ]),
        Some(Outcome::Error(_)) => Line::styled("failed", Style::new().fg(Color::Red)),
        None => Line::styled("no response yet", Style::new().fg(Color::DarkGray)),
    };
    frame.render_widget(Paragraph::new(status_line), status_inner);

    let headers_block = block("Headers", false);
    let headers_inner = headers_block.inner(headers_area);
    frame.render_widget(headers_block, headers_area);
    let header_lines: Vec<Line> = match outcome {
        Some(Outcome::Response(r)) => r
            .headers
            .iter()
            .map(|h| {
                Line::from(vec![
                    Span::styled(format!("{}: ", h.name), Style::new().fg(Color::Cyan)),
                    Span::raw(h.value.clone()),
                ])
            })
            .collect(),
        _ => Vec::new(),
    };
    frame.render_widget(Paragraph::new(header_lines), headers_inner);

    let body_block = block("Body (PgUp/PgDn)", false);
    let body_inner = body_block.inner(body_area);
    frame.render_widget(body_block, body_area);
    let body_lines: Vec<Line> = match outcome {
        Some(Outcome::Response(r)) => pretty(&r.body).lines().map(highlight_line).collect(),
        Some(Outcome::Error(msg)) => vec![Line::styled(msg.clone(), Style::new().fg(Color::Red))],
        None => Vec::new(),
    };
    frame.render_widget(
        Paragraph::new(body_lines)
            .wrap(ratatui::widgets::Wrap { trim: false })
            .scroll((app.scroll, 0)),
        body_inner,
    );
}

/// Pretty-prints JSON bodies; anything else is shown verbatim.
fn pretty(body: &str) -> String {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| serde_json::to_string_pretty(&v).ok())
        .unwrap_or_else(|| body.to_owned())
}

// ---- status bar ------------------------------------------------------------

fn draw_status(frame: &mut Frame, app: &App, area: Rect) {
    let text = match &app.status {
        Some(message) => Line::styled(message.clone(), Style::new().fg(Color::Red)),
        None => Line::styled(
            "Tab focus | [ ] tabs | j/k move | Enter load | Ctrl+S send | Ctrl+N/P method | PgUp/PgDn scroll | q quit",
            Style::new().fg(Color::DarkGray),
        ),
    };
    frame.render_widget(Paragraph::new(text), area);
}
