#!/usr/bin/env bash
# Installs the Starfish agent on an Ubuntu host, as the Quick start section of
# the README describes: an unprivileged service account, the privileged helper
# owned by root, and one sudo rule joining them.
#
# The binaries and the deployment files are expected beside this script, which
# is how the release archive is laid out:
#
#   starfish-agent  starfish-sync  starfish-agent.service
#   starfish-sync.sudoers  install-agent.sh
#
# Safe to run again.  Everything is compared before it is written, so a second
# run on a correctly configured host changes nothing and does not restart the
# service.
set -euo pipefail

SCRIPT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)

AGENT_BIN=/usr/local/bin/starfish-agent
HELPER_DIR=/usr/local/lib/starfish
HELPER_BIN="$HELPER_DIR/starfish-sync"
SUDOERS=/etc/sudoers.d/starfish
SUDOERS_GRANT=/etc/sudoers.d/starfish-sudo
CONF=/etc/starfish_agent.conf
UNIT=/etc/systemd/system/starfish-agent.service
SERVICE_USER=starfish

CONTROLLER_URL=""
AGENT_KEY=""
HEARTBEAT_SECS=""
LOG_LEVEL=""
CA_CERT=""
INTERACTIVE=1

usage() {
    cat <<'EOF'
Usage: install-agent.sh [options]

  --controller-url URL   ws:// or wss:// address of the controller
  --agent-key KEY        the 16 character key `starfish-admin host add` printed
  --heartbeat-secs N     seconds between heartbeats (default: the agent's, 300)
  --log-level LEVEL      error, warn, info, debug or trace (default: info)
  --ca-cert PATH         internal CA to trust, for a wss:// controller
  --non-interactive      never prompt; fail if a required value is missing
  -h, --help             this message

Values already in /etc/starfish_agent.conf are reused when a flag is omitted,
so a re-run with no arguments repairs an installation without changing it.
EOF
}

while [ $# -gt 0 ]; do
    case $1 in
        --controller-url) CONTROLLER_URL=$2; shift 2 ;;
        --agent-key)      AGENT_KEY=$2; shift 2 ;;
        --heartbeat-secs) HEARTBEAT_SECS=$2; shift 2 ;;
        --log-level)      LOG_LEVEL=$2; shift 2 ;;
        --ca-cert)        CA_CERT=$2; shift 2 ;;
        --non-interactive) INTERACTIVE=0; shift ;;
        -h|--help)        usage; exit 0 ;;
        *) printf 'Unknown option: %s\n\n' "$1" >&2; usage >&2; exit 2 ;;
    esac
done

# --- output ----------------------------------------------------------------

CHANGES=0

info()    { printf '\n\033[32m==>\033[0m %s\n' "$*"; }
changed() { CHANGES=$((CHANGES + 1)); printf '  \033[33mchanged\033[0m  %s\n' "$*"; }
same()    { printf '  ok       %s\n' "$*"; }
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

# Reads one setting out of the TOML we generate.  Good enough for a file this
# script wrote itself, and not a general TOML parser.
conf_value() {
    local key=$1 file=$2
    [ -f "$file" ] || return 0
    sed -n "s/^${key} *= *\"\{0,1\}\([^\"]*\)\"\{0,1\} *$/\1/p" "$file" | head -1
}

prompt() {
    local var=$1 question=$2 default=${3:-} answer

    if [ "$INTERACTIVE" -eq 0 ]; then
        [ -n "$default" ] || fail "$var is required and there is nothing to prompt with"
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

# --- preconditions ----------------------------------------------------------

[ "$(id -u)" -eq 0 ] || fail "this must run as root"

command -v systemctl > /dev/null || fail "this host does not use systemd"
command -v visudo > /dev/null || fail "visudo is not installed (apt install sudo)"

for file in starfish-agent starfish-sync starfish-agent.service starfish-sync.sudoers \
    starfish-sudoers; do
    [ -f "$SCRIPT_DIR/$file" ] || fail "$file is not beside this script, in $SCRIPT_DIR"
done

# The agent is a Linux binary and this refuses to install one built for the
# wrong architecture, which otherwise fails much later as "no such file".
if command -v file > /dev/null; then
    case "$(file -b "$SCRIPT_DIR/starfish-agent")" in
        *ELF*) ;;
        *) fail "starfish-agent is not a Linux executable" ;;
    esac
fi

# --- settings ---------------------------------------------------------------

info "Configuration"

[ -n "$CONTROLLER_URL" ] || CONTROLLER_URL=$(conf_value controller_url "$CONF")
[ -n "$AGENT_KEY" ] || AGENT_KEY=$(conf_value agent_key "$CONF")
[ -n "$HEARTBEAT_SECS" ] || HEARTBEAT_SECS=$(conf_value heartbeat_secs "$CONF")
[ -n "$LOG_LEVEL" ] || LOG_LEVEL=$(conf_value log_level "$CONF")

[ -n "$CONTROLLER_URL" ] \
    || prompt CONTROLLER_URL "Controller URL, e.g. wss://starfish.example.com:9600"
[ -n "$AGENT_KEY" ] \
    || prompt AGENT_KEY "Agent key for this host, from \`starfish-admin host add\`"

case $CONTROLLER_URL in
    ws://*|wss://*) ;;
    *) fail "the controller URL must start with ws:// or wss://" ;;
esac

# Matches AgentKey::from_str, so a typo is caught here rather than by an agent
# that starts, is rejected, and backs off.
[ "${#AGENT_KEY}" -eq 16 ] && [[ $AGENT_KEY =~ ^[A-Za-z0-9]+$ ]] \
    || fail "an agent key is exactly 16 alphanumeric characters"

# The one security choice worth making here: an agent verifies the controller
# against the system trust store, so a wss:// controller behind an internal CA
# needs that CA installed before the agent will connect.
case $CONTROLLER_URL in
    ws://*)
        printf '  \033[33mnote\033[0m     ws:// is unencrypted; agents and the controller\n'
        printf '           should use wss:// outside a trusted network\n'
        ;;
    wss://*)
        if [ -z "$CA_CERT" ] && [ "$INTERACTIVE" -eq 1 ]; then
            read -r -p "Internal CA certificate to trust (blank if publicly signed): " CA_CERT
            CA_CERT=$(printf '%s' "$CA_CERT" | tr -d '[:space:]')
        fi
        ;;
esac

[ -z "$CA_CERT" ] || [ -f "$CA_CERT" ] || fail "no such CA certificate: $CA_CERT"

printf '  controller  %s\n' "$CONTROLLER_URL"
printf '  agent key   %s\n' "${AGENT_KEY:0:4}············"

# --- the service account ----------------------------------------------------

info "Service account"

# It owns nothing and logs in nowhere; its only privilege is the sudo rule
# installed below.
if getent passwd "$SERVICE_USER" > /dev/null; then
    same "user $SERVICE_USER"
else
    adduser --system --group --no-create-home --shell /usr/sbin/nologin "$SERVICE_USER"
    changed "user $SERVICE_USER"
fi

# --- binaries ---------------------------------------------------------------

info "Binaries"

# For the sudo rule to mean anything, the helper and every directory above it
# must be root owned and not writable by the agent, or the agent can replace
# the binary root is about to run.
if [ -d "$HELPER_DIR" ] && [ "$(stat -c '%U:%G %a' "$HELPER_DIR")" = "root:root 755" ]; then
    same "$HELPER_DIR"
else
    install -o root -g root -m 0755 -d "$HELPER_DIR"
    changed "$HELPER_DIR"
fi

install_file "$SCRIPT_DIR/starfish-sync" "$HELPER_BIN" 0755 root root || true
install_file "$SCRIPT_DIR/starfish-agent" "$AGENT_BIN" 0755 root root || true

# --- the sudo rule ----------------------------------------------------------

info "Sudo rule"

# Checked before they are installed: a syntactically broken file in sudoers.d
# breaks sudo for everyone on the host, root included.
#
# Two rules, doing unrelated jobs.  The first lets the agent reach root through
# the helper.  The second is what makes `--sudoer` mean anything: managed
# accounts have no password, so without a NOPASSWD rule a user granted sudo
# would be in the group and still unable to run a single command.
install_sudoers() {
    local src=$1 dest=$2 tmp

    tmp=$(mktemp)
    cp "$src" "$tmp"
    chmod 0440 "$tmp"
    visudo -c -q -f "$tmp" || { rm -f "$tmp"; fail "$src is not a valid sudoers file"; }

    install_file "$tmp" "$dest" 0440 root root || true
    rm -f "$tmp"
}

install_sudoers "$SCRIPT_DIR/starfish-sync.sudoers" "$SUDOERS"
install_sudoers "$SCRIPT_DIR/starfish-sudoers" "$SUDOERS_GRANT"

# --- the internal CA --------------------------------------------------------

if [ -n "$CA_CERT" ]; then
    info "Certificate authority"

    ca_dest="/usr/local/share/ca-certificates/$(basename "${CA_CERT%.*}").crt"

    if install_file "$CA_CERT" "$ca_dest" 0644 root root; then
        update-ca-certificates > /dev/null
        changed "system trust store"
    fi
fi

# --- configuration ----------------------------------------------------------

info "Agent configuration"

# Holds the agent key, so it is readable by the service account and nobody
# else.
conf_tmp=$(mktemp)
{
    printf 'controller_url = "%s"\n' "$CONTROLLER_URL"
    printf 'agent_key = "%s"\n' "$AGENT_KEY"
    [ -z "$HEARTBEAT_SECS" ] || printf 'heartbeat_secs = %s\n' "$HEARTBEAT_SECS"
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

if install_file "$SCRIPT_DIR/starfish-agent.service" "$UNIT" 0644 root root; then
    systemctl daemon-reload
    unit_changed=1
else
    unit_changed=0
fi

if systemctl is-enabled --quiet starfish-agent 2>/dev/null; then
    same "starfish-agent is enabled"
else
    systemctl enable starfish-agent > /dev/null 2>&1
    changed "starfish-agent enabled"
fi

# Restarted only when something it reads actually changed, so a re-run on a
# healthy host does not interrupt it.  `restart` rather than `start` because on
# a re-run the service is already up with the previous configuration.
if [ "$conf_changed" -eq 1 ] || [ "$unit_changed" -eq 1 ] \
    || ! systemctl is-active --quiet starfish-agent; then
    systemctl restart starfish-agent
    changed "starfish-agent (re)started"
else
    same "starfish-agent is running"
fi

# --- summary ----------------------------------------------------------------

if [ "$CHANGES" -eq 0 ]; then
    printf '\n\033[32mAlready configured; nothing changed.\033[0m\n'
else
    printf '\n\033[32mAgent installed.\033[0m %s change(s).\n' "$CHANGES"
fi

printf 'Follow it with: journalctl -u starfish-agent -f\n'
