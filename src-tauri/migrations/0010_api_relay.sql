-- API relay (third-party Responses reverse-proxy surface).
-- Independent of Codex catalog publish (`model_routes.enabled`).

ALTER TABLE model_routes ADD COLUMN relay_enabled INTEGER NOT NULL DEFAULT 0;

CREATE TABLE IF NOT EXISTS relay_api_keys (
  id TEXT PRIMARY KEY NOT NULL,
  label TEXT NOT NULL,
  key_prefix TEXT NOT NULL,
  key_hash TEXT NOT NULL UNIQUE,
  enabled INTEGER NOT NULL DEFAULT 1,
  allowed_models_json TEXT NOT NULL DEFAULT '[]',
  created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
  updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
  last_used_at TEXT
);

CREATE INDEX IF NOT EXISTS idx_relay_api_keys_enabled ON relay_api_keys(enabled);
