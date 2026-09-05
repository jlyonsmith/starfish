#!/usr/bin/env bash
# Installs the Starfish controller, as the Database access section of the
# README describes: an unprivileged service account, a database owned by
# `starfish_owner`, and a `starfishd` role that can read everything and write
# three heartbeat columns.
#
# The binaries and the deployment files are expected beside this script, which
# is how the release archive is laid out:
#
#   starfishd  starfish-admin  starfishd.service  starfishd.tmpfiles
#   roles.sql  install-controller.sh
#
# Safe to run again.  Everything is compared before it is written, the database
# is created only if it is missing, and roles.sql converges rather than failing
# on roles that already exist.
set -euo pipefail

SCRIPT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)

CTL_BIN=/usr/local/bin/starfishd
ADMIN_BIN=/usr/local/bin/starfish-admin
ETC_DIR=/etc/starfish
CONF=/etc/starfishd.conf
UNIT=/etc/systemd/system/starfishd.service
DROPIN_DIR=/etc/systemd/system/starfishd.service.d
TMPFILES=/etc/tmpfiles.d/starfishd.conf
SERVICE_USER=starfishd
OWNER_PW_FILE="$ETC_DIR/owner.password"
DB_PW_FILE="$ETC_DIR/db.password"
ADMIN_SOCKET=/run/starfishd/starfishd.sock

SQL_HOST=""
SQL_PORT=""
SQL_DATABASE=""
SUPERUSER=postgres
LISTEN=""
TLS_CERT=""
TLS_KEY=""
ADMIN_GROUP=""
LOG_LEVEL=""
SSL_MODE=""
INTERACTIVE=1
SKIP_DATABASE=0

usage() {
    cat <<'EOF'
Usage: install-controller.sh [options]

  --sql-host HOST          PostgreSQL server address (default: localhost)
  --sql-port PORT          PostgreSQL port (default: 5432)
  --sql-database NAME      database to use or create (default: starfish)
  --superuser NAME         PostgreSQL superuser to create it with
                           (default: postgres)
  --listen ADDR:PORT       address to serve agents on (default: 0.0.0.0:9600)
  --tls-cert PATH          PEM certificate chain, to serve wss://
  --tls-key PATH           PEM private key matching the certificate
  --admin-socket-group G   group allowed to run `starfish-admin refresh`
                           (default: starfish-admins; "-" for none)
  --log-level LEVEL        error, warn, info, debug or trace (default: info)
  --skip-database          do not touch PostgreSQL at all
  --non-interactive        never prompt; use defaults and fail if one is missing
  -h, --help               this message

Values already in /etc/starfishd.conf are reused when a flag is omitted, so a
re-run with no arguments repairs an installation without changing it.
EOF
}

while [ $# -gt 0 ]; do
    case $1 in
        --sql-host)     SQL_HOST=$2; shift 2 ;;
        --sql-port)     SQL_PORT=$2; shift 2 ;;
        --sql-database) SQL_DATABASE=$2; shift 2 ;;
        --superuser)    SUPERUSER=$2; shift 2 ;;
        --listen)       LISTEN=$2; shift 2 ;;
        --tls-cert)     TLS_CERT=$2; shift 2 ;;
        --tls-key)      TLS_KEY=$2; shift 2 ;;
        --admin-socket-group) ADMIN_GROUP=$2; shift 2 ;;
        --log-level)    LOG_LEVEL=$2; shift 2 ;;
        --skip-database) SKIP_DATABASE=1; shift ;;
        --non-interactive) INTERACTIVE=0; shift ;;
        -h|--help)      usage; exit 0 ;;
        *) printf 'Unknown option: %s\n\n' "$1" >&2; usage >&2; exit 2 ;;
    esac
done

# --- output ----------------------------------------------------------------

CHANGES=0

info()    { printf '\n\033[32m==>\033[0m %s\n' "$*"; }
changed() { CHANGES=$((CHANGES + 1)); printf '  \033[33mchanged\033[0m  %s\n' "$*"; }
same()    { printf '  ok       %s\n' "$*"; }
note()    { printf '  note     %s\n' "$*"; }
fail()    { printf '\n\033[31mFAILED:\033[0m %s\n' "$*" >&2; exit 1; }

# Installs SRC at DEST unless an identical file is already there with the same
# owner and mode.  Returns 0 when it wrote something, so callers can tell
# whether a reload is needed.
install_file() {
    local src=$1 dest=$2 mode=$3 owner=$4 group=$5

    # The mode is compared as a number: `stat` prints 755 where the argument
    # here is 0755, and comparing those as strings makes every file look
    # different on every run.
    if [ -f "$dest" ] \
        && cmp -s "$src" "$dest" \
        && [ "$(stat -c '%U:%G' "$dest")" = "$owner:$group" ] \
        && [ "$((8#$(stat -c '%a' "$dest")))" -eq "$((8#$mode))" ]; then
        same "$dest"
        return 1
    fi

    install -o "$owner" -g "$group" -m "$mode" -D "$src" "$dest"
    changed "$dest"
}

conf_value() {
    local key=$1 file=$2
    [ -f "$file" ] || return 0
    sed -n "s/^${key} *= *\"\{0,1\}\([^\"]*\)\"\{0,1\} *$/\1/p" "$file" | head -1
}

prompt() {
    local var=$1 question=$2 default=${3:-} answer

    if [ "$INTERACTIVE" -eq 0 ]; then
        printf -v "$var" '%s' "$default"
        return
    fi

    if [ -n "$default" ]; then
        read -r -p "$question [$default]: " answer
        answer=${answer:-$default}
    else
        read -r -p "$question: " answer
    fi

    printf -v "$var" '%s' "$answer"
}

gen_password() { openssl rand -base64 32 | tr -d '=+/' | cut -c1-32; }

is_local() {
    case $1 in
        localhost|127.0.0.1|::1) return 0 ;;
        *) return 1 ;;
    esac
}

# --- preconditions ----------------------------------------------------------

[ "$(id -u)" -eq 0 ] || fail "this must run as root"

command -v systemctl > /dev/null || fail "this host does not use systemd"

for file in starfishd starfish-admin starfishd.service starfishd.tmpfiles roles.sql; do
    [ -f "$SCRIPT_DIR/$file" ] || fail "$file is not beside this script, in $SCRIPT_DIR"
done

if command -v file > /dev/null; then
    case "$(file -b "$SCRIPT_DIR/starfishd")" in
        *ELF*) ;;
        *) fail "starfishd is not a Linux executable" ;;
    esac
fi

if [ "$SKIP_DATABASE" -eq 0 ]; then
    command -v psql > /dev/null \
        || fail "psql is not installed (apt install postgresql-client), or use --skip-database"
    command -v openssl > /dev/null || fail "openssl is not installed"
fi

# --- settings ---------------------------------------------------------------

info "Configuration"

# Anything already configured is the default, so a re-run with no flags is a
# no-op rather than a reset.
EXISTING_URL=$(conf_value sql_server "$CONF")
if [ -n "$EXISTING_URL" ]; then
    rest=${EXISTING_URL#*://}
    rest=${rest#*@}
    hostport=${rest%%/*}
    dbpart=${rest#*/}
    [ -n "$SQL_HOST" ] || SQL_HOST=${hostport%%:*}
    [ -n "$SQL_PORT" ] || { [ "$hostport" = "${hostport%:*}" ] || SQL_PORT=${hostport##*:}; }
    [ -n "$SQL_DATABASE" ] || SQL_DATABASE=${dbpart%%\?*}
fi

[ -n "$LISTEN" ] || LISTEN=$(conf_value listen "$CONF")
[ -n "$TLS_CERT" ] || TLS_CERT=$(conf_value tls_cert "$CONF")
[ -n "$TLS_KEY" ] || TLS_KEY=$(conf_value tls_key "$CONF")
[ -n "$ADMIN_GROUP" ] || ADMIN_GROUP=$(conf_value admin_socket_group "$CONF")
[ -n "$LOG_LEVEL" ] || LOG_LEVEL=$(conf_value log_level "$CONF")

[ -n "$SQL_HOST" ] || prompt SQL_HOST "PostgreSQL server address" localhost
[ -n "$SQL_PORT" ] || prompt SQL_PORT "PostgreSQL port" 5432
[ -n "$SQL_DATABASE" ] || prompt SQL_DATABASE "Database name" starfish
[ -n "$LISTEN" ] || prompt LISTEN "Address to serve agents on" 0.0.0.0:9600

# Two security choices worth stopping for.  Both default to the safer answer.
if [ -z "$TLS_CERT" ] && [ "$INTERACTIVE" -eq 1 ]; then
    printf '\n  Without a certificate the controller serves plain ws://, and agent\n'
    printf '  traffic (including configuration) crosses the network in the clear.\n'
    read -r -p "  PEM certificate chain (blank for plain ws://): " TLS_CERT || true
    TLS_CERT=$(printf '%s' "$TLS_CERT" | tr -d '[:space:]')

    if [ -n "$TLS_CERT" ]; then
        read -r -p "  PEM private key: " TLS_KEY || true
        TLS_KEY=$(printf '%s' "$TLS_KEY" | tr -d '[:space:]')
    fi
fi

if [ -z "$ADMIN_GROUP" ] && [ "$INTERACTIVE" -eq 1 ]; then
    printf '\n  The admin socket is mode 0600 unless it belongs to a group, so only\n'
    printf '  root could run `starfish-admin refresh`.  Naming a group lets\n'
    printf '  administrators with their own accounts do it.\n'
    prompt ADMIN_GROUP "  Group for the admin socket (\"-\" for none)" starfish-admins
fi

[ "$ADMIN_GROUP" != "-" ] || ADMIN_GROUP=""

if [ -n "$TLS_CERT" ] || [ -n "$TLS_KEY" ]; then
    [ -n "$TLS_CERT" ] && [ -n "$TLS_KEY" ] \
        || fail "a TLS certificate and key are needed together, or neither"
    [ -f "$TLS_CERT" ] || fail "no such certificate: $TLS_CERT"
    [ -f "$TLS_KEY" ] || fail "no such key: $TLS_KEY"
fi

# `prefer` encrypts when the server offers it but never checks who answered, so
# a remote database is asked for a verified connection instead.
if is_local "$SQL_HOST"; then
    SSL_MODE=""
else
    SSL_MODE="?sslmode=verify-full&sslrootcert=system"
fi

printf '  database    postgresql://%s@%s:%s/%s\n' "$SERVICE_USER" "$SQL_HOST" "$SQL_PORT" "$SQL_DATABASE"
printf '  listen      %s (%s)\n' "$LISTEN" "$([ -n "$TLS_CERT" ] && echo wss:// || echo ws://)"
printf '  admin group %s\n' "${ADMIN_GROUP:-none, so the socket is 0600}"

# --- the service account ----------------------------------------------------

info "Service account"

if getent passwd "$SERVICE_USER" > /dev/null; then
    same "user $SERVICE_USER"
else
    adduser --system --group --no-create-home --shell /usr/sbin/nologin "$SERVICE_USER"
    changed "user $SERVICE_USER"
fi

# The controller refuses to start if this group does not exist, rather than
# falling back to something more open.
if [ -n "$ADMIN_GROUP" ]; then
    if getent group "$ADMIN_GROUP" > /dev/null; then
        same "group $ADMIN_GROUP"
    else
        groupadd --system "$ADMIN_GROUP"
        changed "group $ADMIN_GROUP"
    fi
fi

# Holds the password files, so it is readable by the service account and root
# and nobody else.
if [ -d "$ETC_DIR" ] && [ "$(stat -c '%U:%G %a' "$ETC_DIR")" = "root:$SERVICE_USER 750" ]; then
    same "$ETC_DIR"
else
    install -o root -g "$SERVICE_USER" -m 0750 -d "$ETC_DIR"
    changed "$ETC_DIR"
fi

# --- binaries ---------------------------------------------------------------

info "Binaries"

install_file "$SCRIPT_DIR/starfishd" "$CTL_BIN" 0755 root root || true
install_file "$SCRIPT_DIR/starfish-admin" "$ADMIN_BIN" 0755 root root || true

# --- the database -----------------------------------------------------------

if [ "$SKIP_DATABASE" -eq 1 ]; then
    info "Database (skipped)"
    note "--skip-database was given; nothing in PostgreSQL was touched"
else
    info "Database"

    # Passwords are generated once and then reused, so a re-run converges on the
    # same roles instead of rotating credentials the controller is using.
    if [ -f "$OWNER_PW_FILE" ]; then
        same "$OWNER_PW_FILE"
    else
        (umask 077 && gen_password > "$OWNER_PW_FILE")
        changed "$OWNER_PW_FILE"
    fi
    chown root:root "$OWNER_PW_FILE"
    chmod 0600 "$OWNER_PW_FILE"

    if [ -f "$DB_PW_FILE" ]; then
        same "$DB_PW_FILE"
    else
        (umask 077 && gen_password > "$DB_PW_FILE")
        changed "$DB_PW_FILE"
    fi
    # The controller reads this as itself, and both tools refuse a password file
    # that anyone else can read, so it is 0600 owned by the service account.
    chown "$SERVICE_USER:$SERVICE_USER" "$DB_PW_FILE"
    chmod 0600 "$DB_PW_FILE"

    OWNER_PASSWORD=$(cat "$OWNER_PW_FILE")
    DB_PASSWORD=$(cat "$DB_PW_FILE")

    # A local superuser authenticates by peer over the Unix socket, which is how
    # Ubuntu ships PostgreSQL; a remote one needs a password.
    if is_local "$SQL_HOST"; then
        getent passwd "$SUPERUSER" > /dev/null \
            || fail "there is no local '$SUPERUSER' account; is PostgreSQL installed here?"
        psql_super() { sudo -u "$SUPERUSER" psql -v ON_ERROR_STOP=1 -X -q "$@"; }
    else
        if [ -z "${PGPASSWORD:-}" ] && [ "$INTERACTIVE" -eq 1 ]; then
            read -r -s -p "  Password for $SUPERUSER@$SQL_HOST: " PGPASSWORD
            printf '\n'
            export PGPASSWORD
        fi
        psql_super() {
            psql -v ON_ERROR_STOP=1 -X -q \
                -h "$SQL_HOST" -p "$SQL_PORT" -U "$SUPERUSER" "$@"
        }
    fi

    psql_super -d postgres -c 'SELECT 1' > /dev/null \
        || fail "cannot connect to PostgreSQL as $SUPERUSER"

    # The owner has to exist before the database can be owned by it, and before
    # `init-db` can create tables as it.
    #
    # Fed on standard input rather than with -c, because psql only interpolates
    # :variables in input it reads, and the password must not be pasted into the
    # command line where `ps` would show it.
    psql_super -d postgres -v pw="$OWNER_PASSWORD" -f - > /dev/null <<'SQL'
DO $$
BEGIN
    IF NOT EXISTS (SELECT FROM pg_roles WHERE rolname = 'starfish_owner') THEN
        CREATE ROLE starfish_owner LOGIN;
    END IF;
END
$$;
ALTER ROLE starfish_owner PASSWORD :'pw';
SQL

    exists=$(psql_super -d postgres -At \
        -c "SELECT 1 FROM pg_database WHERE datname = '$SQL_DATABASE';")

    if [ "$exists" = "1" ]; then
        same "database $SQL_DATABASE"
    else
        # Owned by starfish_owner so that it, and not the controller or an
        # administrator, is the role that can DROP or ALTER the tables.
        psql_super -d postgres \
            -c "CREATE DATABASE \"$SQL_DATABASE\" OWNER starfish_owner;" > /dev/null
        changed "database $SQL_DATABASE"
    fi

    # The schema is created, not migrated, so this runs once and is skipped
    # afterwards.  `starfish-admin init-db` connects as the owner.
    schema=$(psql_super -d "$SQL_DATABASE" -At \
        -c "SELECT to_regclass('public.hosts') IS NOT NULL;")

    if [ "$schema" = "t" ]; then
        same "schema in $SQL_DATABASE"
    else
        "$ADMIN_BIN" \
            -p "postgresql://starfish_owner@$SQL_HOST:$SQL_PORT/$SQL_DATABASE$SSL_MODE" \
            --password-file "$OWNER_PW_FILE" init-db > /dev/null
        changed "schema in $SQL_DATABASE"
    fi

    # Only now, with the tables in place, can the grants reach them.
    psql_super -d "$SQL_DATABASE" -f "$SCRIPT_DIR/roles.sql" \
        -v db_name="$SQL_DATABASE" \
        -v owner_password="$OWNER_PASSWORD" \
        -v controller_password="$DB_PASSWORD" > /dev/null
    same "roles and grants applied"
fi

# --- TLS --------------------------------------------------------------------

if [ -n "$TLS_CERT" ]; then
    info "TLS"

    # Referenced where they are rather than copied, so a renewal by certbot or
    # anything else reaches the controller without reinstalling.  They do have
    # to be readable by the service account, and a key that is not is a startup
    # failure worth catching now.
    for path in "$TLS_CERT" "$TLS_KEY"; do
        sudo -u "$SERVICE_USER" test -r "$path" \
            || fail "$path is not readable by $SERVICE_USER (try: chgrp $SERVICE_USER $path && chmod 0640 $path)"
    done

    same "$TLS_CERT"
    same "$TLS_KEY"
fi

# --- configuration ----------------------------------------------------------

info "Controller configuration"

conf_tmp=$(mktemp)
{
    printf 'sql_server = "postgresql://%s@%s:%s/%s%s"\n' \
        "$SERVICE_USER" "$SQL_HOST" "$SQL_PORT" "$SQL_DATABASE" "$SSL_MODE"
    [ "$SKIP_DATABASE" -eq 1 ] || printf 'password_file = "%s"\n' "$DB_PW_FILE"
    printf 'listen = "%s"\n' "$LISTEN"
    [ -z "$TLS_CERT" ] || printf 'tls_cert = "%s"\n' "$TLS_CERT"
    [ -z "$TLS_KEY" ] || printf 'tls_key = "%s"\n' "$TLS_KEY"
    printf 'admin_socket = "%s"\n' "$ADMIN_SOCKET"
    [ -z "$ADMIN_GROUP" ] || printf 'admin_socket_group = "%s"\n' "$ADMIN_GROUP"
    [ -z "$LOG_LEVEL" ] || printf 'log_level = "%s"\n' "$LOG_LEVEL"
} > "$conf_tmp"

if install_file "$conf_tmp" "$CONF" 0640 root "$SERVICE_USER"; then
    conf_changed=1
else
    conf_changed=0
fi
rm -f "$conf_tmp"

# --- the unit ---------------------------------------------------------------

info "systemd unit"

unit_changed=0
install_file "$SCRIPT_DIR/starfishd.service" "$UNIT" 0644 root root && unit_changed=1 || true

# Ordering after postgresql.service is in the unit already; wanting it only
# makes sense when the database really is on this host.
if is_local "$SQL_HOST"; then
    if systemctl list-unit-files postgresql.service > /dev/null 2>&1 \
        && systemctl cat postgresql.service > /dev/null 2>&1; then
        dropin_tmp=$(mktemp)
        printf '[Unit]\nWants=postgresql.service\n' > "$dropin_tmp"
        install_file "$dropin_tmp" "$DROPIN_DIR/postgresql.conf" 0644 root root \
            && unit_changed=1 || true
        rm -f "$dropin_tmp"
    fi
fi

# The controller gives its own socket away to the admin group, and the kernel
# only allows that for a group the process is actually in.  Without this it
# starts, fails on the socket, and restarts forever.
admin_dropin="$DROPIN_DIR/admin-group.conf"

if [ -n "$ADMIN_GROUP" ]; then
    dropin_tmp=$(mktemp)
    printf '[Service]\nSupplementaryGroups=%s\n' "$ADMIN_GROUP" > "$dropin_tmp"
    install_file "$dropin_tmp" "$admin_dropin" 0644 root root && unit_changed=1 || true
    rm -f "$dropin_tmp"
elif [ -f "$admin_dropin" ]; then
    rm -f "$admin_dropin"
    unit_changed=1
    changed "$admin_dropin removed"
fi

# /run is emptied on boot, so the compatibility symlink that lets
# `starfish-admin` find the socket at its default path is recreated by tmpfiles
# rather than made once here.
if install_file "$SCRIPT_DIR/starfishd.tmpfiles" "$TMPFILES" 0644 root root; then
    systemd-tmpfiles --create "$TMPFILES" > /dev/null 2>&1 || true
fi

[ "$unit_changed" -eq 0 ] || systemctl daemon-reload

if systemctl is-enabled --quiet starfishd 2>/dev/null; then
    same "starfishd is enabled"
else
    systemctl enable starfishd > /dev/null 2>&1
    changed "starfishd enabled"
fi

if [ "$conf_changed" -eq 1 ] || [ "$unit_changed" -eq 1 ] \
    || ! systemctl is-active --quiet starfishd; then
    systemctl restart starfishd
    changed "starfishd (re)started"
else
    same "starfishd is running"
fi

# --- summary ----------------------------------------------------------------

if [ "$CHANGES" -eq 0 ]; then
    printf '\n\033[32mAlready configured; nothing changed.\033[0m\n'
else
    printf '\n\033[32mController installed.\033[0m %s change(s).\n' "$CHANGES"
fi

if [ -n "$ADMIN_GROUP" ]; then
    printf 'Add administrators to the %s group so they can run `starfish-admin refresh`:\n' "$ADMIN_GROUP"
    printf '    usermod -aG %s <user>\n' "$ADMIN_GROUP"
fi

printf 'Follow it with: journalctl -u starfishd -f\n'
