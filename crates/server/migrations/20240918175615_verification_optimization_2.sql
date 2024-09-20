-- Add migration script here

BEGIN;

-- Step 1: Add new columns without constraints
ALTER TABLE verification_status
ADD COLUMN primary_id SERIAL,
ADD COLUMN project_id INTEGER;

-- Step 2: Remove the primary key constraint from the existing id column
ALTER TABLE verification_status
DROP CONSTRAINT IF EXISTS verification_status_pkey;

-- Step 3: Set the new primary_id for existing rows
UPDATE verification_status
SET primary_id = DEFAULT;

-- Step 4: Remove default on 'id' column
ALTER TABLE verification_status
ALTER COLUMN id DROP DEFAULT;

-- Step 5: Add the new primary key constraint to primary_id column
ALTER TABLE verification_status
ADD CONSTRAINT verification_status_pkey PRIMARY KEY (primary_id);

-- Step 6: Rename column error_message to message
ALTER TABLE verification_status
RENAME COLUMN error_message TO message;

COMMIT;