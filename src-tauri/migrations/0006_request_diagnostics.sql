-- Redacted proxy request diagnostics for scheduler layer visibility.

CREATE TABLE IF NOT EXISTS proxy_request_events (
  id TEXT PRIMARY KEY NOT NULL,
  created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
  route_slug TEXT,
  display_name TEXT,
  provider_id TEXT,
  upstream_model TEXT,
  protocol TEXT,
  selection_layer TEXT NOT NULL,
  sticky_escaped INTEGER NOT NULL DEFAULT 0,
  account_fingerprint TEXT,
  schedule_state TEXT,
  result_category TEXT NOT NULL,
  failover_attempt INTEGER NOT NULL DEFAULT 0,
  latency_ms_total INTEGER,
  first_token_ms INTEGER,
  cooldown_applied INTEGER NOT NULL DEFAULT 0,
  error_summary TEXT
);

CREATE INDEX IF NOT EXISTS idx_proxy_request_events_created
  ON proxy_request_events(created_at DESC);
