-- Add migration script here
ALTER TABLE simulations 
ADD COLUMN status VARCHAR(255) DEFAULT 'simulating' NOT NULL;