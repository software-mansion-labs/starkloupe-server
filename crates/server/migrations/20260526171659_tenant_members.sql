CREATE TABLE tenant_members (
  id                UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  tenant_id         UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
  github_email      TEXT NOT NULL,
  added_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
  added_by_email    TEXT NOT NULL,
  removed_at        TIMESTAMPTZ NULL,
  removed_by_email  TEXT NULL,

  CONSTRAINT uniq_tenant_member UNIQUE (tenant_id, github_email)
);

CREATE INDEX idx_tenant_members_active
  ON tenant_members(tenant_id)
  WHERE removed_at IS NULL;