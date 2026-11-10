//! Geometry of the screen: where each panel and widget sits, and hit-testing for the mouse.

use ratatui::layout::{Constraint, Layout, Margin, Position, Rect};

use crate::client::{BodyKind, Kind};
use crate::tui::app::{App, LeftTab, RequestTab, Tab};

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
    /// The raw HTTP view under the body; REST only.
    pub preview: Rect,
    pub response: Rect,
    pub response_body: Rect,
}

/// Splits the left column 50/50: the request list on top, the log below. Both are outer
/// rects.
pub(crate) fn left_split(left: Rect) -> (Rect, Rect) {
    let [requests, log] =
        Layout::vertical([Constraint::Ratio(1, 2), Constraint::Ratio(1, 2)]).areas(left);
    (requests, log)
}

/// Splits the request list's outer rect into its tab row and list.
pub(crate) fn left_areas(requests: Rect) -> (Rect, Rect) {
    let [tabs, list] =
        Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).areas(inner(requests));
    (tabs, list)
}

/// Outer rects of the center panel's boxes. `schema` and `rpc` only exist in gRPC mode
/// and are empty otherwise; `method` holds the TLS state there.
pub(crate) struct Center {
    pub(crate) method: Rect,
    pub(crate) url: Rect,
    pub(crate) schema: Rect,
    pub(crate) rpc: Rect,
    pub(crate) body: Rect,
    /// The raw HTTP view of the request.
    pub(crate) preview: Rect,
}

/// Rows of the raw HTTP view under the request box, borders included.
const PREVIEW_HEIGHT: u16 = 10;

pub(crate) fn center_areas(center: Rect, kind: Kind) -> Center {
    let top_row = |top: Rect| {
        Layout::horizontal([Constraint::Length(12), Constraint::Min(10)]).areas::<2>(top)
    };
    match kind {
        Kind::Http | Kind::Ws => {
            let [top, body, preview] = Layout::vertical([
                Constraint::Length(3),
                Constraint::Min(3),
                Constraint::Length(PREVIEW_HEIGHT),
            ])
            .areas(center);
            let [method, url] = top_row(top);
            Center {
                method,
                url,
                schema: Rect::ZERO,
                rpc: Rect::ZERO,
                body,
                preview,
            }
        }
        Kind::Graphql => {
            let [top, rpc, body, preview] = Layout::vertical([
                Constraint::Length(3),
                Constraint::Length(3),
                Constraint::Min(3),
                Constraint::Length(PREVIEW_HEIGHT),
            ])
            .areas(center);
            let [method, url] = top_row(top);
            Center {
                method,
                url,
                schema: Rect::ZERO,
                rpc,
                body,
                preview,
            }
        }
        Kind::Grpc => {
            let [top, schema, rpc, body, preview] = Layout::vertical([
                Constraint::Length(3),
                Constraint::Length(3),
                Constraint::Length(3),
                Constraint::Min(3),
                Constraint::Length(PREVIEW_HEIGHT),
            ])
            .areas(center);
            let [method, url] = top_row(top);
            Center {
                method,
                url,
                schema,
                rpc,
                body,
                preview,
            }
        }
    }
}

/// Outer rects of the status, headers and body boxes.
pub(crate) fn right_areas(right: Rect) -> (Rect, Rect, Rect) {
    Layout::vertical([
        Constraint::Length(3),
        Constraint::Percentage(30),
        Constraint::Min(3),
    ])
    .areas::<3>(right)
    .into()
}

pub(crate) fn main_area(area: Rect) -> (Rect, Rect) {
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
    let mut last_service: Option<String> = None;
    for (i, m) in app.picker_matches().into_iter().enumerate() {
        if last_service.as_deref() != Some(m.group.as_str()) {
            rows.push(None);
            last_service = Some(m.group.clone());
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
        preview: center.preview,
        response,
        response_body,
    }
}

/// The kind selector in the top border of the URL box, right-aligned.
pub(crate) const KIND_LABEL: &str = " REST|GQL|gRPC|WS ";

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
        o if o < 6 => Some(Kind::Http),
        o if o < 10 => Some(Kind::Graphql),
        o if o < 15 => Some(Kind::Grpc),
        o if o < width => Some(Kind::Ws),
        _ => None,
    }
}

pub(crate) const HISTORY_TAB_WIDTH: u16 = " History ".len() as u16;

/// Which tab title sits under column `x` of the tab row. Matches `Tabs` default padding
/// (one space each side of a title) and the one-column `|` divider.
pub fn tab_at(tabs: Rect, x: u16) -> Option<Tab> {
    let offset = x.checked_sub(tabs.x)?;
    match offset {
        o if o < HISTORY_TAB_WIDTH => Some(Tab::History),
        o if o == HISTORY_TAB_WIDTH => None, // divider
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

/// Titles of the request's lower box, in order, each with the tab it selects: `" Body "`
/// and `" Headers (n) "` (`" Metadata (n) "` for gRPC; `" Query "`, `" Variables "` and
/// `" Headers (n) "` for GraphQL), joined by `|`.
pub(crate) fn request_tab_labels(
    count: usize,
    params: usize,
    auth: &str,
    kind: Kind,
    panel: usize,
) -> Vec<(RequestTab, String)> {
    let name = match kind {
        Kind::Grpc => "Metadata",
        _ => "Headers",
    };
    let second = if count == 0 {
        format!(" {name} ")
    } else {
        format!(" {name} ({count}) ")
    };
    match kind {
        Kind::Graphql => vec![
            (RequestTab::Body, format!(" [{panel}] Query ")),
            (RequestTab::Variables, " Variables ".to_owned()),
            (RequestTab::Headers, second),
        ],
        Kind::Http => vec![
            (RequestTab::Body, format!(" [{panel}] Body ")),
            (RequestTab::Headers, second),
            (
                RequestTab::Params,
                if params == 0 {
                    " URL Params ".to_owned()
                } else {
                    format!(" URL Params ({params}) ")
                },
            ),
            (
                RequestTab::Auth,
                if auth.is_empty() {
                    " Auth ".to_owned()
                } else {
                    format!(" Auth ({auth}) ")
                },
            ),
        ],
        _ => vec![
            (RequestTab::Body, format!(" [{panel}] Body ")),
            (RequestTab::Headers, second),
        ],
    }
}

/// Which request tab title is under `at`; titles sit on the box's top border, starting
/// one column in. `header_count` and `params` must be the counts shown in the titles.
pub fn request_tab_at(
    body: Rect,
    at: Position,
    header_count: usize,
    params: usize,
    auth: &str,
    kind: Kind,
    panel: usize,
) -> Option<RequestTab> {
    if at.y != body.y {
        return None;
    }
    let mut offset = at.x.checked_sub(body.x + 1)? as usize;
    for (tab, label) in request_tab_labels(header_count, params, auth, kind, panel) {
        if offset < label.len() {
            return Some(tab);
        }
        // the `|` between two titles belongs to neither
        offset = offset.checked_sub(label.len() + 1)?;
    }
    None
}

/// The body type shown on the right of the request box's top border, e.g. `" JSON (RAW) "`.
pub fn body_kind_title(kind: BodyKind) -> String {
    format!(" {kind} ")
}

/// Whether `at` is on the body type title of the request box (`body` is its outer rect).
pub fn body_kind_title_at(body: Rect, at: Position, kind: BodyKind) -> bool {
    let width = body_kind_title(kind).len() as u16;
    // the title ends one column before the corner
    at.y == body.y && at.x >= body.right().saturating_sub(width + 1) && at.x < body.right() - 1
}

/// Width of the `Logs` label in the lower left panel's title.
const LOGS_LABEL: u16 = 4;
/// Width of the `Variables` label in the lower left panel's title.
const VARIABLES_LABEL: u16 = 9;

/// The tab under `at` on the top border of the lower left panel (`logs` is its outer
/// rect). The title is ` Logs|Variables `.
pub fn left_tab_at(logs: Rect, at: Position) -> Option<LeftTab> {
    if at.y != logs.y {
        return None;
    }
    let offset = at.x.checked_sub(logs.x + 2)?;
    match offset {
        o if o < LOGS_LABEL => Some(LeftTab::Logs),
        o if o > LOGS_LABEL && o <= LOGS_LABEL + VARIABLES_LABEL => Some(LeftTab::Variables),
        _ => None,
    }
}

/// Splits the Variables panel's inner rect into the capture rows (with their "add" line),
/// the captured values and a line of key hints. `count` is the number of capture rows.
pub fn captures_areas(inner: Rect, count: usize) -> (Rect, Rect, Rect) {
    let body = inner.height.saturating_sub(1);
    let rows = ((count + 1) as u16).min(body * 2 / 3).max(1.min(body));
    let [rows, values] =
        Layout::vertical([Constraint::Length(rows), Constraint::Min(0)]).areas(Rect {
            height: body,
            ..inner
        });
    let hint = Rect::new(
        inner.x,
        inner.bottom().saturating_sub(1),
        inner.width,
        1.min(inner.height),
    );
    (rows, values, hint)
}

/// Whether the `tab` of a `kind` request has a "use variables" switch: headers (or metadata)
/// for every kind, and REST's params and body.
pub fn vars_tab(kind: crate::client::Kind, tab: crate::tui::app::RequestTab) -> bool {
    use crate::tui::app::RequestTab as T;
    match tab {
        T::Headers => true,
        T::Params | T::Body => kind == crate::client::Kind::Http,
        _ => false,
    }
}

/// The "use variables" checkbox, on the right of the request box's bottom border.
pub fn vars_title(on: bool) -> String {
    format!(" [{}] $vars ", if on { 'x' } else { ' ' })
}

/// Whether `at` is on the "use variables" checkbox (`body` is the request box's outer rect).
pub fn vars_title_at(body: Rect, at: Position, on: bool) -> bool {
    let width = vars_title(on).len() as u16;
    at.y + 1 == body.bottom()
        && at.x >= body.right().saturating_sub(width + 1)
        && at.x < body.right() - 1
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

/// The inputs of one JSON field row.
#[derive(Debug, Clone, Copy)]
pub struct JsonCells {
    pub row: usize,
    pub key: Rect,
    pub value: Rect,
    /// The row's type, at the right edge.
    pub tag: Rect,
}

/// Where the JSON field rows are drawn inside the request box.
pub struct JsonGrid {
    pub rows: Vec<JsonCells>,
    /// The line of key hints under the last row, when it is in view.
    pub hint: Option<Rect>,
}

/// Width of the type column of a JSON row: the longest type name and a space.
const JSON_TAG_WIDTH: u16 = 8;

/// Lays out the rows of a JSON editor: the key (indented by depth), a `:`, the value and
/// the type, scrolled so that `selected` is in view. `depths` has one entry per row.
pub fn json_grid(inner: Rect, depths: &[usize], selected: usize) -> JsonGrid {
    let height = inner.height as usize;
    let offset = list_offset(Some(selected), inner.height);
    let key_width = (inner.width / 3).max(6);
    let rows = (offset..depths.len().min(offset + height))
        .map(|row| {
            let y = inner.y + (row - offset) as u16;
            let x = (inner.x + (depths[row] * 2) as u16).min(inner.right().saturating_sub(1));
            let key = Rect::new(x, y, key_width.min(inner.right() - x), 1);
            let value_x = (key.right() + 2).min(inner.right());
            let tag_x = inner.right().saturating_sub(JSON_TAG_WIDTH).max(value_x);
            JsonCells {
                row,
                key,
                value: Rect::new(value_x, y, tag_x.saturating_sub(value_x + 1), 1),
                tag: Rect::new(tag_x, y, inner.right() - tag_x, 1),
            }
        })
        .collect();
    let count = depths.len();
    let hint = (count >= offset && count < offset + height)
        .then(|| Rect::new(inner.x, inner.y + (count - offset) as u16, inner.width, 1));
    JsonGrid { rows, hint }
}
