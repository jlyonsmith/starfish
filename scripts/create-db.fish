#!/usr/bin/env fish

set DB_NAME "starfish"
set DB_HOST "localhost"
set DB_PORT "5432"
set DB_USER "postgres"
set SCRIPT_DIR (dirname (status --current-filename))

set PSQL "psql -h $DB_HOST -p $DB_PORT -U $DB_USER"

echo "Creating database '$DB_NAME' on $DB_HOST:$DB_PORT..."

# Have to use quotes carefully because eval does double expansion
eval $PSQL -d postgres -c '"CREATE DATABASE '$DB_NAME';"'; \
  or echo "Database '$DB_NAME' already exists, skipping."

echo "Applying schema..."

eval $PSQL -d "$DB_NAME" -f "$SCRIPT_DIR/starfish-schema.sql"

echo "Done."
