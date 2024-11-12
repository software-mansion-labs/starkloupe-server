CREATE TABLE verification_requests (
    id UUID PRIMARY KEY,
    status VARCHAR(10),
    message TEXT,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP NOT NULL,
    cairo_version VARCHAR(16),
    package_name VARCHAR(256)
);