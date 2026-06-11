ALTER TABLE tenant_members DROP CONSTRAINT uniq_tenant_member;

CREATE UNIQUE INDEX uniq_tenant_member_active
  ON tenant_members (tenant_id, github_email)
  WHERE removed_at IS NULL;
