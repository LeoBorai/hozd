-- Applied migrations are append-only: never edit this file once released,
-- add a new numbered migration instead (editing breaks the checksum on users' databases).

-- Projects of the Projects tab. `data` is the format-neutral `model::Project` as JSON, so
-- directories and requests need no tables of their own.
CREATE TABLE projects (
    id            TEXT    PRIMARY KEY,
    name          TEXT    NOT NULL,
    created_at_ms INTEGER NOT NULL,
    data          TEXT    NOT NULL
);
