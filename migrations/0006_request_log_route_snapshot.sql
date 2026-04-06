ALTER TABLE request_logs
ADD COLUMN route_id INTEGER;

ALTER TABLE request_logs
ADD COLUMN channel_label TEXT;

ALTER TABLE request_logs
ADD COLUMN site_name TEXT;

ALTER TABLE request_logs
ADD COLUMN upstream_model TEXT;

UPDATE request_logs
SET route_id = (
    SELECT c.route_id
    FROM channels c
    WHERE c.id = request_logs.channel_id
  ),
  channel_label = (
    SELECT c.label
    FROM channels c
    WHERE c.id = request_logs.channel_id
  ),
  site_name = (
    SELECT s.name
    FROM channels c
    JOIN accounts a ON a.id = c.account_id
    JOIN sites s ON s.id = a.site_id
    WHERE c.id = request_logs.channel_id
  ),
  upstream_model = (
    SELECT c.upstream_model
    FROM channels c
    WHERE c.id = request_logs.channel_id
  )
WHERE channel_id IS NOT NULL;

CREATE INDEX IF NOT EXISTS idx_request_logs_route_created
  ON request_logs(route_id, created_at DESC);
