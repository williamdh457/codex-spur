-- Per-route user overrides for display name and context window.
-- NULL = use official defaults. Survives rediscovery / catalog heal.

ALTER TABLE model_routes ADD COLUMN display_name_override TEXT;
ALTER TABLE model_routes ADD COLUMN context_window_override INTEGER;
