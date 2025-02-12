-- Add migration script here
ALTER TABLE class_hash_profiles
ADD COLUMN is_inline_strategy_active BOOLEAN NOT NULL DEFAULT FALSE;
