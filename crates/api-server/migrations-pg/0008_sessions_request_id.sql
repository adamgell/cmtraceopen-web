ALTER TABLE sessions ADD COLUMN request_id uuid NULL;
CREATE INDEX sessions_request_id_idx ON sessions (request_id) WHERE request_id IS NOT NULL;
