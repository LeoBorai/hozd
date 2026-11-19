-- Applied migrations are append-only: never edit this file once released, add a new
-- numbered migration instead (and its entry in toasty/history.toml). Toasty records only the
-- id of an applied migration, so an edit never reaches users' databases.
-- Statements are separated by `toasty::breakpoint` comment lines: the SQLite driver runs one
-- statement at a time.

CREATE TABLE history (
    id               INTEGER PRIMARY KEY AUTOINCREMENT,
    created_at_ms    INTEGER NOT NULL,
    method           TEXT    NOT NULL,
    url              TEXT    NOT NULL,
    request_headers  TEXT    NOT NULL, -- JSON array of {name, value}
    request_body     TEXT,
    status           INTEGER,          -- NULL when no response was received
    response_headers TEXT,             -- JSON array of {name, value}
    response_body    TEXT,
    elapsed_ms       INTEGER,
    error            TEXT              -- set when no response was received
);

-- #[toasty::breakpoint]
CREATE INDEX history_created_at ON history (id DESC);
