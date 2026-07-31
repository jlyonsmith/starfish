CREATE TABLE IF NOT EXISTS "user" (
  "id" BIGSERIAL,
  "alias" TEXT NOT NULL UNIQUE,
  "email" TEXT NOT NULL UNIQUE,
  "first_name" TEXT NOT NULL,
  "last_name" TEXT NOT NULL,
  "modified_at" TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  "created_at" TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  PRIMARY KEY ("id")
);

CREATE TABLE IF NOT EXISTS "ssh_key" (
  "id" BIGSERIAL,
  "user_id" BIGINT NOT NULL REFERENCES "user"("id") ON DELETE CASCADE,
  "key" TEXT NOT NULL,
  "name" TEXT NOT NULL,
  "modified_at" TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  "created_at" TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  PRIMARY KEY ("id")
);

CREATE TABLE IF NOT EXISTS "host_group" (
  "id" BIGSERIAL,
  "name" TEXT NOT NULL,
  "created_at" TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  "modified_at" TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  PRIMARY KEY ("id")
);

CREATE TABLE IF NOT EXISTS "host" (
  "id" BIGSERIAL,
  "host_group_id" BIGINT NOT NULL REFERENCES "host_group"("id") ON DELETE CASCADE,
  "name" TEXT NOT NULL,
  "os" TEXT NOT NULL,
  "created_at" TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  "modified_at" TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  PRIMARY KEY ("id")
);

CREATE TABLE IF NOT EXISTS "host_group_security_group" (
  "id" BIGSERIAL,
  "host_group_id" BIGINT NOT NULL REFERENCES "host_group"("id") ON DELETE CASCADE,
  "sec_group" TEXT NOT NULL,
  "created_at" TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  "modified_at" TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  PRIMARY KEY ("id")
);

CREATE TABLE IF NOT EXISTS "host_group_user" (
  "host_group_id" BIGINT NOT NULL REFERENCES "host_group"("id") ON DELETE CASCADE,
  "user_id" BIGINT NOT NULL REFERENCES "user"("id") ON DELETE CASCADE,
  "is_admin" BOOLEAN NOT NULL,
  "is_sudoer" BOOLEAN NOT NULL,
  "created_at" TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  "modified_at" TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  PRIMARY KEY ("host_group_id", "user_id")
);

CREATE OR REPLACE FUNCTION set_modified_at() RETURNS TRIGGER AS $$
BEGIN
  NEW."modified_at" = NOW();
  RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE OR REPLACE TRIGGER set_modified_at_user BEFORE UPDATE ON "user"
  FOR EACH ROW EXECUTE FUNCTION set_modified_at();

CREATE OR REPLACE TRIGGER set_modified_at_ssh_key BEFORE UPDATE ON "ssh_key"
  FOR EACH ROW EXECUTE FUNCTION set_modified_at();

CREATE OR REPLACE TRIGGER set_modified_at_host_group BEFORE UPDATE ON "host_group"
  FOR EACH ROW EXECUTE FUNCTION set_modified_at();

CREATE OR REPLACE TRIGGER set_modified_at_host BEFORE UPDATE ON "host"
  FOR EACH ROW EXECUTE FUNCTION set_modified_at();

CREATE OR REPLACE TRIGGER set_modified_at_host_group_security_group BEFORE UPDATE ON "host_group_security_group"
  FOR EACH ROW EXECUTE FUNCTION set_modified_at();

CREATE OR REPLACE TRIGGER set_modified_at_host_group_user BEFORE UPDATE ON "host_group_user"
  FOR EACH ROW EXECUTE FUNCTION set_modified_at();