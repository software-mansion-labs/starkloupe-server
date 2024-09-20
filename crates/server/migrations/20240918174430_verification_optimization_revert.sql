-- Add migration script here
BEGIN;

-- Step 1: Remove the new primary key constraint from primary_id column
ALTER TABLE verification_status
DROP CONSTRAINT IF EXISTS verification_status_pkey;

-- Step 2: Drop the new columns
ALTER TABLE verification_status
DROP COLUMN IF EXISTS primary_id,
DROP COLUMN IF EXISTS project_id;

-- Step 3: Restore the primary key constraint to the existing id column
ALTER TABLE verification_status
ADD CONSTRAINT verification_status_pkey PRIMARY KEY (id);

COMMIT;