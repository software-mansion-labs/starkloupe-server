-- Add migration script here

-- Add index on the 'id' column on verification_status table
CREATE INDEX verification_status_id_idx ON verification_status (id);

-- Add index on the 'verification_id' on class_hash_profiles table
CREATE INDEX class_hash_profiles_verification_id_idx ON class_hash_profiles (verification_id);
