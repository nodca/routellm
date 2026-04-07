ALTER TABLE request_logs
ADD COLUMN downstream_client_request_id TEXT;

ALTER TABLE request_logs
ADD COLUMN claude_request_fingerprint TEXT;

CREATE INDEX IF NOT EXISTS idx_request_logs_client_request_id
  ON request_logs(downstream_client_request_id);
