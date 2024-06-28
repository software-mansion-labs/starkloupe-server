UPDATE contract_classes
SET 
    is_sierra_debug_info = COALESCE(is_sierra_debug_info, false),
    is_cairo_debug_info = COALESCE(is_cairo_debug_info, false),
    is_source_code = COALESCE(is_source_code, false);

-- Alter columns to be NOT NULL
ALTER TABLE contract_classes
    ALTER COLUMN is_sierra_debug_info SET NOT NULL,
    ALTER COLUMN is_cairo_debug_info SET NOT NULL,
    ALTER COLUMN is_source_code SET NOT NULL;