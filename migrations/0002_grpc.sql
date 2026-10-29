-- Applied migrations are append-only: never edit this file once released,
-- add a new numbered migration instead (editing breaks the checksum on users' databases).

-- Rows written before gRPC support are HTTP, which the default says.
ALTER TABLE history ADD COLUMN kind TEXT NOT NULL DEFAULT 'http';

-- gRPC rows only. JSON with what the shared columns cannot hold: schema source, insecure
-- flag, received messages, status message and trailers.
-- The shared columns hold: method = `package.Service/Method`, url = server address,
-- status = gRPC status code, request_headers / response_headers = metadata.
ALTER TABLE history ADD COLUMN grpc TEXT;
