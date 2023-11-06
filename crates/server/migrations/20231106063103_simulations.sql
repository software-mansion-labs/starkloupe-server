DROP TABLE IF EXISTS simulations;
CREATE TABLE simulations (
    id UUID DEFAULT uuid_generate_v4 (),
    team_id INTEGER NOT NULL,
    chain_id INTEGER NOT NULL,
    block_at INTEGER NOT NULL,
    transaction_type TEXT NOT NULL,
    transaction_version INTEGER NOT NULL,
    created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
);