CREATE TABLE api_keys (
  id                UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  tenant_id         UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
  key_hash          BYTEA NOT NULL UNIQUE,
  key_prefix        TEXT NOT NULL,
  status            TEXT NOT NULL DEFAULT 'active'
                     CHECK (status IN ('active','revoked')),
  created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
  created_by_email  TEXT NOT NULL,
  revoked_at        TIMESTAMPTZ NULL,
  revoked_by_email  TEXT NULL
);

-- at most one active shared key per tenant; revoked rows accumulate as history
CREATE UNIQUE INDEX uniq_active_key_per_tenant
  ON api_keys(tenant_id)
  WHERE status = 'active';

-- hot lookup path on every authenticated request
CREATE INDEX idx_api_keys_hash ON api_keys(key_hash) WHERE status = 'active';