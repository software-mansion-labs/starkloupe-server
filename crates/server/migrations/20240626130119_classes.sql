CREATE TABLE contract_classes (
    hash VARCHAR(66) PRIMARY KEY,
    is_sierra_debug_info BOOLEAN,
    is_cairo_debug_info BOOLEAN,
    is_source_code BOOLEAN
);