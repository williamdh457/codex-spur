CREATE TABLE IF NOT EXISTS app_settings (
  key TEXT PRIMARY KEY NOT NULL,
  value_json TEXT NOT NULL,
  updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS providers (
  id TEXT PRIMARY KEY NOT NULL,
  name TEXT NOT NULL,
  region TEXT NOT NULL,
  protocol TEXT NOT NULL,
  base_url TEXT,
  configured INTEGER NOT NULL DEFAULT 0,
  selected_models INTEGER NOT NULL DEFAULT 0,
  discovered_models INTEGER NOT NULL DEFAULT 0,
  last_fetched_at TEXT
);

CREATE TABLE IF NOT EXISTS model_routes (
  id TEXT PRIMARY KEY NOT NULL,
  provider_id TEXT NOT NULL REFERENCES providers(id) ON DELETE CASCADE,
  upstream_model TEXT NOT NULL,
  display_name TEXT NOT NULL,
  enabled INTEGER NOT NULL DEFAULT 0,
  catalog_json TEXT NOT NULL,
  created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
  updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS apply_revisions (
  id TEXT PRIMARY KEY NOT NULL,
  catalog_path TEXT NOT NULL,
  config_path TEXT NOT NULL,
  config_hash_before TEXT,
  config_hash_after TEXT,
  state TEXT NOT NULL,
  created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);
