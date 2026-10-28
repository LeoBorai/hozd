use std::time::Duration;

use hozd::client::{Header, Method, Request, Response};
use hozd::history::{History, Outcome};
use hozd::paths::Home;
use hozd::startup;
use sqlx::SqlitePool;

#[tokio::test]
async fn first_run_creates_home_and_database() {
    let tmp = tempfile::tempdir().unwrap();
    let home = Home::at(tmp.path().join(".hozd"));
    startup(&home).await.unwrap();
    assert!(home.root().is_dir() && home.projects_dir().is_dir() && home.db_path().is_file());
}

#[tokio::test]
async fn unwritable_home_names_the_path() {
    let tmp = tempfile::tempdir().unwrap();
    let blocker = tmp.path().join("file");
    std::fs::write(&blocker, "x").unwrap();
    let err = startup(&Home::at(blocker.join("home")))
        .await
        .err()
        .unwrap();
    assert!(format!("{err:#}").contains("file"), "{err:#}");
}

#[tokio::test]
async fn malformed_config_names_the_file() {
    let tmp = tempfile::tempdir().unwrap();
    let home = Home::at(tmp.path());
    std::fs::write(home.config_path(), "= broken").unwrap();
    let err = startup(&home).await.err().unwrap();
    assert!(format!("{err:#}").contains("config.toml"), "{err:#}");
}

#[tokio::test]
async fn newer_database_is_refused_with_upgrade_message() {
    let tmp = tempfile::tempdir().unwrap();
    let home = Home::at(tmp.path());
    startup(&home).await.unwrap();

    let pool = SqlitePool::connect(&format!("sqlite://{}", home.db_path().display()))
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO _sqlx_migrations (version, description, success, checksum, execution_time)
         VALUES (9999, 'future', 1, x'00', 0)",
    )
    .execute(&pool)
    .await
    .unwrap();
    pool.close().await;

    let err = startup(&home).await.err().unwrap();
    assert!(format!("{err:#}").contains("newer hozd"), "{err:#}");
}

/// History written by one run is there, replayable, after a restart.
#[tokio::test]
async fn history_survives_restart_and_can_be_replayed() {
    let tmp = tempfile::tempdir().unwrap();
    let home = Home::at(tmp.path());

    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .respond_with(wiremock::ResponseTemplate::new(201).set_body_string("made"))
        .expect(2)
        .mount(&server)
        .await;

    // first run: send for real through the app
    let (mut app, mut rx) = startup(&home).await.unwrap();
    app.load_request(&Request {
        method: Method::Post,
        url: server.uri(),
        headers: vec![Header::new("X-T", "1")],
        body: Some("{\"a\":1}".into()),
    });
    app.send();
    wait(&mut app, &mut rx, |a| {
        a.live.is_some() && a.history.len() == 1
    })
    .await;
    drop((app, rx));

    // restart: the entry is listed with its full request and response
    let (mut app, mut rx) = startup(&home).await.unwrap();
    wait(&mut app, &mut rx, |a| a.history.len() == 1).await;
    let id = app.history[0].id;
    let entry = History::open(&home.db_path())
        .await
        .unwrap()
        .get(id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(entry.request.url, server.uri());
    let Outcome::Response(Response { status, body, .. }) = &entry.outcome else {
        panic!("expected response");
    };
    assert_eq!((*status, body.as_str()), (201, "made"));

    // replay: load it into the editor and send again; original untouched, new entry added
    app.load_request(&entry.request);
    app.send();
    wait(&mut app, &mut rx, |a| a.history.len() == 2).await;
    assert_eq!(app.history[1].id, id);
    assert_eq!(app.history[0].url, server.uri());
}

async fn wait(
    app: &mut hozd::tui::App,
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<hozd::tui::Msg>,
    done: impl Fn(&hozd::tui::App) -> bool,
) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while !done(app) {
            let msg = rx.recv().await.unwrap();
            app.update(msg);
        }
    })
    .await
    .expect("timed out");
}
