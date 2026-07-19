CREATE TABLE IF NOT EXISTS credentials (
  id TEXT PRIMARY KEY NOT NULL,
  provider_id TEXT NOT NULL REFERENCES providers(id) ON DELETE CASCADE,
  kind TEXT NOT NULL,
  state TEXT NOT NULL,
  label TEXT,
  email TEXT,
  account_id TEXT,
  expires_at INTEGER,
  fingerprint TEXT NOT NULL UNIQUE,
  refreshable INTEGER NOT NULL DEFAULT 0,
  healthy INTEGER NOT NULL DEFAULT 1,
  last_error TEXT,
  secret_envelope_json TEXT NOT NULL,
  credential_version INTEGER NOT NULL DEFAULT 1,
  created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
  updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_credentials_provider ON credentials(provider_id);
CREATE INDEX IF NOT EXISTS idx_credentials_healthy ON credentials(provider_id, healthy);

CREATE TABLE IF NOT EXISTS account_pools (
  id TEXT PRIMARY KEY NOT NULL,
  name TEXT NOT NULL,
  provider_id TEXT NOT NULL REFERENCES providers(id) ON DELETE CASCADE,
  strategy TEXT NOT NULL DEFAULT 'round_robin',
  sticky_ttl_secs INTEGER NOT NULL DEFAULT 3600,
  enabled INTEGER NOT NULL DEFAULT 1,
  created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
  updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS pool_members (
  pool_id TEXT NOT NULL REFERENCES account_pools(id) ON DELETE CASCADE,
  credential_id TEXT NOT NULL REFERENCES credentials(id) ON DELETE CASCADE,
  weight INTEGER NOT NULL DEFAULT 1,
  priority INTEGER NOT NULL DEFAULT 0,
  enabled INTEGER NOT NULL DEFAULT 1,
  created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (pool_id, credential_id)
);

CREATE TABLE IF NOT EXISTS account_leases (
  id TEXT PRIMARY KEY NOT NULL,
  pool_id TEXT NOT NULL REFERENCES account_pools(id) ON DELETE CASCADE,
  credential_id TEXT NOT NULL REFERENCES credentials(id) ON DELETE CASCADE,
  affinity_key TEXT,
  acquired_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
  expires_at TEXT,
  released_at TEXT
);

CREATE INDEX IF NOT EXISTS idx_account_leases_affinity ON account_leases(pool_id, affinity_key, released_at);

CREATE TABLE IF NOT EXISTS usage_snapshots (
  id TEXT PRIMARY KEY NOT NULL,
  credential_id TEXT NOT NULL REFERENCES credentials(id) ON DELETE CASCADE,
  window TEXT NOT NULL,
  used_units INTEGER,
  limit_units INTEGER,
  reset_at TEXT,
  fetched_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
  UNIQUE (credential_id, window)
);

CREATE TABLE IF NOT EXISTS reset_credit_actions (
  idempotency_key TEXT PRIMARY KEY NOT NULL,
  credential_id TEXT NOT NULL REFERENCES credentials(id) ON DELETE CASCADE,
  status TEXT NOT NULL,
  result_json TEXT,
  created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
  completed_at TEXT
);
