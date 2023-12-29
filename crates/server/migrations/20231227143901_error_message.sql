ALTER TABLE
    simulations
ADD
    COLUMN error_message VARCHAR DEFAULT NULL,
ADD
    COLUMN error_contract_address VARCHAR DEFAULT NULL;