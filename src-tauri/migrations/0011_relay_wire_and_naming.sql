-- Relay API keys: wire type (responses|completions) + naming style (dotted|flat).
ALTER TABLE relay_api_keys ADD COLUMN wire_type TEXT NOT NULL DEFAULT 'responses';
ALTER TABLE relay_api_keys ADD COLUMN name_style TEXT NOT NULL DEFAULT 'flat';
ALTER TABLE relay_api_keys ADD COLUMN allowed_providers_json TEXT NOT NULL DEFAULT '[]';
