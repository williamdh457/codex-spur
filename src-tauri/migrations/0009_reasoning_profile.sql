-- User-selected reasoning mapping template per provider instance.
-- Values: openai_native | openai_compat | deepseek | kimi | xai | minimax | glm | qwen | passthrough
ALTER TABLE providers ADD COLUMN reasoning_profile_id TEXT NOT NULL DEFAULT '';
