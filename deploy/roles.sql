-- Least privilege roles for Starfish.
--
-- Run this as a superuser, AFTER the schema exists, because the GRANTs below
-- only reach tables that are already there:
--
--   createdb starfish
--   starfish-admin -p postgresql://starfish_owner@db/starfish \
--       --password-file /etc/starfish/owner.password init-db
--   psql -d starfish -f roles.sql \
--       -v db_name=starfish \
--       -v owner_password='...' -v controller_password='...'
--
-- `scripts/install-controller.sh` does all of the above.  Running it, or this
-- file, a second time is safe: every statement here converges rather than
-- failing on what already exists.
--
-- Better than either password: create the roles without one and use peer
-- authentication over a Unix socket, where the operating system user *is* the
-- database role and there is nothing to leak.  Pass an empty password for a
-- role to leave whatever it already has alone.
--
-- `starfish-admin` is a convenience layer over SQL, not a security boundary:
-- anyone holding these credentials can use psql instead.  All the enforcement
-- has to be here.

\if :{?db_name}
\else
\set db_name 'starfish'
\endif
\if :{?owner_password}
\else
\set owner_password ''
\endif
\if :{?controller_password}
\else
\set controller_password ''
\endif

-- ---------------------------------------------------------------------------
-- Owner.  Owns the tables, so only it can DROP or ALTER them.  Used by
-- `starfish-admin init-db` and nothing else; it should not be the role the
-- controller or administrators connect with day to day.
-- ---------------------------------------------------------------------------
DO $$
BEGIN
    IF NOT EXISTS (SELECT FROM pg_roles WHERE rolname = 'starfish_owner') THEN
        CREATE ROLE starfish_owner LOGIN;
    END IF;
END
$$;

-- Setting a password is conditional, so a re-run that passes none leaves the
-- existing one alone and a peer authenticated deployment never grows one.
-- psql does not expand :variables inside a DO block's body, so the statement is
-- built here and run by expanding it on the next line; `format %L` quotes the
-- password, and an empty one leaves nothing to run but a bare semicolon.
SELECT CASE WHEN :'owner_password' = '' THEN ''
            ELSE format('ALTER ROLE starfish_owner PASSWORD %L', :'owner_password')
       END AS set_owner_password \gset
:set_owner_password ;

-- ---------------------------------------------------------------------------
-- Controller.  This is the network facing component, and it needs almost
-- nothing: it reads configuration, and records heartbeats on two columns.
--
-- Deliberately has no INSERT, UPDATE or DELETE on anything else, so a
-- compromised controller cannot persist a change to who has access.  Note that
-- it can still send agents whatever it likes over an existing connection, since
-- agents trust it; this limits persistence, not a live compromise.
-- ---------------------------------------------------------------------------
DO $$
BEGIN
    IF NOT EXISTS (SELECT FROM pg_roles WHERE rolname = 'starfishd') THEN
        CREATE ROLE starfishd LOGIN;
    END IF;
END
$$;

SELECT CASE WHEN :'controller_password' = '' THEN ''
            ELSE format('ALTER ROLE starfishd PASSWORD %L', :'controller_password')
       END AS set_controller_password \gset
:set_controller_password ;

GRANT CONNECT ON DATABASE :"db_name" TO starfishd;
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
DO $$
BEGIN
    IF NOT EXISTS (SELECT FROM pg_roles WHERE rolname = 'starfish_admins') THEN
        CREATE ROLE starfish_admins;
    END IF;
END
$$;

GRANT CONNECT ON DATABASE :"db_name" TO starfish_admins;
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
REVOKE ALL ON DATABASE :"db_name" FROM PUBLIC;
REVOKE ALL ON SCHEMA public FROM PUBLIC;
