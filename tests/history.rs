use std::time::Duration;

use hozd::client::{Header, Method, Request, Response};
use hozd::history::{History, HistoryError, MAX_BODY_BYTES, Outcome};
use sqlx::SqlitePool;

fn request(url: &str) -> Request {
    Request {
        method: Method::Post,
        url: url.into(),
        headers: vec![Header::new("Content-Type", "application/json")],
        body: Some("{\"a\":1}".into()),
    }
}

fn response(status: u16, body: &str) -> Outcome {
    Outcome::Response(Response {
        status,
        headers: vec![Header::new("x-a", "1"), Header::new("x-a", "2")],
        body: body.into(),
        elapsed: Duration::from_millis(42),
    })
}

#[sqlx::test]
async fn success_entry_holds_full_request_and_response(pool: SqlitePool) {
    let history = History::from_pool(pool);
    let req = request("http://x/ok");
    let out = response(201, "created");
    let id = history.record(&req, &out).await.unwrap();

    let entry = history.get(id).await.unwrap().unwrap();
    assert_eq!(entry.request, req);
    assert_eq!(entry.outcome, out);
}

#[sqlx::test]
async fn error_entry_holds_request_and_message(pool: SqlitePool) {
    let history = History::from_pool(pool);
    let req = request("http://x/down");
    let out = Outcome::Error("request failed: connection refused".into());
    let id = history.record(&req, &out).await.unwrap();

    let entry = history.get(id).await.unwrap().unwrap();
    assert_eq!(entry.request, req);
    assert_eq!(entry.outcome, out);
}

#[sqlx::test]
async fn list_is_newest_first_with_status(pool: SqlitePool) {
    let history = History::from_pool(pool);
    history
        .record(&request("http://x/1"), &response(200, ""))
        .await
        .unwrap();
    history
        .record(&request("http://x/2"), &Outcome::Error("boom".into()))
        .await
        .unwrap();

    let list = history.list(10).await.unwrap();
    assert_eq!(list.len(), 2);
    assert_eq!(list[0].url, "http://x/2");
    assert_eq!(list[0].status, None);
    assert_eq!(list[1].url, "http://x/1");
    assert_eq!(list[1].status, Some(200));
    assert_eq!(history.list(1).await.unwrap().len(), 1);
}

#[sqlx::test]
async fn get_unknown_id_is_none(pool: SqlitePool) {
    assert!(History::from_pool(pool).get(999).await.unwrap().is_none());
}

#[sqlx::test]
async fn oversized_bodies_are_truncated_on_char_boundary(pool: SqlitePool) {
    let history = History::from_pool(pool);
    let big = "é".repeat(MAX_BODY_BYTES); // 2 bytes per char
    let id = history
        .record(&request("http://x"), &response(200, &big))
        .await
        .unwrap();
    let Outcome::Response(res) = history.get(id).await.unwrap().unwrap().outcome else {
        panic!("expected response");
    };
    assert!(res.body.len() <= MAX_BODY_BYTES && !res.body.is_empty());
    assert!(res.body.chars().all(|c| c == 'é'));
}

#[tokio::test]
async fn fresh_install_creates_file_and_entries_survive_reopen() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("db.sqlite3");
    assert!(!path.exists());

    let id = {
        let history = History::open(&path).await.unwrap();
        history
            .record(&request("http://x/a"), &response(200, "a"))
            .await
            .unwrap()
    };
    assert!(path.exists());

    let reopened = History::open(&path).await.unwrap(); // re-running migrations is a no-op
    let entry = reopened.get(id).await.unwrap().unwrap();
    assert_eq!(entry.request.url, "http://x/a");
}

#[cfg(unix)]
#[tokio::test]
async fn database_file_is_private() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("db.sqlite3");
    History::open(&path).await.unwrap();
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
}

#[tokio::test]
async fn newer_database_is_refused_and_left_unchanged() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("db.sqlite3");
    History::open(&path).await.unwrap();

    // Pretend a newer hozd applied a migration this binary does not embed.
    let pool = SqlitePool::connect(&format!("sqlite://{}", path.display()))
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO _sqlx_migrations (version, description, success, checksum, execution_time)
         VALUES (9999, 'from the future', 1, x'00', 0)",
    )
    .execute(&pool)
    .await
    .unwrap();
    pool.close().await;
    let before = std::fs::read(&path).unwrap();

    let err = History::open(&path).await.err().expect("must refuse");
    assert!(matches!(err, HistoryError::NewerDatabase(_)), "{err}");
    assert!(err.to_string().contains("newer hozd"));
    assert_eq!(
        std::fs::read(&path).unwrap(),
        before,
        "database must be untouched"
    );
}
