-- Applied migrations are append-only: never edit this file once released, add a new
-- numbered migration instead (and its entry in toasty/history.toml). Toasty records only the
-- id of an applied migration, so an edit never reaches users' databases.
-- Statements are separated by `toasty::breakpoint` comment lines: the SQLite driver runs one
-- statement at a time.

-- Rows written before gRPC support are HTTP, which the default says.
ALTER TABLE history ADD COLUMN kind TEXT NOT NULL DEFAULT 'http';

-- #[toasty::breakpoint]
-- gRPC rows only. JSON with what the shared columns cannot hold: schema source, insecure
-- flag, received messages, status message and trailers.
-- The shared columns hold: method = `package.Service/Method`, url = server address,
-- status = gRPC status code, request_headers / response_headers = metadata.
ALTER TABLE history ADD COLUMN grpc TEXT;
