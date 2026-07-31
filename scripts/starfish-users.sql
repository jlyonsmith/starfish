DO $$
BEGIN
  IF NOT EXISTS (SELECT FROM pg_catalog.pg_roles WHERE rolname = 'sf_server') THEN
    CREATE USER sf_server WITH PASSWORD 'snoopy';
  END IF;
END$$;
