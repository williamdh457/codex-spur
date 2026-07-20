-- Multi-instance providers: kind separates template type from instance id.
ALTER TABLE providers ADD COLUMN kind TEXT NOT NULL DEFAULT '';
ALTER TABLE providers ADD COLUMN active_pool_id TEXT;

UPDATE providers SET kind = id WHERE kind = '' OR kind IS NULL;
