-- Applied migrations are append-only: never edit this file once released,
-- add a new numbered migration instead (editing breaks the checksum on users' databases).

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

CREATE INDEX history_created_at ON history (id DESC);
