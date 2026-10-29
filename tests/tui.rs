use std::sync::Arc;
use std::time::Duration;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
mod support;

use hozd::client::{FakeClient, FakeGrpc, Header, HttpRequest, HttpResponse, Method, Request};
use hozd::history::{History, Outcome};
use hozd::project::load_projects;
use hozd::tui::log::Level;
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
    grpc: Arc<FakeGrpc>,
    store: History,
    _tmp: tempfile::TempDir,
}

fn ok_response() -> HttpResponse {
    HttpResponse {
        status: 200,
        headers: vec![Header::new("x-h", "1")],
        body: "{\"x\":1}".into(),
        elapsed: Duration::from_millis(7),
    }
}

async fn harness_with(
    reply: Result<HttpResponse, hozd::client::ClientError>,
    projects: Option<&str>,
) -> Harness {
    harness_full(reply, projects, FakeGrpc::default()).await
}

async fn harness_full(
    reply: Result<HttpResponse, hozd::client::ClientError>,
    projects: Option<&str>,
    grpc: FakeGrpc,
) -> Harness {
    let tmp = tempfile::tempdir().unwrap();
    let store = History::open(&tmp.path().join("db.sqlite3")).await.unwrap();
    let fake = Arc::new(FakeClient::new(reply));
    let grpc = Arc::new(grpc);
    let (tx, rx) = unbounded_channel();
    let loaded = projects
        .map(|_| load_projects(&tmp.path().join("projects")))
        .unwrap_or_default();
    let app = App::new(fake.clone(), grpc.clone(), store.clone(), loaded, tx);
    Harness {
        app,
        rx,
        fake,
        grpc,
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
async fn left_panel_splits_50_50_with_logs_below() {
    let mut h = harness().await;
    let out = text(&render(&h.app, 120, 30));
    assert!(out.contains("Requests") && out.contains("Logs") && out.contains("ready"));
    h.app.log.push(Level::Error, "boom");
    assert!(text(&render(&h.app, 120, 30)).contains("boom"));
    // 29 main rows: requests take 15 (2 borders + tab row leave 12 for the list)
    assert_eq!(ui::areas(SCREEN).list.height, 12);
}

#[tokio::test]
async fn ctrl_e_and_a_click_open_the_log_in_the_reading_panel() {
    let mut h = harness().await;
    for i in 0..60 {
        h.app.log.push(Level::Info, format!("event {i}"));
    }
    h.app
        .handle_key(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::CONTROL));
    let out = text(&render(&h.app, 120, 30));
    assert!(out.contains(" Logs ") && out.contains("event 59"), "{out}");
    assert!(!out.contains("event 0\n"), "{out}");
    h.app.handle_key(key(KeyCode::Esc));
    assert!(h.app.modal.is_none());

    let a = ui::areas(SCREEN);
    click(&mut h.app, a.logs.x + 2, a.logs.y + 2);
    assert!(h.app.modal.is_some());
}

#[tokio::test]
async fn dragging_in_the_reading_panel_selects_and_copies() {
    use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    let mut h = harness().await;
    h.app.log.push(Level::Info, "alpha beta gamma");
    h.app
        .handle_key(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::CONTROL));
    render(&h.app, 120, 30); // sets the viewport
    let view = ui::inner(ui::modal_area(SCREEN));
    let mouse = |kind, x, y| MouseEvent {
        kind,
        column: x,
        row: y,
        modifiers: KeyModifiers::NONE,
    };
    // the last logged line ("alpha beta gamma") sits after "HH:MM:SS INFO  " (15 cols)
    let rows = h.app.log.len() as u16;
    let y = view.y + rows - 1;
    let start = view.x + 15;
    h.app.handle_mouse(
        mouse(MouseEventKind::Down(MouseButton::Left), start, y),
        SCREEN,
    );
    h.app.handle_mouse(
        mouse(MouseEventKind::Drag(MouseButton::Left), start + 4, y),
        SCREEN,
    );
    assert!(text(&render(&h.app, 120, 30)).contains("alpha"));
    h.app.handle_mouse(
        mouse(MouseEventKind::Up(MouseButton::Left), start + 4, y),
        SCREEN,
    );
    assert_eq!(h.app.take_clipboard().as_deref(), Some("alpha"));
    assert!(h.app.modal.is_some());

    // a plain click copies nothing
    h.app.handle_mouse(
        mouse(MouseEventKind::Down(MouseButton::Left), start, y),
        SCREEN,
    );
    h.app.handle_mouse(
        mouse(MouseEventKind::Up(MouseButton::Left), start, y),
        SCREEN,
    );
    assert_eq!(h.app.take_clipboard(), None);
}

#[tokio::test]
async fn the_schema_file_input_suggests_paths_and_tab_accepts() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("demo.proto"), "").unwrap();
    std::fs::write(dir.path().join("notes.txt"), "").unwrap();
    std::fs::create_dir(dir.path().join("deps")).unwrap();
    let root = dir.path().to_str().unwrap().to_owned();

    let mut h = harness().await;
    h.app.handle_key(ctrl('g'));
    h.app.handle_key(ctrl('o')); // file mode, focus on the path input
    assert_eq!(h.app.focus, Focus::Schema);
    type_text(&mut h.app, &format!("{root}/d"));

    let shown: Vec<&str> = h
        .app
        .suggestions
        .items
        .iter()
        .map(|s| s.text.as_str())
        .collect();
    assert_eq!(
        shown,
        [format!("{root}/deps/"), format!("{root}/demo.proto")]
    );
    assert!(text(&render(&h.app, 120, 30)).contains("demo.proto"));

    // Down moves onto the list, Tab accepts that one instead of leaving the input
    h.app.handle_key(key(KeyCode::Down));
    h.app.handle_key(key(KeyCode::Down));
    h.app.handle_key(key(KeyCode::Tab));
    assert_eq!(h.app.schema_path_text(), format!("{root}/demo.proto"));
    assert_eq!(h.app.focus, Focus::Schema);
    assert!(
        h.app.suggestions.items.is_empty(),
        "nothing left to suggest"
    );

    // with no suggestions Tab moves on as usual
    h.app.handle_key(key(KeyCode::Tab));
    assert_ne!(h.app.focus, Focus::Schema);

    // leaving the input hides the list
    h.app.handle_key(key(KeyCode::BackTab));
    type_text(&mut h.app, "x");
    h.app.handle_key(key(KeyCode::Esc));
    assert!(h.app.suggestions.items.is_empty());
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
        Arc::new(FakeGrpc::default()),
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
        let req = Request::Http(HttpRequest {
            method: Method::Post,
            url: url.into(),
            headers: vec![],
            body: Some("{\"k\": true}".into()),
        });
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
        Arc::new(FakeGrpc::default()),
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
async fn clicking_empty_list_space_changes_nothing_and_the_response_box_takes_focus() {
    let mut h = with_two_history_entries().await;
    let a = ui::areas(SCREEN);
    click(&mut h.app, a.list.x + 1, a.list.y + 10); // below the two rows
    assert_eq!(
        (h.app.history_sel, h.app.focus, h.app.tab),
        (0, Focus::List, Tab::History)
    );

    click(&mut h.app, a.response_body.x + 1, a.response_body.y + 1);
    assert_eq!(
        (h.app.history_sel, h.app.focus, h.app.tab),
        (0, Focus::Response, Tab::History)
    );
    assert!(h.app.modal.is_none(), "one click only focuses");
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
        Arc::new(FakeGrpc::default()),
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
        let req = Request::Http(HttpRequest {
            url: format!("http://x/n{i:02}"),
            ..Default::default()
        });
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
    // selection last: the top visible row is 40 - list height
    let top = 40 - a.list.height as usize;
    let top_url = format!("http://x/n{:02}", 39 - top);
    let above_url = format!("http://x/n{:02}", 39 - top + 1);
    assert!(
        drawn.contains(&top_url) && !drawn.contains(&above_url),
        "{drawn}"
    );

    click(&mut h.app, a.list.x + 2, a.list.y);
    assert_eq!(h.app.history_sel, top);
    assert_eq!(h.app.selected_summary().unwrap().url, top_url);
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
    h.app.load_request(&Request::Http(HttpRequest {
        url: "abcdef".into(),
        ..Default::default()
    }));
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
    h.app.load_request(&Request::Http(HttpRequest {
        body: Some("{\n  \"a\": 1\n}".into()),
        ..Default::default()
    }));
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
    assert_eq!(h.app.focus, Focus::Response);
    h.app.handle_key(key(KeyCode::Tab));
    assert_eq!(h.app.focus, Focus::List);
    h.app.handle_key(key(KeyCode::BackTab));
    assert_eq!(h.app.focus, Focus::Response);
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
    type_text(&mut h.app, "X-A");
    h.app.handle_key(key(KeyCode::Enter)); // name -> value
    type_text(&mut h.app, "1");
    h.app.handle_key(key(KeyCode::Enter)); // the last value: appends another header
    type_text(&mut h.app, "Authorization");
    h.app.handle_key(key(KeyCode::Enter));
    type_text(&mut h.app, "Bearer a:b");

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
    let Request::Http(request) = entry.request else {
        panic!("expected an HTTP request");
    };
    assert_eq!(request.headers.len(), 2, "headers are part of history");
}

fn request_with_headers() -> Request {
    Request::Http(HttpRequest {
        url: "http://x/t".into(),
        headers: vec![Header::new("X-A", "1"), Header::new("X-B", "2")],
        ..Default::default()
    })
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

    // the cursor starts on the empty row after the existing headers
    h.app.handle_key(key(KeyCode::Up));
    h.app.handle_key(key(KeyCode::Up));
    h.app.handle_key(key(KeyCode::Enter)); // name -> value
    h.app.handle_key(key(KeyCode::Backspace));
    type_text(&mut h.app, "9");
    assert_eq!(header_lines(&h.app), ["X-A: 9", "X-B: 2"]);

    // remove the row
    h.app.handle_key(ctrl('d'));
    assert_eq!(header_lines(&h.app), ["X-B: 2"]);
    assert!(text(&render(&h.app, 120, 30)).contains("Headers (1)"));
}

#[tokio::test]
async fn invalid_header_line_blocks_send_and_points_at_the_line() {
    let mut h = harness().await;
    h.app.handle_key(key(KeyCode::Tab));
    type_text(&mut h.app, "http://x");
    h.app.handle_key(key(KeyCode::Tab));
    h.app.handle_key(ctrl('t'));
    type_text(&mut h.app, "X-A");
    h.app.handle_key(key(KeyCode::Enter));
    type_text(&mut h.app, "1");
    h.app.handle_key(key(KeyCode::Enter));
    type_text(&mut h.app, "bad name"); // a space is not allowed in a name
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
    assert!(out.contains("header 2"), "{out}");
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
    let grid = ui::header_grid(inner, h.app.headers.rows.len(), h.app.headers.row);
    let second = grid.rows[1];
    click(&mut h.app, second.name.x + 3, second.name.y); // after `X-B`
    assert_eq!(h.app.focus, Focus::Headers);
    type_text(&mut h.app, "Z");
    assert_eq!(h.app.headers.lines(), ["X-A: 1", "X-BZ: 2"]);

    click(&mut h.app, second.value.x, second.value.y);
    type_text(&mut h.app, "9");
    assert_eq!(h.app.headers.lines(), ["X-A: 1", "X-BZ: 92"]);
}

#[tokio::test]
async fn header_inputs_are_side_by_side_and_navigable_and_can_be_appended() {
    let mut h = harness().await;
    h.app.load_request(&request_with_headers());
    h.app.handle_key(ctrl('t'));
    h.app.handle_key(key(KeyCode::Tab));
    h.app.handle_key(key(KeyCode::Tab));
    let out = text(&render(&h.app, 120, 30));
    let line = out.lines().find(|l| l.contains("X-A")).unwrap();
    assert!(
        line.contains("X-A") && line.find(": ") < line.find('1'),
        "{line}"
    );
    assert!(out.contains("+ add header"), "{out}");

    // Right at the end of a name enters the value; Left at its start goes back
    h.app.handle_key(key(KeyCode::Up));
    h.app.handle_key(key(KeyCode::Up));
    h.app.handle_key(key(KeyCode::Right));
    type_text(&mut h.app, "!");
    h.app.handle_key(key(KeyCode::Left));
    h.app.handle_key(key(KeyCode::Left));
    h.app.handle_key(key(KeyCode::Left));
    type_text(&mut h.app, "-");
    assert_eq!(h.app.headers.lines()[0], "X--A: !1");

    // a click on the add line appends a row
    let a = ui::areas(SCREEN);
    let grid = ui::header_grid(
        ui::inner(a.body),
        h.app.headers.rows.len(),
        h.app.headers.row,
    );
    let add = grid.add.expect("room for the add line");
    let before = h.app.headers.rows.len();
    click(&mut h.app, add.x + 2, add.y);
    assert_eq!(h.app.headers.rows.len(), before + 1);
    assert_eq!(h.app.headers.row, before);
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

// ---- reading panel (modal) -----------------------------------------------------

async fn with_response(body: &str) -> Harness {
    let mut h = harness_with(
        Ok(HttpResponse {
            status: 200,
            headers: vec![],
            body: body.into(),
            elapsed: Duration::from_millis(1),
        }),
        None,
    )
    .await;
    h.app.handle_key(key(KeyCode::Tab));
    type_text(&mut h.app, "http://x/r");
    h.app.handle_key(ctrl('s'));
    pump(&mut h, |a| a.live.is_some() && !a.history.is_empty()).await;
    h
}

fn focus_response(app: &mut App) {
    app.handle_key(key(KeyCode::Esc)); // list
    for _ in 0..3 {
        app.handle_key(key(KeyCode::Tab)); // url, body, response
    }
    assert_eq!(app.focus, Focus::Response);
}

#[tokio::test]
async fn enter_on_the_focused_response_opens_the_reading_panel() {
    let mut h = with_response("{\"x\":1}").await;
    assert!(h.app.modal.is_none());
    focus_response(&mut h.app);

    h.app.handle_key(key(KeyCode::Enter));
    assert!(h.app.modal.is_some());
    let out = text(&render(&h.app, 120, 30));
    assert!(
        out.contains("Response body") && out.contains("JSON") && out.contains("{\"x\":1}"),
        "{out}"
    );
    assert!(out.contains("as received"), "opens unformatted");
}

#[tokio::test]
async fn f_formats_json_and_toggles_back() {
    let mut h = with_response("{\"x\":1}").await;
    focus_response(&mut h.app);
    h.app.handle_key(key(KeyCode::Enter));

    h.app.handle_key(key(KeyCode::Char('F')));
    let formatted = text(&render(&h.app, 120, 30));
    assert!(
        formatted.contains("formatted") && formatted.contains("\"x\": 1"),
        "{formatted}"
    );
    assert!(!formatted.contains("{\"x\":1}"));

    h.app.handle_key(key(KeyCode::Char('f')));
    let raw = text(&render(&h.app, 120, 30));
    assert!(
        raw.contains("{\"x\":1}") && raw.contains("as received"),
        "{raw}"
    );
}

#[tokio::test]
async fn f_formats_html() {
    let mut h = with_response("<html><body><p>hi</p></body></html>").await;
    focus_response(&mut h.app);
    h.app.handle_key(key(KeyCode::Enter));
    h.app.handle_key(key(KeyCode::Char('F')));
    let out = text(&render(&h.app, 120, 30));
    assert!(out.contains("HTML"), "{out}");
    assert_eq!(
        h.app.modal.as_ref().unwrap().displayed(),
        "<html>\n  <body>\n    <p>\n      hi\n    </p>\n  </body>\n</html>"
    );
}

#[tokio::test]
async fn f_on_other_text_says_there_is_nothing_to_format() {
    let mut h = with_response("just words").await;
    focus_response(&mut h.app);
    h.app.handle_key(key(KeyCode::Enter));
    h.app.handle_key(key(KeyCode::Char('F')));
    assert!(
        h.app
            .notice
            .as_deref()
            .unwrap()
            .contains("nothing to format")
    );
    assert_eq!(h.app.modal.as_ref().unwrap().displayed(), "just words");
}

#[tokio::test]
async fn double_click_opens_and_single_click_only_focuses() {
    let mut h = with_response("{\"x\":1}").await;
    let a = ui::areas(SCREEN);
    let (x, y) = (a.response_body.x + 2, a.response_body.y + 2);

    click(&mut h.app, x, y);
    assert_eq!(h.app.focus, Focus::Response);
    assert!(h.app.modal.is_none());
    click(&mut h.app, x, y);
    assert!(h.app.modal.is_some());
}

#[tokio::test]
async fn slow_clicks_are_not_a_double_click() {
    let mut h = with_response("{\"x\":1}").await;
    let a = ui::areas(SCREEN);
    let (x, y) = (a.response_body.x + 2, a.response_body.y + 2);
    click(&mut h.app, x, y);
    tokio::time::sleep(Duration::from_millis(500)).await;
    click(&mut h.app, x, y);
    assert!(h.app.modal.is_none());
}

#[tokio::test]
async fn nothing_to_open_without_a_response() {
    let mut h = harness().await;
    for _ in 0..3 {
        h.app.handle_key(key(KeyCode::Tab));
    }
    assert_eq!(h.app.focus, Focus::Response);
    h.app.handle_key(key(KeyCode::Enter));
    assert!(h.app.modal.is_none());
    assert_eq!(h.app.notice.as_deref(), Some("no response to open"));
}

#[tokio::test]
async fn esc_and_q_close_and_the_panel_swallows_other_input() {
    let mut h = with_response("{\"x\":1}").await;
    focus_response(&mut h.app);
    h.app.handle_key(key(KeyCode::Enter));

    h.app.handle_key(key(KeyCode::Tab));
    type_text(&mut h.app, "zzz");
    assert_eq!(
        h.app.focus,
        Focus::Response,
        "focus does not move behind the panel"
    );
    assert_eq!(
        h.app.url_text(),
        "http://x/r",
        "nothing is typed behind the panel"
    );
    assert!(!h.app.should_quit);

    h.app.handle_key(key(KeyCode::Esc));
    assert!(h.app.modal.is_none());
    h.app.handle_key(key(KeyCode::Enter));
    h.app.handle_key(key(KeyCode::Char('q')));
    assert!(h.app.modal.is_none() && !h.app.should_quit);

    h.app.handle_key(key(KeyCode::Enter));
    h.app.handle_key(ctrl('c'));
    assert!(h.app.should_quit, "Ctrl+C still quits");
}

#[tokio::test]
async fn long_bodies_scroll_with_keys_and_wheel() {
    let body: String = (0..200).map(|i| format!("line {i:03}\n")).collect();
    let mut h = with_response(&body).await;
    focus_response(&mut h.app);
    h.app.handle_key(key(KeyCode::Enter));
    let _ = render(&h.app, 120, 30); // the renderer tells the panel its size

    assert!(text(&render(&h.app, 120, 30)).contains("line 000"));
    h.app.handle_key(key(KeyCode::PageDown));
    let page = text(&render(&h.app, 120, 30));
    assert!(
        !page.contains("line 000") && page.contains("line 030"),
        "{page}"
    );

    h.app.handle_key(key(KeyCode::End));
    let end = text(&render(&h.app, 120, 30));
    assert!(
        end.contains("line 199") && !end.contains("line 100"),
        "{end}"
    );
    h.app.handle_key(key(KeyCode::Down));
    assert!(
        text(&render(&h.app, 120, 30)).contains("line 199"),
        "stops at the end"
    );

    h.app.handle_key(key(KeyCode::Home));
    assert!(text(&render(&h.app, 120, 30)).contains("line 000"));
    h.app
        .handle_mouse(mouse(MouseEventKind::ScrollDown, 50, 10), SCREEN);
    let wheel = text(&render(&h.app, 120, 30));
    assert!(
        !wheel.contains("line 000") && wheel.contains("line 003"),
        "{wheel}"
    );
}

#[tokio::test]
async fn long_lines_wrap_instead_of_running_off_screen() {
    let long = format!("{{\"k\":\"{}\"}}", "ab".repeat(120));
    let mut h = with_response(&long).await;
    focus_response(&mut h.app);
    h.app.handle_key(key(KeyCode::Enter));
    let out = text(&render(&h.app, 120, 30));
    assert!(
        out.matches("abab").count() >= 2,
        "wrapped over several rows: {out}"
    );
    assert!(out.contains("\"}"), "the end of the line is visible");
}

#[tokio::test]
async fn y_in_the_panel_copies_what_is_shown() {
    let mut h = with_response("{\"x\":1}").await;
    focus_response(&mut h.app);
    h.app.handle_key(key(KeyCode::Enter));
    h.app.handle_key(key(KeyCode::Char('y')));
    assert_eq!(h.app.take_clipboard().as_deref(), Some("{\"x\":1}"));
    h.app.handle_key(key(KeyCode::Char('F')));
    h.app.handle_key(key(KeyCode::Char('y')));
    assert_eq!(h.app.take_clipboard().as_deref(), Some("{\n  \"x\": 1\n}"));
}

#[tokio::test]
async fn clicking_outside_the_panel_closes_it() {
    let mut h = with_response("{\"x\":1}").await;
    focus_response(&mut h.app);
    h.app.handle_key(key(KeyCode::Enter));
    click(&mut h.app, 50, 10); // inside: stays open
    assert!(h.app.modal.is_some());
    click(&mut h.app, 0, 0); // outside the margin
    assert!(h.app.modal.is_none());
}

#[tokio::test]
async fn hovered_history_entry_can_be_opened_from_the_response_box() {
    let mut h = with_two_history_entries().await;
    pump(&mut h, |a| a.view().previewing).await;
    h.app.handle_key(key(KeyCode::Char('j'))); // the older entry: JSON response
    pump(&mut h, |a| a.view().request.url == "http://x/first").await;

    let a = ui::areas(SCREEN);
    let (x, y) = (a.response_body.x + 2, a.response_body.y + 2);
    click(&mut h.app, x, y);
    assert_eq!(h.app.focus, Focus::Response);
    assert!(h.app.view().previewing, "the hovered entry stays on screen");
    assert_eq!(h.app.view().request.url, "http://x/first");

    click(&mut h.app, x, y);
    assert_eq!(h.app.modal.as_ref().unwrap().displayed(), "{\"x\":1}");
    assert!(h.fake.requests().is_empty());
}

#[tokio::test]
async fn error_messages_open_too() {
    let mut h = with_two_history_entries().await;
    pump(&mut h, |a| a.view().previewing).await; // newest = error "boom"
    h.app.handle_key(key(KeyCode::Tab)); // url ... the preview is replaced by the editor
    h.app.handle_key(key(KeyCode::Esc));
    h.app.handle_key(key(KeyCode::Tab));
    h.app.handle_key(key(KeyCode::Tab));
    h.app.handle_key(key(KeyCode::Tab));
    assert_eq!(h.app.focus, Focus::Response);
    // from the editor the live response (none yet) is shown, so nothing opens
    h.app.handle_key(key(KeyCode::Enter));
    assert!(h.app.modal.is_none());

    h.app.handle_key(key(KeyCode::Esc));
    let a = ui::areas(SCREEN);
    let (x, y) = (a.response_body.x + 2, a.response_body.y + 2);
    click(&mut h.app, x, y);
    click(&mut h.app, x, y);
    assert_eq!(h.app.modal.as_ref().unwrap().displayed(), "boom");
}

// ---- gRPC mode: kind toggle and loading ------------------------------------------

use hozd::client::{GrpcRequest, Kind, SchemaSource};
use ratatui_textarea::TextArea;

fn grpc_request() -> GrpcRequest {
    GrpcRequest {
        address: "https://api.example.com:443".into(),
        method: "user.v1.UserService/GetUser".into(),
        schema: SchemaSource::File("api/user.proto".into()),
        insecure: true,
        metadata: vec![Header::new("authorization", "Bearer t")],
        body: Some("{\n  \"id\": 7\n}".into()),
    }
}

fn lines(ta: &TextArea<'static>) -> Vec<String> {
    ta.lines().to_vec()
}

#[tokio::test]
async fn ctrl_g_switches_kind_keeping_address_body_and_headers() {
    let mut h = harness().await;
    h.app.load_request(&request_with_headers());
    h.app.body = TextArea::new(vec!["{\"a\": 1}".into()]);
    let (url, body, headers) = (h.app.url_text(), lines(&h.app.body), h.app.headers.lines());
    assert_eq!(h.app.kind, Kind::Http);

    h.app.handle_key(ctrl('g'));
    assert_eq!(h.app.kind, Kind::Grpc);
    assert_eq!(h.app.url_text(), url, "the URL becomes the address");
    assert_eq!(lines(&h.app.body), body);
    assert_eq!(h.app.headers.lines(), headers, "headers become metadata");
    assert_eq!(h.app.editor_grpc_request().address, "http://x/t");

    h.app.handle_key(ctrl('g'));
    assert_eq!(h.app.kind, Kind::Http);
    assert_eq!(h.app.url_text(), url);
}

#[tokio::test]
async fn clicking_the_kind_labels_switches_kind() {
    let mut h = harness().await;
    let url = ui::areas(SCREEN).url;
    // The labels sit right-aligned in the top border of the URL box: ` REST | gRPC `.
    let start = url.right() - 1 - " REST | gRPC ".len() as u16;

    click(&mut h.app, start + 9, url.y); // inside `gRPC`
    assert_eq!(h.app.kind, Kind::Grpc);
    click(&mut h.app, start + 9, url.y);
    assert_eq!(
        h.app.kind,
        Kind::Grpc,
        "clicking the active label changes nothing"
    );
    click(&mut h.app, start + 2, url.y); // inside `REST`
    assert_eq!(h.app.kind, Kind::Http);

    click(&mut h.app, start - 3, url.y); // the border left of the labels
    assert_eq!(h.app.kind, Kind::Http);
    assert_eq!(
        h.app.focus,
        Focus::Url,
        "that click is a plain click on the URL box"
    );
}

#[tokio::test]
async fn loading_a_grpc_request_switches_to_grpc_and_fills_every_field() {
    let mut h = harness().await;
    assert_eq!(h.app.kind, Kind::Http);

    h.app.load_request(&Request::Grpc(grpc_request()));

    assert_eq!(h.app.kind, Kind::Grpc);
    assert_eq!(h.app.url_text(), "https://api.example.com:443");
    assert_eq!(
        h.app.selected_method.as_deref(),
        Some("user.v1.UserService/GetUser")
    );
    assert_eq!(
        h.app.schema_source(),
        SchemaSource::File("api/user.proto".into())
    );
    assert!(h.app.insecure);
    assert_eq!(h.app.headers.lines(), ["authorization: Bearer t"]);
    assert_eq!(h.app.body_text(), "{\n  \"id\": 7\n}");
    assert_eq!(h.app.editor_grpc_request(), grpc_request());
}

#[tokio::test]
async fn loading_a_reflection_request_clears_the_file_mode() {
    let mut h = harness().await;
    h.app.load_request(&Request::Grpc(grpc_request()));
    let mut reflection = grpc_request();
    reflection.schema = SchemaSource::Reflection;
    h.app.load_request(&Request::Grpc(reflection));
    assert_eq!(h.app.schema_source(), SchemaSource::Reflection);
}

#[tokio::test]
async fn loading_an_http_request_switches_back_to_rest() {
    let mut h = harness().await;
    h.app.load_request(&Request::Grpc(grpc_request()));
    h.app.load_request(&request_with_headers());
    assert_eq!(h.app.kind, Kind::Http);
    assert_eq!(h.app.url_text(), "http://x/t");
}

#[tokio::test]
async fn activating_a_grpc_history_entry_loads_it_into_the_editor() {
    let mut h = harness().await;
    h.store
        .record(
            &Request::Grpc(grpc_request()),
            &Outcome::Grpc(hozd::client::GrpcOutcome::default()),
        )
        .await
        .unwrap();
    h.app.refresh_history();
    pump(&mut h, |a| a.history.len() == 1).await;
    h.app.set_tab(Tab::History);
    pump(&mut h, |a| a.view().request.kind == Kind::Grpc).await;

    h.app.handle_key(key(KeyCode::Enter));
    assert_eq!((h.app.kind, h.app.focus), (Kind::Grpc, Focus::Url));
    assert_eq!(h.app.editor_grpc_request(), grpc_request());
}

#[tokio::test]
async fn focus_ring_includes_schema_and_method_only_in_grpc_mode() {
    let mut h = harness().await;
    let ring = |app: &mut App| {
        let mut seen = vec![app.focus];
        loop {
            app.handle_key(key(KeyCode::Tab));
            if app.focus == seen[0] {
                return seen;
            }
            seen.push(app.focus);
        }
    };
    assert_eq!(
        ring(&mut h.app),
        [Focus::List, Focus::Url, Focus::Body, Focus::Response]
    );
    h.app.handle_key(ctrl('g'));
    assert_eq!(
        ring(&mut h.app),
        [
            Focus::List,
            Focus::Url,
            Focus::Schema,
            Focus::Method,
            Focus::Body,
            Focus::Response
        ]
    );
    // Shift+Tab walks it backwards.
    h.app
        .handle_key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT));
    assert_eq!(h.app.focus, Focus::Response);
    h.app
        .handle_key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT));
    assert_eq!(h.app.focus, Focus::Body);
    h.app
        .handle_key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT));
    assert_eq!(h.app.focus, Focus::Method);
}

// ---- gRPC editor rendering ---------------------------------------------------------

fn grpc_screen(app: &App) -> String {
    text(&render(app, 120, 30))
}

#[tokio::test]
async fn grpc_mode_shows_address_schema_method_and_metadata_boxes() {
    let mut h = harness().await;
    h.app.load_request(&Request::Grpc(GrpcRequest {
        address: "http://localhost:50051".into(),
        method: "user.v1.UserService/GetUser".into(),
        schema: SchemaSource::Reflection,
        metadata: vec![Header::new("a", "1"), Header::new("b", "2")],
        ..Default::default()
    }));
    let out = grpc_screen(&h.app);
    for expected in [
        "Address",
        "REST | gRPC",
        "TLS",
        "plain",
        "Schema",
        "reflection",
        "not loaded",
        "user.v1.UserService/GetUser",
        "Metadata (2)",
    ] {
        assert!(out.contains(expected), "missing `{expected}` in:\n{out}");
    }
    assert!(!out.contains("Headers ("), "{out}");
    assert!(!out.contains("NOT VERIFIED"), "{out}");
}

#[tokio::test]
async fn rest_mode_is_unchanged() {
    let mut h = harness().await;
    h.app.load_request(&request_with_headers());
    let out = grpc_screen(&h.app);
    for expected in ["URL", "Method", "Headers (2)", "REST | gRPC"] {
        assert!(out.contains(expected), "missing `{expected}` in:\n{out}");
    }
    for absent in ["Schema", "Address", "Metadata", "TLS"] {
        assert!(!out.contains(absent), "unexpected `{absent}` in:\n{out}");
    }
}

#[tokio::test]
async fn insecure_mode_is_flagged_on_the_address_row_for_tls_addresses_only() {
    let mut h = harness().await;
    h.app.load_request(&Request::Grpc(GrpcRequest {
        address: "https://api.example.com".into(),
        insecure: true,
        ..Default::default()
    }));
    let out = grpc_screen(&h.app);
    assert!(
        out.contains("NOT VERIFIED") && out.contains("insecure"),
        "{out}"
    );

    h.app.insecure = false;
    let out = grpc_screen(&h.app);
    assert!(
        out.contains("verified") && !out.contains("NOT VERIFIED"),
        "{out}"
    );

    // Plaintext has nothing to verify.
    h.app.insecure = true;
    h.app.url = TextArea::new(vec!["http://localhost:1".into()]);
    let out = grpc_screen(&h.app);
    assert!(
        out.contains("plain") && !out.contains("NOT VERIFIED"),
        "{out}"
    );
}

#[tokio::test]
async fn schema_file_mode_shows_the_editable_path_and_hint_when_empty() {
    let mut h = harness().await;
    h.app.handle_key(ctrl('g'));
    h.app.handle_key(ctrl('o'));
    assert_eq!(h.app.focus, Focus::Schema);
    type_text(&mut h.app, "api/user.proto");
    assert!(grpc_screen(&h.app).contains("api/user.proto"));
    assert_eq!(
        h.app.schema_source(),
        SchemaSource::File("api/user.proto".into())
    );

    h.app.handle_key(ctrl('o'));
    assert_eq!(h.app.schema_source(), SchemaSource::Reflection);
    h.app.handle_key(ctrl('o'));
    h.app.schema_path = TextArea::default();
    h.app.focus = Focus::List;
    assert!(grpc_screen(&h.app).contains("path to a .proto"));
}

#[tokio::test]
async fn hovering_a_grpc_history_entry_previews_it_even_in_rest_mode() {
    let mut h = harness().await;
    h.store
        .record(
            &Request::Grpc(grpc_request()),
            &Outcome::Grpc(hozd::client::GrpcOutcome::default()),
        )
        .await
        .unwrap();
    h.app.refresh_history();
    pump(&mut h, |a| a.history.len() == 1).await;
    h.app.set_tab(Tab::History);
    pump(&mut h, |a| a.view().request.kind == Kind::Grpc).await;

    assert_eq!(h.app.kind, Kind::Http, "only the panels preview");
    let out = grpc_screen(&h.app);
    for expected in [
        "Address (preview)",
        "https://api.example.com:443",
        "NOT VERIFIED",
        "file: api/user.proto",
        "Method (preview)",
    ] {
        assert!(out.contains(expected), "missing `{expected}` in:\n{out}");
    }
}

#[test]
fn grpc_layout_adds_the_schema_and_method_rows_and_rest_has_none() {
    let rest = ui::areas(SCREEN);
    assert_eq!((rest.schema.height, rest.rpc.height), (0, 0));

    let grpc = ui::areas_for(SCREEN, Kind::Grpc);
    assert_eq!((grpc.schema.height, grpc.rpc.height), (3, 3));
    assert_eq!(grpc.url.y, rest.url.y);
    assert_eq!(grpc.schema.y, grpc.url.bottom());
    assert_eq!(grpc.rpc.y, grpc.schema.bottom());
    assert_eq!(grpc.body.y, grpc.rpc.bottom());
    assert_eq!(grpc.body.bottom(), rest.body.bottom());
}

#[tokio::test]
async fn clicking_the_tls_box_toggles_insecure_and_metadata_title_switches_tabs() {
    let mut h = harness().await;
    h.app.handle_key(ctrl('g'));
    let a = ui::areas_for(SCREEN, Kind::Grpc);

    click(&mut h.app, a.method.x + 2, a.method.y + 1);
    assert!(h.app.insecure);
    h.app.handle_key(ctrl('k'));
    assert!(!h.app.insecure);

    // `Metadata` is the second title on the request box's top border.
    click(
        &mut h.app,
        a.body.x + 1 + " Body ".len() as u16 + 3,
        a.body.y,
    );
    assert_eq!(h.app.request_tab, hozd::tui::RequestTab::Headers);

    click(&mut h.app, a.schema.x + 3, a.schema.y + 1);
    assert_eq!(h.app.focus, Focus::Schema);
}

// ---- gRPC: schema loading ----------------------------------------------------------

use hozd::client::{CallEvent, FakeStep, GrpcError, GrpcOutcome, GrpcStatus, Schema};

fn fixture_schema() -> Schema {
    hozd::client::load_schema_file(&support::fixtures().join("descriptor.bin"), &[]).unwrap()
}

async fn grpc_harness(fake: FakeGrpc) -> Harness {
    let mut h = harness_full(Ok(ok_response()), None, fake).await;
    h.app.handle_key(ctrl('g'));
    h.app.url = TextArea::new(vec!["http://localhost:50051".into()]);
    h
}

fn ok_done() -> CallEvent {
    CallEvent::Done(GrpcOutcome {
        status: Some(GrpcStatus {
            code: 0,
            message: String::new(),
            trailers: vec![],
        }),
        ..Default::default()
    })
}

#[tokio::test]
async fn ctrl_l_loads_the_schema_and_shows_what_was_found() {
    let mut h = grpc_harness(FakeGrpc::new(Ok(fixture_schema()))).await;
    assert!(grpc_screen(&h.app).contains("not loaded"));

    h.app.handle_key(ctrl('l'));
    assert!(h.app.schema_loading);
    assert!(grpc_screen(&h.app).contains("loading..."));

    pump(&mut h, |a| !a.schema_loading).await;
    assert!(grpc_screen(&h.app).contains("1 service, 6 methods"));
    assert_eq!(h.app.notice.as_deref(), Some("loaded 1 service, 6 methods"));
    assert_eq!(
        h.grpc.loads(),
        [(
            "http://localhost:50051".to_owned(),
            SchemaSource::Reflection,
            false
        )]
    );
}

#[tokio::test]
async fn enter_in_the_schema_box_loads_it_too() {
    let mut h = grpc_harness(FakeGrpc::new(Ok(fixture_schema()))).await;
    h.app.focus = Focus::Schema;
    h.app.handle_key(key(KeyCode::Enter));
    pump(&mut h, |a| a.schema.is_some()).await;
}

#[tokio::test]
async fn a_failed_load_reports_the_error_and_keeps_the_previous_schema() {
    let mut h = grpc_harness(FakeGrpc::new(Ok(fixture_schema()))).await;
    h.app.handle_key(ctrl('l'));
    pump(&mut h, |a| a.schema.is_some()).await;

    let failing = FakeGrpc::new(Err(GrpcError::Schema(
        "the server does not support reflection".into(),
    )));
    let mut f = grpc_harness(failing).await;
    f.app.schema = h.app.schema.clone();
    f.app.handle_key(ctrl('l'));
    pump(&mut f, |a| a.status.is_some()).await;

    assert_eq!(
        f.app.status.as_deref(),
        Some("the server does not support reflection")
    );
    assert!(!f.app.schema_loading);
    assert!(f.app.schema.is_some(), "the picker keeps its methods");
    assert!(grpc_screen(&f.app).contains("the server does not support reflection"));
}

#[tokio::test]
async fn the_loaded_schema_is_dropped_when_address_or_source_change() {
    let mut h = grpc_harness(FakeGrpc::new(Ok(fixture_schema()))).await;
    h.app.handle_key(ctrl('l'));
    pump(&mut h, |a| a.schema.is_some()).await;
    assert!(h.app.current_schema().is_some());

    h.app.url = TextArea::new(vec!["http://other:1".into()]);
    assert!(h.app.current_schema().is_none());
    assert_eq!(h.app.schema_status(), "not loaded");

    h.app.url = TextArea::new(vec!["http://localhost:50051".into()]);
    assert!(
        h.app.current_schema().is_some(),
        "same address and source again"
    );

    h.app.handle_key(ctrl('o')); // schema file mode
    assert!(h.app.current_schema().is_none());
    h.app.handle_key(ctrl('o'));
    h.app.insecure = true;
    assert!(
        h.app.current_schema().is_none(),
        "insecure is part of the key"
    );
}

#[tokio::test]
async fn load_needs_an_address_or_a_path() {
    let mut h = grpc_harness(FakeGrpc::default()).await;
    h.app.url = TextArea::default();
    h.app.handle_key(ctrl('l'));
    assert_eq!(
        h.app.status.as_deref(),
        Some("enter the server address first")
    );
    assert!(!h.app.schema_loading);

    h.app.handle_key(ctrl('o'));
    h.app.handle_key(ctrl('l'));
    assert_eq!(
        h.app.status.as_deref(),
        Some("enter the path of a schema file first")
    );
    assert!(h.grpc.loads().is_empty());
}

#[tokio::test]
async fn sending_without_a_loaded_schema_loads_it_first_and_then_calls() {
    let fake = FakeGrpc::new(Ok(fixture_schema())).with_script(vec![FakeStep::Event(ok_done())]);
    let mut h = grpc_harness(fake).await;
    h.app.selected_method = Some("demo.v1.Demo/GetUser".into());
    h.app.body = TextArea::new(vec!["{\"id\": 1}".into()]);

    h.app.handle_key(ctrl('s'));
    assert!(h.app.sending);
    pump(&mut h, |a| !a.sending && a.history.len() == 1).await;

    assert_eq!(h.grpc.loads().len(), 1, "loaded because nothing was cached");
    let sent = h.grpc.requests();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].method, "demo.v1.Demo/GetUser");
    assert!(h.app.schema.is_some(), "and kept for the next call");

    // The next send reuses it.
    h.app.handle_key(ctrl('s'));
    pump(&mut h, |a| !a.sending && a.history.len() == 2).await;
    assert_eq!(h.grpc.loads().len(), 1);
    assert_eq!(h.grpc.requests().len(), 2);
}

#[tokio::test]
async fn a_failed_load_while_sending_is_recorded_as_a_call_that_never_reached_the_server() {
    let failing = FakeGrpc::new(Err(GrpcError::Schema("no reflection".into())));
    let mut h = grpc_harness(failing).await;
    h.app.selected_method = Some("demo.v1.Demo/GetUser".into());

    h.app.handle_key(ctrl('s'));
    pump(&mut h, |a| !a.sending && a.history.len() == 1).await;

    assert!(h.grpc.requests().is_empty());
    let Some(Outcome::Grpc(out)) = &h.app.live else {
        panic!("expected a gRPC outcome");
    };
    assert_eq!(
        (out.status.is_none(), out.error.as_deref()),
        (true, Some("no reflection"))
    );
}

// ---- gRPC: method picker -------------------------------------------------------------

async fn loaded_grpc_harness() -> Harness {
    let mut h = grpc_harness(FakeGrpc::new(Ok(fixture_schema()))).await;
    h.app.handle_key(ctrl('l'));
    pump(&mut h, |a| a.schema.is_some()).await;
    h
}

fn picker_names(app: &App) -> Vec<String> {
    app.picker_matches()
        .iter()
        .map(|m| m.name.clone())
        .collect()
}

#[tokio::test]
async fn ctrl_n_opens_the_picker_listing_every_method_by_service() {
    let mut h = loaded_grpc_harness().await;
    h.app.handle_key(ctrl('n'));
    assert!(h.app.picker.is_some());
    assert_eq!(
        picker_names(&h.app),
        ["Chat", "Collect", "Create", "GetUser", "ListUsers", "Tree"]
    );

    let out = grpc_screen(&h.app);
    for expected in [
        "Pick a method",
        "demo.v1.Demo",
        "GetUser  [unary]",
        "ListUsers  [server-stream]",
        "Collect  [client-stream]",
        "Chat  [bidi-stream]",
    ] {
        assert!(out.contains(expected), "missing `{expected}` in:\n{out}");
    }
    assert_eq!(
        h.app.method,
        Method::Get,
        "Ctrl+N does not cycle HTTP methods in gRPC mode"
    );
}

#[tokio::test]
async fn typing_filters_the_picker_ignoring_case() {
    let mut h = loaded_grpc_harness().await;
    h.app.handle_key(ctrl('p'));
    type_text(&mut h.app, "getu");
    assert_eq!(picker_names(&h.app), ["GetUser"]);

    for _ in 0..4 {
        h.app.handle_key(key(KeyCode::Backspace));
    }
    type_text(&mut h.app, "DEMO.V1.DEMO/c");
    assert_eq!(picker_names(&h.app), ["Chat", "Collect", "Create"]);

    type_text(&mut h.app, "zzz");
    assert!(picker_names(&h.app).is_empty());
    assert!(grpc_screen(&h.app).contains("no method matches"));
    h.app.handle_key(key(KeyCode::Enter));
    assert_eq!(h.app.selected_method, None, "nothing to pick");
}

#[tokio::test]
async fn picking_into_an_empty_body_fills_the_skeleton() {
    let mut h = loaded_grpc_harness().await;
    h.app.handle_key(ctrl('n'));
    type_text(&mut h.app, "listusers");
    h.app.handle_key(key(KeyCode::Enter));

    assert!(h.app.picker.is_none());
    assert_eq!(
        h.app.selected_method.as_deref(),
        Some("demo.v1.Demo/ListUsers")
    );
    let body: serde_json::Value = serde_json::from_str(&h.app.body_text()).unwrap();
    assert_eq!(
        body,
        serde_json::json!({"name": "", "pageSize": 0, "delayMs": 0})
    );
    assert!(grpc_screen(&h.app).contains("[server-stream]"));
}

#[tokio::test]
async fn picking_keeps_a_body_the_user_wrote() {
    let mut h = loaded_grpc_harness().await;
    h.app.body = TextArea::new(vec!["{\"id\": 42}".into()]);
    h.app.handle_key(ctrl('n'));
    type_text(&mut h.app, "getuser");
    h.app.handle_key(key(KeyCode::Enter));

    assert_eq!(
        h.app.selected_method.as_deref(),
        Some("demo.v1.Demo/GetUser")
    );
    assert_eq!(h.app.body_text(), "{\"id\": 42}");
}

#[tokio::test]
async fn arrow_keys_move_the_highlight_and_esc_closes_without_picking() {
    let mut h = loaded_grpc_harness().await;
    h.app.handle_key(ctrl('n'));
    h.app.handle_key(key(KeyCode::Down));
    h.app.handle_key(ctrl('n')); // also moves down while open
    assert_eq!(h.app.picker.as_ref().unwrap().selected, 2);
    h.app.handle_key(key(KeyCode::Up));
    assert_eq!(h.app.picker.as_ref().unwrap().selected, 1);
    for _ in 0..20 {
        h.app.handle_key(key(KeyCode::Down));
    }
    assert_eq!(
        h.app.picker.as_ref().unwrap().selected,
        5,
        "stops at the last"
    );

    h.app.handle_key(key(KeyCode::Esc));
    assert!(h.app.picker.is_none());
    assert_eq!(h.app.selected_method, None);

    h.app.handle_key(ctrl('n'));
    h.app.handle_key(key(KeyCode::Down));
    h.app.handle_key(key(KeyCode::Enter));
    assert_eq!(
        h.app.selected_method.as_deref(),
        Some("demo.v1.Demo/Collect")
    );
}

#[tokio::test]
async fn the_picker_needs_a_schema_for_the_current_address() {
    let mut h = grpc_harness(FakeGrpc::default()).await;
    h.app.handle_key(ctrl('n'));
    assert!(h.app.picker.is_none());
    assert_eq!(
        h.app.status.as_deref(),
        Some("load the schema first (Ctrl+L)")
    );

    let mut h = loaded_grpc_harness().await;
    h.app.url = TextArea::new(vec!["http://other:1".into()]);
    h.app.handle_key(ctrl('n'));
    assert!(
        h.app.picker.is_none(),
        "a schema loaded for another address is not offered"
    );
}

#[tokio::test]
async fn the_method_box_opens_the_picker_with_enter_or_a_click() {
    let mut h = loaded_grpc_harness().await;
    h.app.focus = Focus::Method;
    h.app.handle_key(key(KeyCode::Enter));
    assert!(h.app.picker.is_some());
    h.app.handle_key(key(KeyCode::Esc));

    let a = ui::areas_for(SCREEN, Kind::Grpc);
    click(&mut h.app, a.rpc.x + 3, a.rpc.y + 1);
    assert!(h.app.picker.is_some());
    assert_eq!(h.app.focus, Focus::Method);
}

#[tokio::test]
async fn clicking_a_picker_row_picks_it_and_clicking_outside_closes() {
    let mut h = loaded_grpc_harness().await;
    h.app.handle_key(ctrl('n'));
    let popup = ui::picker_area(SCREEN);
    let list = ui::picker_list_area(popup);

    // Row 0 is the service header, row 1 is `Chat`, row 4 is `GetUser`.
    click(&mut h.app, list.x + 4, list.y);
    assert!(h.app.picker.is_some(), "a header row picks nothing");
    click(&mut h.app, list.x + 4, list.y + 4);
    assert!(h.app.picker.is_none());
    assert_eq!(
        h.app.selected_method.as_deref(),
        Some("demo.v1.Demo/GetUser")
    );

    h.app.handle_key(ctrl('n'));
    click(&mut h.app, 0, 0); // on the left panel
    assert!(h.app.picker.is_none());
    assert_eq!(
        h.app.selected_method.as_deref(),
        Some("demo.v1.Demo/GetUser")
    );
}

#[tokio::test]
async fn sending_without_a_method_asks_for_one_and_sends_nothing() {
    let mut h = loaded_grpc_harness().await;
    h.app.handle_key(ctrl('s'));
    assert_eq!(
        h.app.status.as_deref(),
        Some("pick a method first (Ctrl+N)")
    );
    assert!(!h.app.sending);
    assert!(h.grpc.requests().is_empty());
}

#[tokio::test]
async fn rest_mode_ctrl_n_still_cycles_methods() {
    let mut h = harness().await;
    h.app.handle_key(ctrl('n'));
    assert_eq!(h.app.method, Method::Post);
    assert!(h.app.picker.is_none());
}

// ---- gRPC: live responses and cancel -----------------------------------------------------

fn message(json: &str) -> FakeStep {
    FakeStep::Event(CallEvent::Message(json.into()))
}

fn done_with(code: i32, message: &str, trailers: usize, elapsed_ms: u64) -> FakeStep {
    FakeStep::Event(CallEvent::Done(GrpcOutcome {
        messages: vec![],
        status: Some(GrpcStatus {
            code,
            message: message.into(),
            trailers: (0..trailers)
                .map(|i| Header::new(format!("t{i}"), "v"))
                .collect(),
        }),
        elapsed: Duration::from_millis(elapsed_ms),
        ..Default::default()
    }))
}

async fn streaming_harness(script: Vec<FakeStep>) -> Harness {
    let fake = FakeGrpc::new(Ok(fixture_schema())).with_script(script);
    let mut h = grpc_harness(fake).await;
    h.app.selected_method = Some("demo.v1.Demo/ListUsers".into());
    h
}

fn live_messages(app: &App) -> usize {
    match &app.live {
        Some(Outcome::Grpc(g)) => g.messages.len(),
        _ => 0,
    }
}

#[tokio::test]
async fn a_message_is_on_screen_while_the_call_is_still_open() {
    let mut h = streaming_harness(vec![
        FakeStep::Event(CallEvent::Headers(vec![Header::new("x-h", "1")])),
        message("{\"id\":1}"),
        FakeStep::HoldOpen,
    ])
    .await;
    h.app.handle_key(ctrl('s'));
    pump(&mut h, |a| live_messages(a) == 1).await;

    assert!(h.app.sending, "the call has not ended");
    let out = grpc_screen(&h.app);
    for expected in ["streaming 1", "^X cancel", "#1", "\"id\": 1", "x-h: 1"] {
        assert!(out.contains(expected), "missing `{expected}` in:\n{out}");
    }
    assert!(
        h.app.history.is_empty(),
        "history is written when the call ends"
    );
}

#[tokio::test]
async fn ctrl_x_cancels_keeps_the_messages_and_records_the_call() {
    let mut h = streaming_harness(vec![
        message("{\"id\":1}"),
        message("{\"id\":2}"),
        FakeStep::HoldOpen,
    ])
    .await;
    h.app.handle_key(ctrl('s'));
    pump(&mut h, |a| live_messages(a) == 2).await;

    h.app.handle_key(ctrl('x'));
    pump(&mut h, |a| !a.sending && a.history.len() == 1).await;

    let Some(Outcome::Grpc(out)) = &h.app.live else {
        panic!("expected a gRPC outcome");
    };
    assert_eq!(out.status.as_ref().unwrap().code, 1);
    assert_eq!(out.messages.len(), 2);
    let screen = grpc_screen(&h.app);
    assert!(
        screen.contains("CANCELLED (1)") && screen.contains("#2"),
        "{screen}"
    );

    let entry = h.store.get(h.app.history[0].id).await.unwrap().unwrap();
    let Outcome::Grpc(stored) = entry.outcome else {
        panic!("expected a gRPC entry");
    };
    assert_eq!((stored.status.unwrap().code, stored.messages.len()), (1, 2));
}

#[tokio::test]
async fn a_second_send_during_a_call_is_refused() {
    let mut h = streaming_harness(vec![FakeStep::HoldOpen]).await;
    h.app.handle_key(ctrl('s'));
    assert!(h.app.sending);

    h.app.handle_key(ctrl('s'));
    assert_eq!(
        h.app.status.as_deref(),
        Some("a request is already in flight")
    );
    pump(&mut h, |a| a.schema.is_some()).await; // the first call got going
    h.app.handle_key(ctrl('x'));
    pump(&mut h, |a| !a.sending).await;
    assert_eq!(h.grpc.requests().len(), 1, "only one call was made");
}

#[tokio::test]
async fn ctrl_x_with_nothing_running_says_so() {
    let mut h = harness().await;
    h.app.handle_key(ctrl('x'));
    assert_eq!(h.app.notice.as_deref(), Some("nothing to cancel"));
}

#[tokio::test]
async fn finished_call_shows_numbered_messages_status_elapsed_and_trailers() {
    let mut h = streaming_harness(vec![
        message("{\"id\":1}"),
        message("{\"id\":2}"),
        done_with(0, "", 2, 312),
    ])
    .await;
    h.app.handle_key(ctrl('s'));
    pump(&mut h, |a| !a.sending && a.history.len() == 1).await;

    let out = grpc_screen(&h.app);
    for expected in [
        "#1",
        "#2",
        "OK (0)",
        "312 ms",
        "2 trailers",
        "trailer t0: v",
    ] {
        assert!(out.contains(expected), "missing `{expected}` in:\n{out}");
    }
}

#[tokio::test]
async fn an_error_status_is_shown_with_its_message() {
    let mut h = streaming_harness(vec![done_with(5, "user 7 not found", 0, 4)]).await;
    h.app.handle_key(ctrl('s'));
    pump(&mut h, |a| !a.sending && a.history.len() == 1).await;

    let out = grpc_screen(&h.app);
    assert!(
        out.contains("NOT_FOUND (5)") && out.contains("NOT_FOUND: user 7 not found"),
        "{out}"
    );
    assert_eq!(h.app.history[0].status, Some(5));
}

#[tokio::test]
async fn a_call_that_never_reached_the_server_shows_its_error() {
    let fake = FakeGrpc::new(Ok(fixture_schema())).with_script(vec![FakeStep::Event(
        CallEvent::Done(GrpcOutcome {
            error: Some("cannot connect to http://localhost:50051: refused".into()),
            ..Default::default()
        }),
    )]);
    let mut h = grpc_harness(fake).await;
    h.app.selected_method = Some("demo.v1.Demo/GetUser".into());
    h.app.handle_key(ctrl('s'));
    pump(&mut h, |a| !a.sending && a.history.len() == 1).await;

    let out = grpc_screen(&h.app);
    assert!(
        out.contains("failed") && out.contains("cannot connect"),
        "{out}"
    );
    assert_eq!(h.app.history[0].status, None);
}

// ---- both lists tag rows by kind -------------------------------------------------------

/// The left panel of a 200-column screen (50 columns wide), one string per row.
fn left_panel(app: &App) -> String {
    text(&render(app, 200, 30))
        .lines()
        .map(|l| l.chars().take(50).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

fn grpc_entry(method: &str, code: Option<i32>, error: Option<&str>) -> (Request, Outcome) {
    (
        Request::Grpc(GrpcRequest {
            address: "http://localhost:50051".into(),
            method: method.into(),
            ..Default::default()
        }),
        Outcome::Grpc(GrpcOutcome {
            status: code.map(|code| GrpcStatus {
                code,
                message: String::new(),
                trailers: vec![],
            }),
            error: error.map(str::to_owned),
            ..Default::default()
        }),
    )
}

#[tokio::test]
async fn history_rows_tag_grpc_calls_with_short_method_and_status_name() {
    let mut h = harness().await;
    let http = Request::Http(HttpRequest {
        method: Method::Post,
        url: "http://x/http-row".into(),
        ..Default::default()
    });
    h.store
        .record(&http, &Outcome::Response(ok_response()))
        .await
        .unwrap();
    for (method, code, error) in [
        ("user.v1.UserService/GetUser", Some(5), None),
        ("user.v1.UserService/ListUsers", Some(0), None),
        ("user.v1.UserService/Never", None, Some("refused")),
    ] {
        let (req, out) = grpc_entry(method, code, error);
        h.store.record(&req, &out).await.unwrap();
    }
    h.app.refresh_history();
    pump(&mut h, |a| a.history.len() == 4).await;
    h.app.set_tab(Tab::History);

    let left = left_panel(&h.app);
    let row = |needle: &str| {
        left.lines()
            .find(|l| l.contains(needle))
            .unwrap_or_else(|| panic!("no row with `{needle}` in:\n{left}"))
            .to_owned()
    };
    let never = row("Never");
    assert!(never.contains("ERR") && never.contains("gRPC"), "{never}");
    let list = row("ListUsers");
    assert!(
        list.contains("OK") && list.contains("gRPC UserService/ListUsers"),
        "{list}"
    );
    let get = row("GetUser");
    assert!(
        get.contains("NOT_FOUND") && get.contains("gRPC UserService/GetUser"),
        "{get}"
    );
    assert!(!get.contains("user.v1."), "the package is dropped: {get}");
    let http_row = row("http-row");
    assert!(
        http_row.contains("200") && http_row.contains("POST") && !http_row.contains("gRPC"),
        "{http_row}"
    );
}

#[tokio::test]
async fn stored_rows_tag_grpc_templates_and_preview_and_load_them() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("projects/api");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("a-http.toml"),
        "name = \"List things\"\nmethod = \"GET\"\nurl = \"http://x/things\"\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("b-grpc.toml"),
        "name = \"Get user\"\nkind = \"grpc\"\naddress = \"http://localhost:50051\"\n\
         method = \"user.v1.UserService/GetUser\"\nreflection = true\nbody = '{\"id\": 7}'\n",
    )
    .unwrap();
    let store = History::open(&tmp.path().join("db.sqlite3")).await.unwrap();
    let (tx, _rx) = unbounded_channel();
    let mut app = App::new(
        Arc::new(FakeClient::new(Ok(ok_response()))),
        Arc::new(FakeGrpc::default()),
        store,
        load_projects(&tmp.path().join("projects")),
        tx,
    );

    let left = left_panel(&app);
    assert!(left.contains("GET    List things"), "{left}");
    assert!(left.contains("gRPC   Get user"), "{left}");

    // Moving over the gRPC template previews it, whatever the editor is in.
    app.handle_key(key(KeyCode::Down));
    assert_eq!(app.view().request.kind, Kind::Grpc);
    assert!(text(&render(&app, 120, 30)).contains("Address (preview)"));
    assert_eq!(app.kind, Kind::Http);

    app.handle_key(key(KeyCode::Enter));
    assert_eq!(app.kind, Kind::Grpc);
    assert_eq!(app.url_text(), "http://localhost:50051");
    assert_eq!(
        app.selected_method.as_deref(),
        Some("user.v1.UserService/GetUser")
    );
    assert_eq!(app.schema_source(), SchemaSource::Reflection);
    assert_eq!(app.body_text(), "{\"id\": 7}");
}

// ---- gRPC: copy and reading panel ----------------------------------------------------------

/// A finished call as `Done` carries it.
fn finished(messages: &[&str], stream: bool, code: i32, status_message: &str) -> FakeStep {
    FakeStep::Event(CallEvent::Done(GrpcOutcome {
        messages: messages.iter().map(|m| m.to_string()).collect(),
        status: Some(GrpcStatus {
            code,
            message: status_message.into(),
            trailers: vec![],
        }),
        response_stream: stream,
        ..Default::default()
    }))
}

async fn after_call(method: &str, script: Vec<FakeStep>) -> Harness {
    let fake = FakeGrpc::new(Ok(fixture_schema())).with_script(script);
    let mut h = grpc_harness(fake).await;
    h.app.selected_method = Some(method.into());
    h.app.handle_key(ctrl('s'));
    pump(&mut h, |a| !a.sending && a.history.len() == 1).await;
    h
}

fn copied(app: &mut App) -> Option<serde_json::Value> {
    app.focus = Focus::List;
    app.handle_key(key(KeyCode::Char('y')));
    app.take_clipboard()
        .map(|text| serde_json::from_str(&text).unwrap_or(serde_json::Value::String(text)))
}

#[tokio::test]
async fn copying_a_streamed_response_copies_a_json_array_of_the_messages() {
    let mut h = after_call(
        "demo.v1.Demo/ListUsers",
        vec![finished(&["{\"id\":1}", "{\"id\":2}"], true, 0, "")],
    )
    .await;
    assert_eq!(
        copied(&mut h.app),
        Some(serde_json::json!([{"id": 1}, {"id": 2}]))
    );
    assert!(h.app.notice.as_deref().unwrap().starts_with("copied "));
}

#[tokio::test]
async fn copying_a_unary_response_copies_the_single_message() {
    let mut h = after_call(
        "demo.v1.Demo/GetUser",
        vec![finished(&["{\"id\":1,\"name\":\"a\"}"], false, 0, "")],
    )
    .await;
    assert_eq!(
        copied(&mut h.app),
        Some(serde_json::json!({"id": 1, "name": "a"}))
    );
}

#[tokio::test]
async fn copying_an_error_status_or_a_connection_error_copies_that_text() {
    let mut h = after_call(
        "demo.v1.Demo/GetUser",
        vec![finished(&[], false, 5, "user 7 not found")],
    )
    .await;
    assert_eq!(
        copied(&mut h.app),
        Some(serde_json::Value::String(
            "NOT_FOUND: user 7 not found".into()
        ))
    );

    let refused = FakeStep::Event(CallEvent::Done(GrpcOutcome {
        error: Some("cannot connect: refused".into()),
        ..Default::default()
    }));
    let mut h = after_call("demo.v1.Demo/GetUser", vec![refused]).await;
    assert_eq!(
        copied(&mut h.app),
        Some(serde_json::Value::String("cannot connect: refused".into()))
    );
}

#[tokio::test]
async fn nothing_to_copy_for_a_successful_call_without_messages() {
    let mut h = after_call("demo.v1.Demo/GetUser", vec![finished(&[], false, 0, "")]).await;
    assert_eq!(copied(&mut h.app), None);
    assert_eq!(h.app.notice.as_deref(), Some("nothing to copy"));
}

#[tokio::test]
async fn the_reading_panel_shows_the_array_for_streams_and_the_message_for_unary() {
    let mut h = after_call(
        "demo.v1.Demo/ListUsers",
        vec![finished(&["{\"id\":1}", "{\"id\":2}"], true, 0, "")],
    )
    .await;
    h.app.focus = Focus::Response;
    h.app.handle_key(key(KeyCode::Enter));
    let out = grpc_screen(&h.app);
    assert!(out.contains("Response body - JSON"), "{out}");
    assert!(
        out.contains('[') && out.contains("\"id\": 1") && out.contains("\"id\": 2"),
        "{out}"
    );
    h.app.handle_key(key(KeyCode::Esc));

    let mut h = after_call(
        "demo.v1.Demo/GetUser",
        vec![finished(&["{\"id\":9}"], false, 0, "")],
    )
    .await;
    h.app.focus = Focus::Response;
    h.app.handle_key(key(KeyCode::Enter));
    let out = grpc_screen(&h.app);
    assert!(
        out.contains("{\"id\":9}") && !out.contains("[{"),
        "as received: {out}"
    );
    h.app.handle_key(key(KeyCode::Char('f')));
    assert!(grpc_screen(&h.app).contains("\"id\": 9"), "F formats it");
}

#[tokio::test]
async fn the_reading_panel_has_nothing_to_open_for_an_empty_unary_response() {
    let mut h = after_call("demo.v1.Demo/GetUser", vec![finished(&[], false, 0, "")]).await;
    h.app.focus = Focus::Response;
    h.app.handle_key(key(KeyCode::Enter));
    assert!(h.app.modal.is_none());
    assert_eq!(h.app.notice.as_deref(), Some("the response body is empty"));
}

// ---- key help --------------------------------------------------------------------------

#[tokio::test]
async fn the_status_bar_help_matches_the_mode_and_lists_the_grpc_keys() {
    let mut h = harness().await;
    let rest = grpc_screen(&h.app);
    assert!(
        rest.contains("Ctrl+G gRPC") && rest.contains("Ctrl+S send"),
        "{rest}"
    );
    assert!(!rest.contains("Ctrl+X"), "{rest}");

    h.app.handle_key(ctrl('g'));
    let grpc = grpc_screen(&h.app);
    for key in [
        "Ctrl+G REST/gRPC",
        "Ctrl+L schema",
        "Ctrl+N method",
        "Ctrl+X cancel",
        "Ctrl+O source",
        "Ctrl+K TLS",
    ] {
        assert!(grpc.contains(key), "missing `{key}` in:\n{grpc}");
    }
    // Neither line may be cut off at 120 columns.
    assert!(ui::HELP_REST.len() <= 120 && ui::HELP_GRPC.len() <= 120);
}

#[test]
fn every_grpc_key_is_documented_in_the_readme() {
    let readme =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/README.md")).unwrap();
    for key in [
        "`Ctrl+G`",
        "`Ctrl+L`",
        "`Ctrl+O`",
        "`Ctrl+K`",
        "`Ctrl+X`",
        "`Ctrl+N` / `Ctrl+P`",
    ] {
        assert!(readme.contains(key), "README lacks {key}");
    }
}
