-- Sub2API-like scheduler: routing mode, sticky bindings, cooldown, config.

ALTER TABLE providers ADD COLUMN routing_mode TEXT NOT NULL DEFAULT 'pool';
ALTER TABLE providers ADD COLUMN fixed_credential_id TEXT;

ALTER TABLE account_pools ADD COLUMN scheduler_config_json TEXT NOT NULL DEFAULT '{}';

ALTER TABLE pool_members ADD COLUMN concurrency_limit INTEGER NOT NULL DEFAULT 1;

ALTER TABLE credentials ADD COLUMN cooldown_until INTEGER;
ALTER TABLE credentials ADD COLUMN schedule_state TEXT NOT NULL DEFAULT 'ready';
ALTER TABLE credentials ADD COLUMN error_rate_ewma REAL NOT NULL DEFAULT 0;
ALTER TABLE credentials ADD COLUMN ttft_ewma_ms REAL NOT NULL DEFAULT 0;

CREATE TABLE IF NOT EXISTS sticky_bindings (
  pool_id TEXT NOT NULL REFERENCES account_pools(id) ON DELETE CASCADE,
  binding_kind TEXT NOT NULL,
  binding_key_hash TEXT NOT NULL,
  credential_id TEXT NOT NULL REFERENCES credentials(id) ON DELETE CASCADE,
  expires_at INTEGER NOT NULL,
  updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (pool_id, binding_kind, binding_key_hash)
);

CREATE INDEX IF NOT EXISTS idx_sticky_bindings_expires ON sticky_bindings(expires_at);
CREATE INDEX IF NOT EXISTS idx_credentials_cooldown ON credentials(cooldown_until);
CREATE INDEX IF NOT EXISTS idx_account_leases_active ON account_leases(credential_id, released_at, expires_at);
