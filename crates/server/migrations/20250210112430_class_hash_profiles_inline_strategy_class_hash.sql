-- Add migration script here
ALTER TABLE class_hash_profiles 
ADD COLUMN IF NOT EXISTS inline_strategy_class_hash VARCHAR(66);
