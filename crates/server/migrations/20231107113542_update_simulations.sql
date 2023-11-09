DROP TABLE IF EXISTS simulations;
CREATE TABLE simulations (
    id UUID DEFAULT uuid_generate_v4(),
    team_id INTEGER NOT NULL,
    chain_id VARCHAR(255) NOT NULL,
    block_at INTEGER NOT NULL,
    transaction_version INTEGER NOT NULL,
    nonce INTEGER NOT NULL,
    max_fee VARCHAR(255) NOT NULL,
    cairo_version VARCHAR(255) NOT NULL,
    wallet_address VARCHAR(255) NOT NULL,
    calldata VARCHAR(255) [],
    created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
);