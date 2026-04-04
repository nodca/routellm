ALTER TABLE channels
ADD COLUMN avg_latency_ms INTEGER;

ALTER TABLE request_logs
ADD COLUMN input_tokens INTEGER;

ALTER TABLE request_logs
ADD COLUMN output_tokens INTEGER;

ALTER TABLE request_logs
ADD COLUMN total_tokens INTEGER;
