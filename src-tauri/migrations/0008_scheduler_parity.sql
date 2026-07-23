-- Sub2API-parity scheduler knobs: cost rate, last_used, wait-queue tracking.

ALTER TABLE pool_members ADD COLUMN upstream_cost_rate REAL NOT NULL DEFAULT 1.0;

ALTER TABLE credentials ADD COLUMN last_used_at INTEGER;

CREATE TABLE IF NOT EXISTS schedule_waiters (
  id TEXT PRIMARY KEY NOT NULL,
  pool_id TEXT NOT NULL REFERENCES account_pools(id) ON DELETE CASCADE,
  credential_id TEXT,
  kind TEXT NOT NULL,
  expires_at INTEGER NOT NULL,
  created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_schedule_waiters_cred
  ON schedule_waiters(credential_id, kind, expires_at);
CREATE INDEX IF NOT EXISTS idx_schedule_waiters_pool
  ON schedule_waiters(pool_id, kind, expires_at);
CREATE INDEX IF NOT EXISTS idx_credentials_last_used ON credentials(last_used_at);
