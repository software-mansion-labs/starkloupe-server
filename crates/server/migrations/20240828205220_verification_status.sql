DROP TABLE IF EXISTS verification_status;
CREATE TABLE verification_status (
    id UUID DEFAULT uuid_generate_v4 () PRIMARY KEY,
    network VARCHAR(100),
    class_hash VARCHAR(66),
    status VARCHAR(10) NOT NULL,
    error_message TEXT,
    created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY (class_hash) REFERENCES contract_classes (hash)
);
