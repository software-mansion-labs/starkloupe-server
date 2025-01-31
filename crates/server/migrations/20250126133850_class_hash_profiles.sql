-- Add migration script here
CREATE TABLE class_hash_profiles (
    class_hash VARCHAR(66),
    profile VARCHAR(30),
    verification_id UUID,
    PRIMARY KEY (class_hash, profile, verification_id)
);
