INSERT INTO sites (name, base_url, status) VALUES
  ('demo-site', 'https://example.com', 'active');

INSERT INTO accounts (site_id, label, api_key, status) VALUES
  (1, 'demo-account', 'replace-me', 'active');

INSERT INTO model_routes (model_pattern, enabled, routing_strategy, cooldown_seconds) VALUES
  ('gpt-5.4', 1, 'weighted', 300);

INSERT INTO channels (
  route_id,
  account_id,
  label,
  upstream_model,
  supports_responses,
  enabled,
  weight,
  priority
) VALUES (
  1,
  1,
  'default',
  'gpt-5.4',
  1,
  1,
  10,
  0
);
