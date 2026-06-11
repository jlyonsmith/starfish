-- Automatically update 'modified' on UPDATE
CREATE OR REPLACE FUNCTION set_modified()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
  NEW.modified = NOW();
  RETURN NEW;
END;
$$;

DO $$
BEGIN
  IF NOT EXISTS (SELECT FROM pg_catalog.pg_roles WHERE rolname = 'sf_server') THEN
    CREATE USER sf_server WITH PASSWORD 'snoopy';
  END IF;
END$$;

CREATE TABLE IF NOT EXISTS users (
  id          BIGSERIAL    PRIMARY KEY,
  email       TEXT         NOT NULL UNIQUE,
  first_name  TEXT         NOT NULL,
  last_name   TEXT         NOT NULL,
  alias       TEXT         NOT NULL UNIQUE,
  created     TIMESTAMPTZ  NOT NULL DEFAULT NOW(),
  modified    TIMESTAMPTZ  NOT NULL DEFAULT NOW()
);

CREATE OR REPLACE TRIGGER users_set_modified
  BEFORE UPDATE ON users
  FOR EACH ROW EXECUTE FUNCTION set_modified();

CREATE TABLE IF NOT EXISTS user_ssh_keys (
  user_id  BIGINT       NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  key      TEXT         NOT NULL,
  name     TEXT         NOT NULL,
  created  TIMESTAMPTZ  NOT NULL DEFAULT NOW(),
  PRIMARY KEY (user_id, key)
);

CREATE TABLE IF NOT EXISTS host_groups (
  id        BIGSERIAL    PRIMARY KEY,
  name      TEXT         NOT NULL UNIQUE,
  created   TIMESTAMPTZ  NOT NULL DEFAULT NOW(),
  modified  TIMESTAMPTZ  NOT NULL DEFAULT NOW()
);

CREATE OR REPLACE TRIGGER host_groups_set_modified
  BEFORE UPDATE ON host_groups
  FOR EACH ROW EXECUTE FUNCTION set_modified();

CREATE TABLE IF NOT EXISTS host_group_users (
  host_group_id  BIGINT   NOT NULL REFERENCES host_groups(id) ON DELETE CASCADE,
  user_id        BIGINT   NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  is_group_admin BOOLEAN  NOT NULL DEFAULT FALSE,
  PRIMARY KEY (host_group_id, user_id)
);

CREATE TABLE IF NOT EXISTS hosts (
  id              BIGSERIAL    PRIMARY KEY,
  host_group_id   BIGINT       NOT NULL REFERENCES host_groups(id) ON DELETE CASCADE,
  host_name       TEXT         NOT NULL,
  os_description  TEXT,
  created         TIMESTAMPTZ  NOT NULL DEFAULT NOW(),
  modified        TIMESTAMPTZ  NOT NULL DEFAULT NOW()
);

CREATE OR REPLACE TRIGGER hosts_set_modified
  BEFORE UPDATE ON hosts
  FOR EACH ROW EXECUTE FUNCTION set_modified();

CREATE TABLE IF NOT EXISTS host_users (
  host_id     BIGINT       NOT NULL REFERENCES hosts(id) ON DELETE CASCADE,
  user_id     BIGINT       NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  uid         INTEGER      NOT NULL,
  gid         INTEGER      NOT NULL,
  created     TIMESTAMPTZ  NOT NULL DEFAULT NOW(),
  is_sudoer   BOOLEAN      NOT NULL DEFAULT FALSE,
  PRIMARY KEY (host_id, user_id)
);
