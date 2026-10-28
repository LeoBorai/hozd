use std::sync::Arc;
use std::time::Duration;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use hozd::client::{FakeClient, Header, Method, Request, Response};
use hozd::history::{History, Outcome};
use hozd::project::load_projects;
use hozd::tui::{App, Focus, Msg, Tab, ui};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

struct Harness {
    app: App,
    rx: UnboundedReceiver<Msg>,
    fake: Arc<FakeClient>,
    store: History,
    _tmp: tempfile::TempDir,
}

fn ok_response() -> Response {
    Response {
        status: 200,
        headers: vec![Header::new("x-h", "1")],
        body: "{\"x\":1}".into(),
        elapsed: Duration::from_millis(7),
    }
}

async fn harness_with(
    reply: Result<Response, hozd::client::ClientError>,
    projects: Option<&str>,
) -> Harness {
    let tmp = tempfile::tempdir().unwrap();
    let store = History::open(&tmp.path().join("db.sqlite3")).await.unwrap();
    let fake = Arc::new(FakeClient::new(reply));
    let (tx, rx) = unbounded_channel();
    let loaded = projects
        .map(|_| load_projects(&tmp.path().join("projects")))
        .unwrap_or_default();
    let app = App::new(fake.clone(), store.clone(), loaded, tx);
    Harness {
        app,
        rx,
        fake,
        store,
        _tmp: tmp,
    }
}

async fn harness() -> Harness {
    harness_with(Ok(ok_response()), None).await
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn ctrl(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
}

fn type_text(app: &mut App, text: &str) {
    for c in text.chars() {
        app.handle_key(key(KeyCode::Char(c)));
    }
}

/// Applies background messages until `done` holds.
async fn pump(h: &mut Harness, done: impl Fn(&App) -> bool) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while !done(&h.app) {
            let msg = h.rx.recv().await.expect("channel open");
            h.app.update(msg);
        }
    })
    .await
    .expect("timed out waiting for app state");
}

fn render(app: &App, w: u16, h: u16) -> Buffer {
    let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
    terminal.draw(|f| ui::draw(f, app)).unwrap();
    terminal.backend().buffer().clone()
}

fn text(buf: &Buffer) -> String {
    let w = buf.area.width as usize;
    buf.content()
        .chunks(w)
        .map(|row| row.iter().map(|c| c.symbol()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

// ---- layout ------------------------------------------------------------

#[test]
fn panels_are_3_6_3_of_12_columns() {
    let [l, c, r] = ui::panel_areas(Rect::new(0, 0, 120, 29));
    assert_eq!((l.width, c.width, r.width), (30, 60, 30));
    assert_eq!((l.x, c.x, r.x), (0, 30, 90));
}

#[tokio::test]
async fn left_panel_has_both_tabs_and_empty_hints() {
    let mut h = harness().await;
    let out = text(&render(&h.app, 120, 30));
    assert!(out.contains("Stored") && out.contains("History"));
    assert!(out.contains("no templates"));

    h.app.handle_key(key(KeyCode::Char(']')));
    assert_eq!(h.app.tab, Tab::History);
    assert!(text(&render(&h.app, 120, 30)).contains("no requests sent yet"));
    h.app.handle_key(key(KeyCode::Char('[')));
    assert_eq!(h.app.tab, Tab::Stored);
}

#[tokio::test]
async fn stored_tab_groups_templates_by_project() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("projects/api/users");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("create.toml"),
        "name = \"Create user\"\nmethod = \"POST\"\nurl = \"http://x/users\"\nbody = '''\n{\"n\": 1}\n'''\n",
    )
    .unwrap();
    let store = History::open(&tmp.path().join("db.sqlite3")).await.unwrap();
    let (tx, _rx) = unbounded_channel();
    let app = App::new(
        Arc::new(FakeClient::new(Ok(ok_response()))),
        store,
        load_projects(&tmp.path().join("projects")),
        tx,
    );
    let out = text(&render(&app, 120, 30));
    assert!(out.contains("api/") && out.contains("Create user"), "{out}");
}

// ---- editor ------------------------------------------------------------

#[tokio::test]
async fn building_a_request_highlights_json_body() {
    let mut h = harness().await;
    h.app.handle_key(ctrl('n')); // GET -> POST
    h.app.handle_key(key(KeyCode::Tab)); // URL
    type_text(&mut h.app, "http://x/y");
    h.app.handle_key(key(KeyCode::Tab)); // Body
    assert_eq!(h.app.focus, Focus::Body);
    type_text(&mut h.app, "{\"a\": 1}");

    let req = h.app.editor_request();
    assert_eq!((req.method, req.url.as_str()), (Method::Post, "http://x/y"));
    assert_eq!(req.body.as_deref(), Some("{\"a\": 1}"));

    let buf = render(&h.app, 120, 30);
    let styled = |sym: &str, color: Color| {
        buf.content()
            .iter()
            .any(|c| c.symbol() == sym && c.fg == color)
    };
    assert!(styled("1", Color::Yellow), "number styled");
    assert!(styled("a", Color::Cyan), "key styled");
}

// ---- send ----------------------------------------------------------------

#[tokio::test]
async fn send_is_non_blocking_then_records_history_and_shows_response() {
    let mut h = harness().await;
    h.app.handle_key(key(KeyCode::Tab));
    type_text(&mut h.app, "http://x/y");
    h.app.handle_key(ctrl('s'));
    assert!(
        h.app.sending,
        "handle_key returned before the request finished"
    );

    pump(&mut h, |a| a.live.is_some() && !a.history.is_empty()).await;

    assert!(!h.app.sending);
    assert_eq!(h.fake.requests().len(), 1);
    assert_eq!(h.app.history[0].url, "http://x/y");
    let out = text(&render(&h.app, 120, 30));
    assert!(
        out.contains("200") && out.contains("x-h") && out.contains("\"x\": 1"),
        "{out}"
    );
}

#[tokio::test]
async fn transport_error_is_shown_and_recorded() {
    let mut h = harness_with(
        Err(hozd::client::ClientError::Transport(
            "connection refused".into(),
        )),
        None,
    )
    .await;
    h.app.handle_key(key(KeyCode::Tab));
    type_text(&mut h.app, "http://x");
    h.app.handle_key(key(KeyCode::Enter)); // Enter in the URL field sends

    pump(&mut h, |a| a.live.is_some() && !a.history.is_empty()).await;
    // the message wraps inside the narrow right panel, so check fragments
    let out = text(&render(&h.app, 120, 30));
    assert!(out.contains("failed") && out.contains("refused"), "{out}");
    assert_eq!(h.app.history[0].status, None);
}

#[tokio::test]
async fn send_without_url_does_nothing() {
    let mut h = harness().await;
    h.app.handle_key(ctrl('s'));
    assert!(!h.app.sending);
    assert!(h.fake.requests().is_empty());
}

// ---- preview / load --------------------------------------------------------

async fn with_two_history_entries() -> Harness {
    let mut h = harness().await;
    for (url, outcome) in [
        ("http://x/first", Outcome::Response(ok_response())),
        ("http://x/second", Outcome::Error("boom".into())),
    ] {
        let req = Request {
            method: Method::Post,
            url: url.into(),
            headers: vec![],
            body: Some("{\"k\": true}".into()),
        };
        h.store.record(&req, &outcome).await.unwrap();
    }
    h.app.refresh_history();
    pump(&mut h, |a| a.history.len() == 2).await;
    h.app.set_tab(Tab::History);
    h
}

#[tokio::test]
async fn hovering_history_previews_request_and_response_without_sending() {
    let mut h = with_two_history_entries().await;

    pump(&mut h, |a| a.view().previewing).await;
    assert_eq!(h.app.view().request.url, "http://x/second");
    assert!(matches!(h.app.view().outcome, Some(Outcome::Error(m)) if m == "boom"));

    h.app.handle_key(key(KeyCode::Char('j')));
    pump(&mut h, |a| a.view().request.url == "http://x/first").await;
    assert!(matches!(h.app.view().outcome, Some(Outcome::Response(r)) if r.status == 200));
    let out = text(&render(&h.app, 120, 30));
    assert!(
        out.contains("http://x/first") && out.contains("\"x\": 1"),
        "{out}"
    );

    assert!(
        h.fake.requests().is_empty(),
        "hover must not execute anything"
    );
    assert_eq!(
        h.store.list(10).await.unwrap().len(),
        2,
        "hover must not add history"
    );
}

#[tokio::test]
async fn enter_on_history_loads_request_into_editor() {
    let mut h = with_two_history_entries().await;
    pump(&mut h, |a| a.view().previewing).await;
    h.app.handle_key(key(KeyCode::Enter));

    assert_eq!(h.app.focus, Focus::Url);
    let req = h.app.editor_request();
    assert_eq!(
        (req.method, req.url.as_str()),
        (Method::Post, "http://x/second")
    );
    assert_eq!(req.body.as_deref(), Some("{\"k\": true}"));
    assert!(h.fake.requests().is_empty());
}

#[tokio::test]
async fn enter_on_stored_template_loads_it_ready_to_send() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("projects/api");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("t.toml"),
        "name = \"T\"\nmethod = \"PUT\"\nurl = \"http://x/t\"\nbody = '''\n{\"a\": 1}\n'''\n\n[headers]\nX-A = \"b\"\n",
    )
    .unwrap();
    let store = History::open(&tmp.path().join("db.sqlite3")).await.unwrap();
    let (tx, _rx) = unbounded_channel();
    let mut app = App::new(
        Arc::new(FakeClient::new(Ok(ok_response()))),
        store,
        load_projects(&tmp.path().join("projects")),
        tx,
    );

    // hovering previews the template in the center panel
    assert_eq!(app.view().request.url, "http://x/t");
    assert!(app.view().previewing);

    app.handle_key(key(KeyCode::Enter));
    let req = app.editor_request();
    assert_eq!((req.method, req.url.as_str()), (Method::Put, "http://x/t"));
    assert_eq!(req.headers, vec![Header::new("X-A", "b")]);
    assert!(req.body.unwrap().contains("\"a\": 1"));
    assert!(!app.view().previewing);
}

#[tokio::test]
async fn quit_keys() {
    let mut h = harness().await;
    h.app.handle_key(key(KeyCode::Tab));
    h.app.handle_key(key(KeyCode::Char('q'))); // typed into the URL, not a quit
    assert!(!h.app.should_quit);
    h.app.handle_key(ctrl('c'));
    assert!(h.app.should_quit);

    let mut h = harness().await;
    h.app.handle_key(key(KeyCode::Char('q')));
    assert!(h.app.should_quit);
}

// ---- mouse -----------------------------------------------------------------

use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};

const SCREEN: Rect = Rect::new(0, 0, 120, 30);

fn mouse(kind: MouseEventKind, x: u16, y: u16) -> MouseEvent {
    MouseEvent {
        kind,
        column: x,
        row: y,
        modifiers: KeyModifiers::NONE,
    }
}

fn click(app: &mut App, x: u16, y: u16) {
    app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), x, y), SCREEN);
}

#[tokio::test]
async fn clicking_tabs_switches_them() {
    let mut h = harness().await;
    let a = ui::areas(SCREEN);
    click(&mut h.app, a.tabs.x + 12, a.tabs.y); // inside " History "
    assert_eq!((h.app.tab, h.app.focus), (Tab::History, Focus::List));
    click(&mut h.app, a.tabs.x + 2, a.tabs.y); // inside " Stored "
    assert_eq!(h.app.tab, Tab::Stored);
    click(&mut h.app, a.tabs.x + 8, a.tabs.y); // the divider
    assert_eq!(h.app.tab, Tab::Stored);
}

#[tokio::test]
async fn clicking_history_selects_then_second_click_loads() {
    let mut h = with_two_history_entries().await;
    let a = ui::areas(SCREEN);

    click(&mut h.app, a.list.x + 3, a.list.y + 1); // second row = older entry
    assert_eq!(h.app.history_sel, 1);
    pump(&mut h, |a| a.view().request.url == "http://x/first").await;
    assert_eq!(h.app.focus, Focus::List, "first click only previews");

    click(&mut h.app, a.list.x + 3, a.list.y + 1);
    assert_eq!(h.app.focus, Focus::Url);
    assert_eq!(h.app.editor_request().url, "http://x/first");
    assert!(h.fake.requests().is_empty(), "clicking never sends");
}

#[tokio::test]
async fn clicking_empty_list_space_or_response_panel_changes_nothing() {
    let mut h = with_two_history_entries().await;
    let a = ui::areas(SCREEN);
    click(&mut h.app, a.list.x + 1, a.list.y + 10); // below the two rows
    click(&mut h.app, a.response_body.x + 1, a.response_body.y + 1);
    assert_eq!(
        (h.app.history_sel, h.app.focus, h.app.tab),
        (0, Focus::List, Tab::History)
    );
}

#[tokio::test]
async fn clicking_stored_templates_skips_project_headers() {
    let tmp = tempfile::tempdir().unwrap();
    for (project, file) in [("api", "a.toml"), ("api", "b.toml"), ("web", "c.toml")] {
        let dir = tmp.path().join("projects").join(project);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(file),
            format!("name = \"{file}\"\nmethod = \"GET\"\nurl = \"http://x/{file}\"\n"),
        )
        .unwrap();
    }
    let store = History::open(&tmp.path().join("db.sqlite3")).await.unwrap();
    let (tx, _rx) = unbounded_channel();
    let mut app = App::new(
        Arc::new(FakeClient::new(Ok(ok_response()))),
        store,
        load_projects(&tmp.path().join("projects")),
        tx,
    );
    let a = ui::areas(SCREEN);
    // rows: 0 "api/", 1 a, 2 b, 3 "web/", 4 c
    click(&mut app, a.list.x + 2, a.list.y); // header: ignored
    assert_eq!(app.stored_sel, 0);
    click(&mut app, a.list.x + 2, a.list.y + 2);
    assert_eq!(app.stored_sel, 1);
    assert_eq!(app.view().request.url, "http://x/b.toml");
    click(&mut app, a.list.x + 2, a.list.y + 4);
    assert_eq!(app.stored_sel, 2);
    click(&mut app, a.list.x + 2, a.list.y + 4); // again: load
    assert_eq!(app.focus, Focus::Url);
    assert_eq!(app.editor_request().url, "http://x/c.toml");
}

#[tokio::test]
async fn long_history_list_clicks_hit_the_rows_that_are_drawn() {
    let mut h = harness().await;
    for i in 0..40 {
        let req = Request {
            url: format!("http://x/n{i:02}"),
            ..Default::default()
        };
        h.store.record(&req, &response_outcome()).await.unwrap();
    }
    h.app.refresh_history();
    pump(&mut h, |a| a.history.len() == 40).await;
    h.app.set_tab(Tab::History);
    for _ in 0..39 {
        h.app.handle_key(key(KeyCode::Char('j'))); // newest first: bottom = n00
    }
    assert_eq!(h.app.history_sel, 39);

    let a = ui::areas(SCREEN);
    let drawn = text(&render(&h.app, 120, 30));
    // list height 26, selection last: rows 14..=39 are visible, so row 14 (n25) is on top
    assert!(
        drawn.contains("http://x/n25") && !drawn.contains("http://x/n26"),
        "{drawn}"
    );

    click(&mut h.app, a.list.x + 2, a.list.y);
    assert_eq!(h.app.history_sel, 14);
    assert_eq!(h.app.selected_summary().unwrap().url, "http://x/n25");
}

fn response_outcome() -> Outcome {
    Outcome::Response(ok_response())
}

#[tokio::test]
async fn clicking_method_cycles_and_right_click_goes_back() {
    let mut h = harness().await;
    let a = ui::areas(SCREEN);
    click(&mut h.app, a.method.x + 2, a.method.y + 1);
    assert_eq!(h.app.method, Method::Post);
    assert_eq!(h.app.focus, Focus::Url);
    h.app.handle_mouse(
        mouse(
            MouseEventKind::Down(MouseButton::Right),
            a.method.x + 2,
            a.method.y + 1,
        ),
        SCREEN,
    );
    assert_eq!(h.app.method, Method::Get);
}

#[tokio::test]
async fn clicking_url_places_the_cursor() {
    let mut h = harness().await;
    h.app.load_request(&Request {
        url: "abcdef".into(),
        ..Default::default()
    });
    let a = ui::areas(SCREEN);
    let inner = ui::inner(a.url);
    click(&mut h.app, inner.x + 2, inner.y);
    assert_eq!(h.app.focus, Focus::Url);
    type_text(&mut h.app, "X");
    assert_eq!(h.app.url_text(), "abXcdef");
}

#[tokio::test]
async fn clicking_body_focuses_and_places_the_cursor() {
    let mut h = harness().await;
    h.app.load_request(&Request {
        body: Some("{\n  \"a\": 1\n}".into()),
        ..Default::default()
    });
    let a = ui::areas(SCREEN);
    let inner = ui::inner(a.body);
    click(&mut h.app, inner.x + 3, inner.y + 1); // after `  "`... column 3 of line 2
    assert_eq!(h.app.focus, Focus::Body);
    type_text(&mut h.app, "Z");
    assert_eq!(h.app.body_text(), "{\n  \"Za\": 1\n}");
}

#[tokio::test]
async fn clicking_the_editor_while_previewing_loads_the_hovered_entry() {
    let mut h = with_two_history_entries().await;
    pump(&mut h, |a| a.view().previewing).await;
    let a = ui::areas(SCREEN);
    let inner = ui::inner(a.body);

    click(&mut h.app, inner.x, inner.y);
    assert_eq!(h.app.focus, Focus::Body);
    assert_eq!(h.app.editor_request().url, "http://x/second");
    assert_eq!(
        h.app.body_text(),
        "{\"k\": true}",
        "the previewed body is now editable"
    );
}

#[tokio::test]
async fn wheel_moves_list_selection_and_scrolls_response() {
    let mut h = with_two_history_entries().await;
    let a = ui::areas(SCREEN);
    h.app.handle_mouse(
        mouse(MouseEventKind::ScrollDown, a.list.x + 1, a.list.y),
        SCREEN,
    );
    assert_eq!(h.app.history_sel, 1);
    h.app.handle_mouse(
        mouse(MouseEventKind::ScrollUp, a.list.x + 1, a.list.y),
        SCREEN,
    );
    assert_eq!(h.app.history_sel, 0);

    h.app.handle_mouse(
        mouse(
            MouseEventKind::ScrollDown,
            a.response_body.x + 1,
            a.response_body.y + 1,
        ),
        SCREEN,
    );
    assert_eq!(h.app.scroll, 3);
    h.app.handle_mouse(
        mouse(
            MouseEventKind::ScrollUp,
            a.response_body.x + 1,
            a.response_body.y + 1,
        ),
        SCREEN,
    );
    assert_eq!(h.app.scroll, 0);
}

#[test]
fn hit_areas_match_the_layout() {
    let a = ui::areas(SCREEN);
    let [left, center, right] = ui::panel_areas(Rect::new(0, 0, 120, 29));
    assert!(left.contains(a.tabs.as_position()) && left.contains(a.list.as_position()));
    assert!(center.contains(a.method.as_position()) && center.contains(a.body.as_position()));
    assert!(right.contains(a.response_body.as_position()));
}

// ---- copy ------------------------------------------------------------------

#[tokio::test]
async fn y_copies_the_pretty_response_body_without_sending() {
    let mut h = with_two_history_entries().await;
    pump(&mut h, |a| a.view().previewing).await;
    h.app.handle_key(key(KeyCode::Char('j'))); // older entry: JSON response {"x":1}
    pump(&mut h, |a| a.view().request.url == "http://x/first").await;

    h.app.handle_key(key(KeyCode::Char('y')));
    assert_eq!(h.app.take_clipboard().as_deref(), Some("{\n  \"x\": 1\n}"));
    assert!(h.app.notice.as_deref().unwrap().contains("copied"));
    assert!(h.app.take_clipboard().is_none(), "taken once");
    assert!(h.fake.requests().is_empty());

    let out = text(&render(&h.app, 120, 30));
    assert!(out.contains("copied"), "{out}");
    h.app.handle_key(key(KeyCode::Char('k')));
    assert!(h.app.notice.is_none(), "notice clears on the next key");
}

#[tokio::test]
async fn copy_error_message_and_nothing_to_copy() {
    let mut h = with_two_history_entries().await;
    pump(&mut h, |a| a.view().previewing).await; // newest = error "boom"
    h.app.handle_key(key(KeyCode::Char('y')));
    assert_eq!(h.app.take_clipboard().as_deref(), Some("boom"));

    let mut empty = harness().await; // no response yet
    empty.app.handle_key(key(KeyCode::Char('y')));
    assert!(empty.app.take_clipboard().is_none());
    assert_eq!(empty.app.notice.as_deref(), Some("nothing to copy"));
}

#[tokio::test]
async fn right_click_on_response_body_copies() {
    let mut h = with_two_history_entries().await;
    h.app.handle_key(key(KeyCode::Char('j')));
    pump(&mut h, |a| a.view().request.url == "http://x/first").await;
    let a = ui::areas(SCREEN);
    h.app.handle_mouse(
        mouse(
            MouseEventKind::Down(MouseButton::Right),
            a.response_body.x + 2,
            a.response_body.y + 1,
        ),
        SCREEN,
    );
    assert_eq!(h.app.take_clipboard().as_deref(), Some("{\n  \"x\": 1\n}"));
}

#[tokio::test]
async fn y_in_the_editor_types_a_y() {
    let mut h = harness().await;
    h.app.handle_key(key(KeyCode::Tab));
    type_text(&mut h.app, "y");
    assert_eq!(h.app.url_text(), "y");
    assert!(h.app.take_clipboard().is_none());
}

// ---- headers tab -------------------------------------------------------------

use hozd::tui::RequestTab;

fn header_lines(app: &App) -> Vec<String> {
    hozd::tui::headers::to_lines(&app.editor_request().headers)
}

#[tokio::test]
async fn ctrl_t_toggles_between_body_and_headers() {
    let mut h = harness().await;
    assert_eq!(h.app.request_tab, RequestTab::Body);
    h.app.handle_key(key(KeyCode::Tab));
    h.app.handle_key(key(KeyCode::Tab));
    assert_eq!(h.app.focus, Focus::Body);

    h.app.handle_key(ctrl('t'));
    assert_eq!(
        (h.app.request_tab, h.app.focus),
        (RequestTab::Headers, Focus::Headers)
    );
    let out = text(&render(&h.app, 120, 30));
    assert!(out.contains("Body") && out.contains("Headers"), "{out}");

    h.app.handle_key(ctrl('t'));
    assert_eq!(
        (h.app.request_tab, h.app.focus),
        (RequestTab::Body, Focus::Body)
    );
}

#[tokio::test]
async fn focus_cycle_follows_the_active_tab() {
    let mut h = harness().await;
    h.app.handle_key(ctrl('t')); // from the list: tab changes, focus stays
    assert_eq!(
        (h.app.request_tab, h.app.focus),
        (RequestTab::Headers, Focus::List)
    );
    h.app.handle_key(key(KeyCode::Tab));
    h.app.handle_key(key(KeyCode::Tab));
    assert_eq!(h.app.focus, Focus::Headers);
    h.app.handle_key(key(KeyCode::Tab));
    assert_eq!(h.app.focus, Focus::List);
    h.app.handle_key(key(KeyCode::BackTab));
    assert_eq!(h.app.focus, Focus::Headers);
}

#[tokio::test]
async fn add_headers_by_typing_and_they_are_sent_and_recorded() {
    let mut h = harness().await;
    h.app.handle_key(key(KeyCode::Tab));
    type_text(&mut h.app, "http://x/h");
    h.app.handle_key(key(KeyCode::Tab));
    h.app.handle_key(ctrl('t'));
    type_text(&mut h.app, "X-A: 1");
    h.app.handle_key(key(KeyCode::Enter)); // a new line appends another header
    type_text(&mut h.app, "Authorization: Bearer a:b");

    assert_eq!(
        header_lines(&h.app),
        ["X-A: 1", "Authorization: Bearer a:b"]
    );
    assert!(text(&render(&h.app, 120, 30)).contains("Headers (2)"));

    h.app.handle_key(ctrl('s'));
    pump(&mut h, |a| a.live.is_some() && !a.history.is_empty()).await;
    assert_eq!(
        h.fake.requests()[0].headers,
        vec![
            Header::new("X-A", "1"),
            Header::new("Authorization", "Bearer a:b")
        ]
    );
    let id = h.app.history[0].id;
    let entry = h.store.get(id).await.unwrap().unwrap();
    assert_eq!(
        entry.request.headers.len(),
        2,
        "headers are part of history"
    );
}

fn request_with_headers() -> Request {
    Request {
        url: "http://x/t".into(),
        headers: vec![Header::new("X-A", "1"), Header::new("X-B", "2")],
        ..Default::default()
    }
}

#[tokio::test]
async fn edit_and_remove_existing_headers() {
    let mut h = harness().await;
    h.app.load_request(&request_with_headers());
    h.app.handle_key(ctrl('t'));
    h.app.handle_key(key(KeyCode::Tab)); // list -> url
    h.app.handle_key(key(KeyCode::Tab)); // url -> headers
    assert_eq!(h.app.focus, Focus::Headers);
    assert_eq!(header_lines(&h.app), ["X-A: 1", "X-B: 2"]);

    // edit: change the value on the first line
    h.app.handle_key(key(KeyCode::End));
    h.app.handle_key(key(KeyCode::Backspace));
    type_text(&mut h.app, "9");
    assert_eq!(header_lines(&h.app), ["X-A: 9", "X-B: 2"]);

    // remove: clear the first line (Ctrl+K deletes to the end of the line)
    h.app.handle_key(key(KeyCode::Home));
    h.app.handle_key(ctrl('k'));
    assert_eq!(
        header_lines(&h.app),
        ["X-B: 2"],
        "a blank line is not a header"
    );
    assert!(text(&render(&h.app, 120, 30)).contains("Headers (1)"));
}

#[tokio::test]
async fn invalid_header_line_blocks_send_and_points_at_the_line() {
    let mut h = harness().await;
    h.app.handle_key(key(KeyCode::Tab));
    type_text(&mut h.app, "http://x");
    h.app.handle_key(key(KeyCode::Tab));
    h.app.handle_key(ctrl('t'));
    type_text(&mut h.app, "X-A: 1");
    h.app.handle_key(key(KeyCode::Enter));
    type_text(&mut h.app, "oops");
    h.app.handle_key(key(KeyCode::Tab)); // leave the editor
    h.app.handle_key(key(KeyCode::Tab));
    h.app.handle_key(ctrl('t')); // back to Body to prove the failure switches tabs
    assert_eq!(h.app.request_tab, RequestTab::Body);

    h.app.handle_key(ctrl('s'));
    assert!(!h.app.sending);
    assert!(h.fake.requests().is_empty());
    assert_eq!(
        (h.app.request_tab, h.app.focus),
        (RequestTab::Headers, Focus::Headers)
    );
    let out = text(&render(&h.app, 120, 30));
    assert!(
        out.contains("header line 2") || out.contains("line 2"),
        "{out}"
    );
}

#[tokio::test]
async fn headers_tab_previews_hovered_history_and_loads_on_enter() {
    let mut h = harness().await;
    h.store
        .record(&request_with_headers(), &response_outcome())
        .await
        .unwrap();
    h.app.refresh_history();
    pump(&mut h, |a| a.history.len() == 1).await;
    h.app.set_tab(Tab::History);
    h.app.handle_key(ctrl('t')); // list focus: only the tab changes
    pump(&mut h, |a| a.view().previewing).await;

    let out = text(&render(&h.app, 120, 30));
    assert!(
        out.contains("X-A: 1") && out.contains("X-B: 2") && out.contains("Headers (2)"),
        "{out}"
    );
    assert!(h.fake.requests().is_empty());

    h.app.handle_key(key(KeyCode::Enter));
    assert_eq!(header_lines(&h.app), ["X-A: 1", "X-B: 2"]);
}

#[tokio::test]
async fn clicking_the_tab_titles_switches_tabs_and_focuses_the_editor() {
    let mut h = harness().await;
    let a = ui::areas(SCREEN);
    // title row is the top border; " Body " = 6 columns after the corner, then `|`, then " Headers "
    click(&mut h.app, a.body.x + 1 + 8, a.body.y);
    assert_eq!(
        (h.app.request_tab, h.app.focus),
        (RequestTab::Headers, Focus::Headers)
    );
    click(&mut h.app, a.body.x + 1 + 2, a.body.y);
    assert_eq!(
        (h.app.request_tab, h.app.focus),
        (RequestTab::Body, Focus::Body)
    );
    click(&mut h.app, a.body.x + 1 + 6, a.body.y); // the divider
    assert_eq!(h.app.request_tab, RequestTab::Body);
}

#[tokio::test]
async fn clicking_the_headers_editor_places_the_cursor() {
    let mut h = harness().await;
    h.app.load_request(&request_with_headers());
    h.app.handle_key(ctrl('t'));
    let a = ui::areas(SCREEN);
    let inner = ui::inner(a.body);
    click(&mut h.app, inner.x + 4, inner.y + 1); // line 2, after `X-B:`
    assert_eq!(h.app.focus, Focus::Headers);
    type_text(&mut h.app, "Z");
    assert_eq!(h.app.headers.lines(), ["X-A: 1", "X-B:Z 2"]);
}

#[tokio::test]
async fn clicking_a_tab_title_while_previewing_loads_the_entry_first() {
    let mut h = with_two_history_entries().await;
    pump(&mut h, |a| a.view().previewing).await;
    let a = ui::areas(SCREEN);
    click(&mut h.app, a.body.x + 1 + 8, a.body.y);
    assert_eq!(h.app.editor_request().url, "http://x/second");
    assert_eq!(h.app.request_tab, RequestTab::Headers);
}
