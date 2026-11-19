-- Applied migrations are append-only: never edit this file once released, add a new
-- numbered migration instead (and its entry in toasty/history.toml). Toasty records only the
-- id of an applied migration, so an edit never reaches users' databases.
-- Statements are separated by `toasty::breakpoint` comment lines: the SQLite driver runs one
-- statement at a time.

-- Projects of the Projects tab. `data` is the format-neutral `model::Project` as JSON, so
-- directories and requests need no tables of their own.
CREATE TABLE projects (
    id            TEXT    PRIMARY KEY,
    name          TEXT    NOT NULL,
    created_at_ms INTEGER NOT NULL,
    data          TEXT    NOT NULL
);
