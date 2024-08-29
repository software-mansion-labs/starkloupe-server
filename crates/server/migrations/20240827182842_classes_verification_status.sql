ALTER TABLE contract_classes
ADD COLUMN verification_status VARCHAR(20) NOT NULL DEFAULT 'pending';
