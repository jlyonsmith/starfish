-- Least privilege roles for Starfish.
--
-- Run this as a superuser, AFTER the schema exists, because the GRANTs below
-- only reach tables that are already there:
--
--   createdb starfish
--   starfish-admin -p postgresql://starfish_owner@db/starfish \
--       --password-file /etc/starfish/owner.password init-db
--   psql -d starfish -f roles.sql
--
-- Replace every CHANGEME before running, or better, create the roles without
-- passwords and use peer authentication over a Unix socket, where the operating
-- system user *is* the database role and there is no password to leak.
--
-- `starfish-admin` is a convenience layer over SQL, not a security boundary:
-- anyone holding these credentials can use psql instead.  All the enforcement
-- has to be here.

-- ---------------------------------------------------------------------------
-- Owner.  Owns the tables, so only it can DROP or ALTER them.  Used by
-- `starfish-admin init-db` and nothing else; it should not be the role the
-- controller or administrators connect with day to day.
-- ---------------------------------------------------------------------------
CREATE ROLE starfish_owner LOGIN PASSWORD 'CHANGEME';

-- ---------------------------------------------------------------------------
-- Controller.  This is the network facing component, and it needs almost
-- nothing: it reads configuration, and records heartbeats on two columns.
--
-- Deliberately has no INSERT, UPDATE or DELETE on anything else, so a
-- compromised controller cannot persist a change to who has access.  Note that
-- it can still send agents whatever it likes over an existing connection, since
-- agents trust it; this limits persistence, not a live compromise.
-- ---------------------------------------------------------------------------
CREATE ROLE starfishd LOGIN PASSWORD 'CHANGEME';

GRANT CONNECT ON DATABASE starfish TO starfishd;
GRANT USAGE ON SCHEMA public TO starfishd;
GRANT SELECT ON ALL TABLES IN SCHEMA public TO starfishd;
-- `updated_at` is in the list because the schema marks it `#[auto]`, so every
-- toasty UPDATE writes it too.  Leaving it out makes the heartbeat fail with
-- "permission denied for table hosts".  It is a timestamp, not an access
-- decision, so granting it costs nothing.
GRANT UPDATE (contacted_at, next_heartbeat_at, updated_at) ON hosts TO starfishd;

-- Tables added by a later schema change should be readable too.
ALTER DEFAULT PRIVILEGES FOR ROLE starfish_owner IN SCHEMA public
    GRANT SELECT ON TABLES TO starfishd;

-- ---------------------------------------------------------------------------
-- Administrators.  A group role holding the grants, with one login per person
-- so `session_user` names a human.  A shared login makes any audit trail
-- worthless.
-- ---------------------------------------------------------------------------
CREATE ROLE starfish_admins;

GRANT CONNECT ON DATABASE starfish TO starfish_admins;
GRANT USAGE ON SCHEMA public TO starfish_admins;
GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA public TO starfish_admins;

ALTER DEFAULT PRIVILEGES FOR ROLE starfish_owner IN SCHEMA public
    GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO starfish_admins;

-- No sequence grants are needed: the schema uses identity columns, whose
-- implicit sequences are covered by the table privilege.

-- One of these per administrator.  With peer authentication over a Unix socket
-- the name must match their operating system account and no password is needed.
-- CREATE ROLE jls LOGIN IN ROLE starfish_admins;

-- ---------------------------------------------------------------------------
-- Nobody else.
-- ---------------------------------------------------------------------------
REVOKE ALL ON DATABASE starfish FROM PUBLIC;
REVOKE ALL ON SCHEMA public FROM PUBLIC;
