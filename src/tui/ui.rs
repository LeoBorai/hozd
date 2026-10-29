use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Margin, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, BorderType, Borders, Clear, List, ListItem, ListState, Paragraph, Tabs,
};
use ratatui_textarea::TextArea;

use super::app::{App, Focus, RequestTab, SchemaMode, Tab};
use super::headers::{self, Col};
use super::json::{highlight_line, pretty};
use super::log::message_color;
use crate::client::{Kind, Method, SchemaSource, status_name};
use crate::history::Outcome;
use crate::project::TemplateKind;

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
    /// The log box (outer).
    pub logs: Rect,
    pub method: Rect,
    pub url: Rect,
    /// gRPC mode only.
    pub schema: Rect,
    /// The method picker box; gRPC mode only.
    pub rpc: Rect,
    pub body: Rect,
    pub response: Rect,
    pub response_body: Rect,
}

/// Splits the left column 50/50: the request list on top, the log below. Both are outer
/// rects.
fn left_split(left: Rect) -> (Rect, Rect) {
    let [requests, log] =
        Layout::vertical([Constraint::Ratio(1, 2), Constraint::Ratio(1, 2)]).areas(left);
    (requests, log)
}

/// Splits the request list's outer rect into its tab row and list.
fn left_areas(requests: Rect) -> (Rect, Rect) {
    let [tabs, list] =
        Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).areas(inner(requests));
    (tabs, list)
}

/// Outer rects of the center panel's boxes. `schema` and `rpc` only exist in gRPC mode
/// and are empty otherwise; `method` holds the TLS state there.
struct Center {
    method: Rect,
    url: Rect,
    schema: Rect,
    rpc: Rect,
    body: Rect,
}

fn center_areas(center: Rect, kind: Kind) -> Center {
    let top_row = |top: Rect| {
        Layout::horizontal([Constraint::Length(12), Constraint::Min(10)]).areas::<2>(top)
    };
    match kind {
        Kind::Http => {
            let [top, body] =
                Layout::vertical([Constraint::Length(3), Constraint::Min(3)]).areas(center);
            let [method, url] = top_row(top);
            Center {
                method,
                url,
                schema: Rect::ZERO,
                rpc: Rect::ZERO,
                body,
            }
        }
        Kind::Grpc => {
            let [top, schema, rpc, body] = Layout::vertical([
                Constraint::Length(3),
                Constraint::Length(3),
                Constraint::Length(3),
                Constraint::Min(3),
            ])
            .areas(center);
            let [method, url] = top_row(top);
            Center {
                method,
                url,
                schema,
                rpc,
                body,
            }
        }
    }
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

/// The big reading panel: the whole main area with a small margin.
pub fn modal_area(area: Rect) -> Rect {
    let (main, _) = main_area(area);
    main.inner(Margin::new(2, 1))
}

/// The regions of the REST layout.
/// The method picker: the center panel with a one-cell margin.
pub fn picker_area(area: Rect) -> Rect {
    let (main, _) = main_area(area);
    let [_, center, _] = panel_areas(main);
    center.inner(Margin::new(1, 1))
}

/// The rows of the picker where methods are listed: below the filter line.
pub fn picker_list_area(picker: Rect) -> Rect {
    let [_, list] =
        Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).areas(inner(picker));
    list
}

/// What each row of the picker shows: `None` for a service header, else the index of the
/// method in [`App::picker_matches`].
pub fn picker_row_map(app: &App) -> Vec<Option<usize>> {
    let mut rows = Vec::new();
    let mut last_service: Option<&str> = None;
    for (i, m) in app.picker_matches().into_iter().enumerate() {
        if last_service != Some(m.service.as_str()) {
            rows.push(None);
            last_service = Some(&m.service);
        }
        rows.push(Some(i));
    }
    rows
}

pub fn areas(area: Rect) -> Areas {
    areas_for(area, Kind::Http)
}

/// The regions of the layout the center panel has for `kind`.
pub fn areas_for(area: Rect, kind: Kind) -> Areas {
    let (main, _) = main_area(area);
    let [left, center, right] = panel_areas(main);
    let (requests, logs) = left_split(left);
    let (tabs, list) = left_areas(requests);
    let center = center_areas(center, kind);
    let (response, _, response_body) = right_areas(right);
    Areas {
        tabs,
        list,
        logs,
        method: center.method,
        url: center.url,
        schema: center.schema,
        rpc: center.rpc,
        body: center.body,
        response,
        response_body,
    }
}

/// The kind selector in the top border of the URL box, right-aligned.
const KIND_LABEL: &str = " REST | gRPC ";

/// Which kind label sits under `at`: the selector is drawn in the top border of the URL
/// box (`url` is its outer rect), right-aligned and one column short of the corner.
pub fn kind_at(url: Rect, at: Position) -> Option<Kind> {
    if at.y != url.y {
        return None;
    }
    let width = KIND_LABEL.len() as u16;
    let start = url.right().checked_sub(1 + width)?;
    let offset = at.x.checked_sub(start)?;
    match offset {
        o if o < 7 => Some(Kind::Http),
        o if o < width => Some(Kind::Grpc),
        _ => None,
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

/// Titles of the request's lower box: `" Body "` and `" Headers (n) "` (`" Metadata (n) "`
/// for gRPC), joined by `|`.
fn request_tab_labels(count: usize, kind: Kind) -> (String, String) {
    let name = match kind {
        Kind::Http => "Headers",
        Kind::Grpc => "Metadata",
    };
    let second = if count == 0 {
        format!(" {name} ")
    } else {
        format!(" {name} ({count}) ")
    };
    (" Body ".to_owned(), second)
}

/// Which request tab title is under `at`; titles sit on the box's top border, starting
/// one column in. `header_count` must be the count shown in the title.
pub fn request_tab_at(
    body: Rect,
    at: Position,
    header_count: usize,
    kind: Kind,
) -> Option<RequestTab> {
    if at.y != body.y {
        return None;
    }
    let offset = at.x.checked_sub(body.x + 1)? as usize;
    let (body_label, headers_label) = request_tab_labels(header_count, kind);
    let divider = body_label.len();
    match offset {
        o if o < divider => Some(RequestTab::Body),
        o if o == divider => None,
        o if o <= divider + headers_label.len() => Some(RequestTab::Headers),
        _ => None,
    }
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

/// The inputs of one header row.
#[derive(Debug, Clone, Copy)]
pub struct HeaderCells {
    pub row: usize,
    pub name: Rect,
    pub value: Rect,
}

/// Where the header rows are drawn inside the request box.
pub struct HeaderGrid {
    pub rows: Vec<HeaderCells>,
    /// The "add header" line, when it is in view.
    pub add: Option<Rect>,
}

/// Lays out `count` header rows (name and value side by side, split by a `:`) followed by
/// an "add header" line, scrolled so that `selected` is in view.
pub fn header_grid(inner: Rect, count: usize, selected: usize) -> HeaderGrid {
    let height = inner.height as usize;
    let offset = list_offset(Some(selected), inner.height);
    let name_width = (inner.width * 2 / 5).clamp(1, inner.width.saturating_sub(4).max(1));
    let value_x = inner.x + name_width + 2;
    let value_width = inner.right().saturating_sub(value_x);
    let rows = (offset..count.min(offset + height))
        .map(|row| {
            let y = inner.y + (row - offset) as u16;
            HeaderCells {
                row,
                name: Rect::new(inner.x, y, name_width, 1),
                value: Rect::new(value_x, y, value_width, 1),
            }
        })
        .collect();
    let add = (count >= offset && count < offset + height)
        .then(|| Rect::new(inner.x, inner.y + (count - offset) as u16, inner.width, 1));
    HeaderGrid { rows, add }
}

pub fn draw(frame: &mut Frame, app: &App) {
    let (main, status) = main_area(frame.area());
    let [left, center, right] = panel_areas(main);

    let view = app.view();
    draw_left(frame, app, left);
    draw_center(frame, app, &view.request, view.previewing, center);
    draw_right(frame, app, view.outcome, right);
    draw_status(frame, app, status);
    if app.picker.is_some() {
        draw_picker(frame, app, frame.area());
    }
    if let Some(modal) = &app.modal {
        draw_modal(frame, modal, frame.area());
    }
}

// ---- method picker ------------------------------------------------------------

fn draw_picker(frame: &mut Frame, app: &App, screen: Rect) {
    let Some(picker) = &app.picker else {
        return;
    };
    let area = picker_area(screen);
    frame.render_widget(Clear, area);
    let title = Line::from(vec![
        Span::styled(" Pick a method ", Style::new().add_modifier(Modifier::BOLD)),
        Span::styled(
            "| type to filter | Up/Down | Enter | Esc ",
            Style::new().fg(Color::DarkGray),
        ),
    ]);
    let outer = block_titled(title, true);
    let inner = outer.inner(area);
    frame.render_widget(outer, area);
    let list_area = picker_list_area(area);
    if inner.height == 0 {
        return;
    }
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("> ", Style::new().fg(Color::Green)),
            Span::raw(picker.filter.clone()),
        ])),
        Rect::new(inner.x, inner.y, inner.width, 1),
    );

    let matches = app.picker_matches();
    if matches.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::styled(
                "no method matches",
                Style::new().fg(Color::DarkGray),
            )),
            list_area,
        );
        return;
    }
    let map = picker_row_map(app);
    // A header row is always followed by the first method of its service.
    let rows: Vec<ListItem> = map
        .iter()
        .enumerate()
        .map(|(row, entry)| match entry {
            Some(i) => ListItem::new(Line::from(vec![
                Span::raw("  "),
                Span::raw(matches[*i].name.clone()),
                Span::styled(
                    format!("  [{}]", matches[*i].kind.label()),
                    Style::new().fg(Color::Magenta),
                ),
            ])),
            None => {
                let service = map
                    .get(row + 1)
                    .copied()
                    .flatten()
                    .map(|i| matches[i].service.clone())
                    .unwrap_or_default();
                ListItem::new(Line::styled(
                    service,
                    Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD),
                ))
            }
        })
        .collect();
    let selected_row = map.iter().position(|r| *r == Some(picker.selected));
    let mut state = ListState::default()
        .with_selected(selected_row)
        .with_offset(list_offset(selected_row, list_area.height));
    frame.render_stateful_widget(
        List::new(rows).highlight_style(
            Style::new()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        ),
        list_area,
        &mut state,
    );
}

// ---- reading panel ----------------------------------------------------------

fn draw_modal(frame: &mut Frame, modal: &super::modal::Modal, screen: Rect) {
    let area = modal_area(screen);
    frame.render_widget(Clear, area);
    let view = if modal.is_formatted() {
        "formatted"
    } else {
        "as received"
    };
    let title = Line::from(vec![
        Span::styled(
            match modal.kind() {
                super::modal::Kind::Log => format!(" {} ", modal.title()),
                kind => format!(" {} - {} ({view}) ", modal.title(), kind.label()),
            },
            Style::new().add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "| F format | j/k PgUp/PgDn scroll | drag select+copy | y copy | Esc close ",
            Style::new().fg(Color::DarkGray),
        ),
    ]);
    let outer = block_titled(title, true);
    let inner = outer.inner(area);
    frame.render_widget(outer, area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    modal.with_lines(inner.width as usize, |lines| {
        modal.set_viewport(lines.len(), inner.height as usize);
        let start = modal.top().min(lines.len());
        let end = (start + inner.height as usize).min(lines.len());
        let visible: Vec<Line> = lines[start..end]
            .iter()
            .enumerate()
            .map(|(i, line)| match modal.selected_cols(start + i) {
                Some((from, to)) => super::modal::reverse_cols(line.clone(), from, to),
                None => line.clone(),
            })
            .collect();
        frame.render_widget(Paragraph::new(visible), inner);
    });
}

fn block(title: &str, focused: bool) -> Block<'static> {
    block_titled(Line::raw(format!(" {title} ")), focused)
}

fn block_titled(title: Line<'static>, focused: bool) -> Block<'static> {
    let color = if focused { Color::Green } else { Color::Gray };
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(color))
        .title(title)
}

// ---- left ----------------------------------------------------------------

fn draw_left(frame: &mut Frame, app: &App, area: Rect) {
    let (requests, log) = left_split(area);
    draw_requests(frame, app, requests);
    draw_log(frame, app, log);
}

/// The log, newest entry last; shows the tail that fits.
fn draw_log(frame: &mut Frame, app: &App, area: Rect) {
    let outer = block("Logs (Ctrl+E: open)", false);
    let inner = outer.inner(area);
    frame.render_widget(outer, area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }
    if app.log.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::styled(
                "no events yet",
                Style::new().fg(Color::DarkGray),
            )),
            inner,
        );
        return;
    }
    let mut lines: Vec<Line> = app
        .log
        .entries()
        .rev()
        .take(inner.height as usize)
        .map(|e| {
            let color = message_color(e.level, &e.message);
            Line::from(vec![
                Span::styled(format!("{} ", e.time), Style::new().fg(Color::DarkGray)),
                Span::styled(e.message.clone(), Style::new().fg(color)),
            ])
        })
        .collect();
    lines.reverse();
    frame.render_widget(Paragraph::new(lines), inner);
}

fn draw_requests(frame: &mut Frame, app: &App, area: Rect) {
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
            match &t.template.kind {
                TemplateKind::Http(h) => method_span(h.method),
                TemplateKind::Grpc(_) => Span::styled("gRPC  ", Style::new().fg(Color::Magenta)),
            },
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
        .map(|s| match s.kind {
            Kind::Http => {
                let status = match s.status {
                    Some(code) => Span::styled(code.to_string(), status_style(code)),
                    None => Span::styled("ERR", Style::new().fg(Color::Red)),
                };
                let method = match s.method.parse::<Method>() {
                    Ok(method) => method_span(method),
                    Err(_) => Span::raw(format!("{:<6}", s.method)),
                };
                ListItem::new(Line::from(vec![
                    status,
                    Span::raw(" "),
                    method,
                    Span::raw(format!(" {}", s.url)),
                ]))
            }
            Kind::Grpc => {
                let status = match s.status {
                    Some(0) => Span::styled("OK", Style::new().fg(Color::Green)),
                    Some(code) => {
                        Span::styled(status_name(i32::from(code)), Style::new().fg(Color::Red))
                    }
                    None => Span::styled("ERR", Style::new().fg(Color::Red)),
                };
                ListItem::new(Line::from(vec![
                    status,
                    Span::raw(" "),
                    Span::styled("gRPC ", Style::new().fg(Color::Magenta)),
                    Span::raw(short_method(&s.method).to_owned()),
                ]))
            }
        })
        .collect();
    (rows, Some(app.history_sel))
}

/// `user.v1.UserService/GetUser` -> `UserService/GetUser`.
fn short_method(full: &str) -> &str {
    let service_end = full.find('/').unwrap_or(full.len());
    let start = full[..service_end].rfind('.').map_or(0, |i| i + 1);
    &full[start..]
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
    let center = center_areas(area, request.kind);

    let grpc = request.kind == Kind::Grpc;
    let method_block = block(if grpc { "TLS" } else { "Method" }, false);
    let method_inner = method_block.inner(center.method);
    frame.render_widget(method_block, center.method);
    let method_text = if grpc {
        tls_span(&request.url, request.grpc.insecure)
    } else {
        method_span(request.method)
    };
    frame.render_widget(Paragraph::new(method_text), method_inner);

    draw_address(frame, app, request, previewing, center.url);
    if grpc {
        draw_schema(frame, app, request, previewing, center.schema);
        draw_rpc(frame, app, request, previewing, center.rpc);
    }
    draw_request_box(frame, app, request, previewing, center.body);
}

/// The URL (REST) or address (gRPC) box, with the kind selector on its top border.
fn draw_address(
    frame: &mut Frame,
    app: &App,
    request: &super::app::RequestView,
    previewing: bool,
    area: Rect,
) {
    let title = match (request.kind, previewing) {
        (Kind::Http, false) => "URL",
        (Kind::Http, true) => "URL (preview)",
        (Kind::Grpc, false) => "Address",
        (Kind::Grpc, true) => "Address (preview)",
    };
    let selected = Style::new()
        .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
        .fg(Color::Yellow);
    let (rest_style, grpc_style) = match request.kind {
        Kind::Http => (selected, Style::new()),
        Kind::Grpc => (Style::new(), selected),
    };
    let mut title_spans = vec![Span::raw(format!(" {title} "))];
    if request.kind == Kind::Grpc && request.grpc.insecure && is_tls(&request.url) {
        title_spans.push(Span::styled(
            "- NOT VERIFIED ",
            Style::new().fg(Color::Red).add_modifier(Modifier::BOLD),
        ));
    }
    let block = block_titled(Line::from(title_spans), app.focus == Focus::Url).title_top(
        Line::from(vec![
            Span::raw(" "),
            Span::styled("REST", rest_style),
            Span::raw(" | "),
            Span::styled("gRPC", grpc_style),
            Span::raw(" "),
        ])
        .right_aligned(),
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if previewing {
        frame.render_widget(Paragraph::new(request.url.clone()), inner);
    } else {
        render_editor(
            frame,
            &app.url,
            inner,
            app.focus == Focus::Url,
            Syntax::Plain,
        );
    }
}

fn is_tls(address: &str) -> bool {
    address.trim_start().starts_with("https://")
}

fn tls_span(address: &str, insecure: bool) -> Span<'static> {
    match (is_tls(address), insecure) {
        (false, _) => Span::styled("plain", Style::new().fg(Color::DarkGray)),
        (true, false) => Span::styled("verified", Style::new().fg(Color::Green)),
        (true, true) => Span::styled(
            "insecure",
            Style::new().fg(Color::Red).add_modifier(Modifier::BOLD),
        ),
    }
}

/// gRPC: where the schema comes from, and what was loaded from it.
fn draw_schema(
    frame: &mut Frame,
    app: &App,
    request: &super::app::RequestView,
    previewing: bool,
    area: Rect,
) {
    let status = if previewing {
        None
    } else {
        Some(app.schema_status())
    };
    let mut block = block("Schema", app.focus == Focus::Schema);
    if let Some(status) = status {
        block = block.title_top(
            Line::styled(format!(" {status} "), Style::new().fg(Color::DarkGray)).right_aligned(),
        );
    }
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let file_mode = match (previewing, &request.grpc.schema) {
        (true, SchemaSource::File(_)) => true,
        (true, SchemaSource::Reflection) => false,
        (false, _) => app.schema_mode == SchemaMode::File,
    };
    if file_mode && !previewing {
        if app.schema_path_text().is_empty() && app.focus != Focus::Schema {
            frame.render_widget(
                Paragraph::new(Line::styled(
                    "file: path to a .proto, .bin, .pb or .desc (Ctrl+O: reflection)",
                    Style::new().fg(Color::DarkGray),
                )),
                inner,
            );
        } else {
            render_editor(
                frame,
                &app.schema_path,
                inner,
                app.focus == Focus::Schema,
                Syntax::Plain,
            );
        }
    } else if let (true, SchemaSource::File(path)) = (previewing, &request.grpc.schema) {
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("file: ", Style::new().fg(Color::Cyan)),
                Span::raw(path.to_string_lossy().into_owned()),
            ])),
            inner,
        );
    } else {
        let hint = if previewing {
            ""
        } else {
            "  (Ctrl+O: use a file, Ctrl+L: load)"
        };
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("reflection", Style::new().fg(Color::Cyan)),
                Span::styled(hint, Style::new().fg(Color::DarkGray)),
            ])),
            inner,
        );
    }
}

/// gRPC: the selected `service/method` and its call kind.
fn draw_rpc(
    frame: &mut Frame,
    app: &App,
    request: &super::app::RequestView,
    previewing: bool,
    area: Rect,
) {
    let title = if previewing {
        "Method (preview)"
    } else {
        "Method (Enter: pick)"
    };
    let block = block(title, app.focus == Focus::Method);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let line = if request.grpc.method.is_empty() {
        Line::styled(
            "none selected: load the schema, then pick a method",
            Style::new().fg(Color::DarkGray),
        )
    } else {
        let mut spans = vec![Span::styled(
            request.grpc.method.clone(),
            Style::new().fg(Color::Yellow),
        )];
        if let Some(kind) = app.method_kind(&request.grpc.method) {
            spans.push(Span::styled(
                format!("  [{}]", kind.label()),
                Style::new().fg(Color::Magenta),
            ));
        }
        Line::from(spans)
    };
    frame.render_widget(Paragraph::new(line), inner);
}

/// The Body / Headers (Metadata) box.
fn draw_request_box(
    frame: &mut Frame,
    app: &App,
    request: &super::app::RequestView,
    previewing: bool,
    area: Rect,
) {
    let lower_focused = matches!(app.focus, Focus::Body | Focus::Headers);
    let (body_label, headers_label) = request_tab_labels(request.headers.len(), request.kind);
    let active = Style::new()
        .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
        .fg(Color::Yellow);
    let (body_style, headers_style) = match app.request_tab {
        RequestTab::Body => (active, Style::new()),
        RequestTab::Headers => (Style::new(), active),
    };
    let title = Line::from(vec![
        Span::styled(body_label, body_style),
        Span::raw("|"),
        Span::styled(headers_label, headers_style),
    ]);
    let body_block = block_titled(title, lower_focused);
    let body_inner = body_block.inner(area);
    frame.render_widget(body_block, area);
    match (app.request_tab, previewing) {
        (RequestTab::Body, true) => {
            let lines: Vec<Line> = request.body.lines().map(highlight_line).collect();
            frame.render_widget(Paragraph::new(lines), body_inner);
        }
        (RequestTab::Headers, true) => {
            let lines: Vec<Line> = headers::to_lines(&request.headers)
                .iter()
                .map(|l| headers::highlight_line(l))
                .collect();
            frame.render_widget(Paragraph::new(lines), body_inner);
        }
        (RequestTab::Body, false) => render_editor(
            frame,
            &app.body,
            body_inner,
            app.focus == Focus::Body,
            Syntax::Json,
        ),
        (RequestTab::Headers, false) => draw_header_rows(frame, app, body_inner),
    }
}

/// The header inputs: name `:` value, one row each, then the "add header" line.
fn draw_header_rows(frame: &mut Frame, app: &App, area: Rect) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let editor = &app.headers;
    let focused = app.focus == Focus::Headers;
    let grid = header_grid(area, editor.rows.len(), editor.row);
    for cells in &grid.rows {
        let row = &editor.rows[cells.row];
        let sep = Rect::new(cells.name.right(), cells.name.y, 1, 1);
        frame.render_widget(
            Paragraph::new(Span::styled(":", Style::new().fg(Color::DarkGray))),
            sep,
        );
        for (rect, col, ta, placeholder) in [
            (cells.name, Col::Name, &row.name, "Name"),
            (cells.value, Col::Value, &row.value, "Value"),
        ] {
            let here = focused && cells.row == editor.row && col == editor.col;
            if here {
                frame.render_widget(
                    Block::default().style(Style::new().bg(Color::DarkGray)),
                    rect,
                );
            }
            if ta.lines()[0].is_empty() && !here {
                frame.render_widget(
                    Paragraph::new(Line::styled(placeholder, Style::new().fg(Color::DarkGray))),
                    rect,
                );
                continue;
            }
            let syntax = if col == Col::Name {
                if headers::name_is_invalid(row) {
                    Syntax::Invalid
                } else {
                    Syntax::HeaderName
                }
            } else {
                Syntax::Plain
            };
            render_editor(frame, ta, rect, here, syntax);
        }
    }
    if let Some(rect) = grid.add {
        frame.render_widget(
            Paragraph::new(Line::styled(
                "+ add header (Enter on the last value, Ctrl+D removes a row)",
                Style::new().fg(Color::DarkGray),
            )),
            rect,
        );
    }
}

#[derive(Clone, Copy)]
enum Syntax {
    HeaderName,
    Invalid,
    Plain,
    Json,
}

/// Draws a `TextArea`'s lines ourselves (it has no syntax-highlight hook) and places
/// the terminal cursor when the editor has focus.
fn render_editor(
    frame: &mut Frame,
    ta: &TextArea<'static>,
    area: Rect,
    focused: bool,
    syntax: Syntax,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let cursor = ta.cursor();
    let (row, col) = (cursor.0, cursor.1);
    let (row_scroll, col_scroll) = editor_scroll((row, col), area);

    let lines: Vec<Line> = ta
        .lines()
        .iter()
        .map(|l| match syntax {
            Syntax::Json => highlight_line(l),
            Syntax::HeaderName => Line::styled(l.clone(), Style::new().fg(Color::Cyan)),
            Syntax::Invalid => Line::styled(l.clone(), Style::new().fg(Color::Red)),
            Syntax::Plain => Line::raw(l.clone()),
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
        Some(Outcome::Grpc(g)) if app.sending => {
            let text = if g.messages.is_empty() && !g.response_stream {
                "sending...  ^X cancel".to_owned()
            } else {
                format!("streaming {}  ^X cancel", g.messages.len())
            };
            Line::styled(text, Style::new().fg(Color::Yellow))
        }
        _ if app.sending => Line::styled("sending...", Style::new().fg(Color::Yellow)),
        Some(Outcome::Response(r)) => Line::from(vec![
            Span::styled(
                r.status.to_string(),
                status_style(r.status).add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!("  {} ms", r.elapsed.as_millis())),
        ]),
        Some(Outcome::Grpc(g)) => match &g.status {
            Some(s) => Line::from(vec![
                Span::styled(
                    format!("{} ({})", s.name(), s.code),
                    Style::new()
                        .fg(if s.code == 0 {
                            Color::Green
                        } else {
                            Color::Red
                        })
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(format!("  {} ms", g.elapsed.as_millis())),
                Span::styled(
                    match s.trailers.len() {
                        0 => String::new(),
                        1 => "  1 trailer".to_owned(),
                        n => format!("  {n} trailers"),
                    },
                    Style::new().fg(Color::DarkGray),
                ),
            ]),
            None => Line::styled("failed", Style::new().fg(Color::Red)),
        },
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
        Some(Outcome::Grpc(g)) => {
            let trailers = g.status.iter().flat_map(|s| s.trailers.iter());
            g.metadata
                .iter()
                .map(|h| (h, Color::Cyan, ""))
                .chain(trailers.map(|h| (h, Color::Blue, "trailer ")))
                .map(|(h, color, prefix)| {
                    Line::from(vec![
                        Span::styled(format!("{prefix}{}: ", h.name), Style::new().fg(color)),
                        Span::raw(h.value.clone()),
                    ])
                })
                .collect()
        }
        _ => Vec::new(),
    };
    frame.render_widget(Paragraph::new(header_lines), headers_inner);

    let body_block = block("Body (Enter: open)", app.focus == Focus::Response);
    let body_inner = body_block.inner(body_area);
    frame.render_widget(body_block, body_area);
    let body_lines: Vec<Line> = match outcome {
        Some(Outcome::Response(r)) => pretty(&r.body).lines().map(highlight_line).collect(),
        Some(Outcome::Error(msg)) => vec![Line::styled(msg.clone(), Style::new().fg(Color::Red))],
        Some(Outcome::Grpc(g)) => grpc_body_lines(g),
        None => Vec::new(),
    };
    frame.render_widget(
        Paragraph::new(body_lines)
            .wrap(ratatui::widgets::Wrap { trim: false })
            .scroll((app.scroll, 0)),
        body_inner,
    );
}

/// The messages of a gRPC call, numbered when there can be several, then the status
/// message of a call that did not end `OK`.
fn grpc_body_lines(g: &crate::client::GrpcOutcome) -> Vec<Line<'static>> {
    if let (None, Some(error)) = (&g.status, &g.error) {
        return vec![Line::styled(error.clone(), Style::new().fg(Color::Red))];
    }
    let numbered = g.response_stream || g.messages.len() > 1;
    let mut lines = Vec::new();
    for (i, message) in g.messages.iter().enumerate() {
        if numbered {
            if i > 0 {
                lines.push(Line::raw(""));
            }
            lines.push(Line::styled(
                format!("#{}", i + 1),
                Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD),
            ));
        }
        lines.extend(pretty(message).lines().map(highlight_line));
    }
    if let Some(status) = g.status.as_ref().filter(|s| s.code != 0) {
        if !lines.is_empty() {
            lines.push(Line::raw(""));
        }
        let text = if status.message.is_empty() {
            status.name().to_owned()
        } else {
            format!("{}: {}", status.name(), status.message)
        };
        lines.push(Line::styled(text, Style::new().fg(Color::Red)));
    }
    lines
}

// ---- status bar ------------------------------------------------------------

/// Both fit a 120-column terminal; the README has the full key tables.
pub const HELP_REST: &str = "Tab focus | Ctrl+G gRPC | [ ] tabs | j/k move | Enter load | Ctrl+S send | Ctrl+N/P method | Ctrl+T hdrs | q quit";
pub const HELP_GRPC: &str = "Ctrl+G REST/gRPC | Ctrl+L schema | Ctrl+N method | Ctrl+S send | Ctrl+X cancel | Ctrl+O source | Ctrl+K TLS | q quit";

fn draw_status(frame: &mut Frame, app: &App, area: Rect) {
    let text = match &app.status {
        Some(message) => Line::styled(message.clone(), Style::new().fg(Color::Red)),
        None if app.notice.is_some() => Line::styled(
            app.notice.clone().unwrap_or_default(),
            Style::new().fg(Color::Green),
        ),
        None => Line::styled(
            match app.kind {
                Kind::Http => HELP_REST,
                Kind::Grpc => HELP_GRPC,
            },
            Style::new().fg(Color::DarkGray),
        ),
    };
    frame.render_widget(Paragraph::new(text), area);
}
