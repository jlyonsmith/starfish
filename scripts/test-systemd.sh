#!/usr/bin/env bash
# Runs the agent under systemd on a real Ubuntu VM.
#
# `just test-ubuntu` covers everything the privileged helper does to a host, but
# a container has no systemd, so deploy/starfish-agent.service goes untested
# there.  This checks the unit itself: that the agent starts under it, runs
# unprivileged inside its sandbox, and can still reach root through sudo to
# create a real account.
#
# The controller and its database stay on the Mac; only the agent runs in the
# VM, which is how a real deployment looks.
set -euo pipefail

VM=starfish-test
IMAGE=starfish-test:latest
DB_NAME=starfish_systemd
DB_URL="postgresql://postgres@localhost:5432/$DB_NAME"
LISTEN_PORT=19980
STAGE=$(mktemp -d)
ROOT=$(cd "$(dirname "$0")/.." && pwd)
CONTROLLER_PID=""

cd "$ROOT"

info() { printf '\n\033[32m==>\033[0m %s\n' "$*"; }
fail() { printf '\n\033[31mFAILED:\033[0m %s\n' "$*" >&2; exit 1; }

cleanup() {
    if [ -n "$CONTROLLER_PID" ]; then
        kill "$CONTROLLER_PID" 2>/dev/null || true
        # Reaped rather than left to job control, which would print "Terminated".
        wait "$CONTROLLER_PID" 2>/dev/null || true
    fi
    rm -rf "$STAGE"
    psql -h localhost -U postgres -d postgres -q -c "DROP DATABASE IF EXISTS $DB_NAME;" 2>/dev/null || true
}
trap cleanup EXIT

for tool in limactl docker psql; do
    command -v "$tool" > /dev/null || fail "$tool is not installed"
done

# --- Linux binaries -------------------------------------------------------
# Reuses the image `just test-ubuntu` builds rather than a second toolchain:
# cross compiling from macOS needs a linker that is not installed by default.
info "Taking the Linux binaries out of $IMAGE"
docker image inspect "$IMAGE" > /dev/null 2>&1 \
    || fail "$IMAGE does not exist. Run \`just docker-image\` first."

container=$(docker create "$IMAGE")
docker cp "$container:/usr/local/bin/starfish-agent" "$STAGE/starfish-agent"
docker cp "$container:/usr/local/lib/starfish/starfish-sync" "$STAGE/starfish-sync"
docker rm -f "$container" > /dev/null
cp deploy/starfish-sync.sudoers deploy/starfish-sudoers deploy/starfish-agent.service \
    scripts/install-agent.sh "$STAGE/"

# --- the VM ----------------------------------------------------------------
if ! limactl list -q 2>/dev/null | grep -qx "$VM"; then
    info "Creating the $VM VM"
    limactl create --name="$VM" --tty=false lima/starfish-test.yaml
fi

if [ "$(limactl list "$VM" --format '{{.Status}}' 2>/dev/null)" != "Running" ]; then
    info "Starting the $VM VM"
    limactl start "$VM" --tty=false
fi

# In Lima's default user mode network the default gateway is the host.
HOST_IP=$(limactl shell "$VM" ip route | awk '/^default/ { print $3 }')
[ -n "$HOST_IP" ] || fail "unable to work out how the VM reaches the host"
info "The VM reaches this Mac at $HOST_IP"

# --- controller and database, on the Mac -----------------------------------
info "Seeding the database"
psql -h localhost -U postgres -d postgres -q \
    -c "DROP DATABASE IF EXISTS $DB_NAME;" -c "CREATE DATABASE $DB_NAME;" 2>&1 | grep -v NOTICE || true

cargo build --quiet -p starfishd -p starfish_admin
admin() { ./target/debug/starfish-admin -p "$DB_URL" "$@"; }

admin init-db > /dev/null
admin user add ada --first-name Ada --last-name Lovelace \
    --email ada@example.com --ssh-key "laptop:ssh-ed25519 AAAAC3-ada-laptop" > /dev/null
admin host-group add --name web > /dev/null
admin host-group add-user --name web --alias ada --sudoer \
    --security-group developers > /dev/null

# The VM knows itself by its Lima hostname, but the controller identifies hosts
# by agent key, so the name here only has to be consistent.
VM_HOSTNAME=$(limactl shell "$VM" hostname)
admin host add --hostname "$VM_HOSTNAME" --host-group web > /dev/null
AGENT_KEY=$(admin host show --hostname "$VM_HOSTNAME" | awk '/agent key/ { print $3 }')

info "Starting the controller on 0.0.0.0:$LISTEN_PORT"
./target/debug/starfishd --config /nonexistent --sql-server "$DB_URL" \
    --listen "0.0.0.0:$LISTEN_PORT" --admin-socket /tmp/starfish-systemd.sock \
    --log-level info > "$STAGE/controller.log" 2>&1 &
CONTROLLER_PID=$!
sleep 4

kill -0 "$CONTROLLER_PID" 2>/dev/null || { cat "$STAGE/controller.log"; fail "the controller did not start"; }

# --- install and start the unit --------------------------------------------
info "Installing the agent in the VM with scripts/install-agent.sh"
limactl shell "$VM" sudo rm -rf /tmp/starfish
limactl shell "$VM" mkdir -p /tmp/starfish

for file in starfish-agent starfish-sync starfish-sync.sudoers starfish-sudoers \
    starfish-agent.service install-agent.sh; do
    limactl copy "$STAGE/$file" "$VM:/tmp/starfish/$file"
done

# The real installer, not a copy of it: this is the only place the shipped
# install-agent.sh is exercised, so a break in it fails the suite.
limactl shell "$VM" sudo bash /tmp/starfish/install-agent.sh \
    --non-interactive \
    --controller-url "ws://$HOST_IP:$LISTEN_PORT" \
    --agent-key "$AGENT_KEY" \
    --heartbeat-secs 5 \
    --log-level debug

info "Waiting for the agent to configure the host"
for _ in $(seq 1 30); do
    limactl shell "$VM" getent passwd ada > /dev/null 2>&1 && break
    sleep 1
done

# --- assertions -------------------------------------------------------------
info "Checking the unit"

check() {
    local what=$1 expected=$2 actual=$3
    if [ "$actual" = "$expected" ]; then
        printf '  ok    %-42s %s\n' "$what" "$actual"
    else
        printf '  FAIL  %-42s %s (expected %s)\n' "$what" "$actual" "$expected"
        printf '\n--- controller log ---\n'
        tail -30 "$STAGE/controller.log" || true
        printf '\n--- agent journal ---\n'
        limactl shell "$VM" sudo journalctl -u starfish-agent --no-pager -n 30 || true
        fail "$what"
    fi
}

check "service is active" "active" \
    "$(limactl shell "$VM" systemctl is-active starfish-agent || true)"
check "runs as the service account" "starfish" \
    "$(limactl shell "$VM" systemctl show -p User --value starfish-agent)"

# The sandbox is what a container cannot test.  If these were silently ignored
# the unit would still start and the hardening would be imaginary.
check "ProtectSystem is applied" "yes" \
    "$(limactl shell "$VM" systemctl show -p ProtectSystem --value starfish-agent)"
check "PrivateTmp is applied" "yes" \
    "$(limactl shell "$VM" systemctl show -p PrivateTmp --value starfish-agent)"

# The three below assert the *absence* of hardening, on purpose.  The helper is
# started by sudo as a child of this unit and inherits its namespace, so each of
# these would silently stop account management from working:
#
#   ProtectHome=yes        hides /home, so authorized_keys cannot be written
#   ProtectSystem=strict   makes /etc read-only, so useradd cannot lock passwd
#   NoNewPrivileges=yes    stops sudo raising privileges at all
#
# Each looks like an improvement and breaks the agent, so they are pinned here.
check "ProtectHome is off, so keys can be written" "no" \
    "$(limactl shell "$VM" systemctl show -p ProtectHome --value starfish-agent)"
check "NoNewPrivileges is off, so sudo works" "no" \
    "$(limactl shell "$VM" systemctl show -p NoNewPrivileges --value starfish-agent)"

# StartLimitIntervalSec belongs in [Unit]; under [Service] systemd ignores it
# with a warning and the agent gets rate limited out of restarting.
check "restart rate limiting is disabled" "0" \
    "$(limactl shell "$VM" systemctl show -p StartLimitIntervalUSec --value starfish-agent)"

check "the agent process is unprivileged" "starfish" \
    "$(limactl shell "$VM" ps -o user= -p "$(limactl shell "$VM" systemctl show -p MainPID --value starfish-agent)" | tr -d ' ')"

info "Checking the host was actually configured"

check "the user exists" "Ada Lovelace" \
    "$(limactl shell "$VM" getent passwd ada | cut -d: -f5)"
check "the security group was created" "0" \
    "$(limactl shell "$VM" sh -c 'id -nG ada | grep -qw developers; echo $?')"
# `grep -w sudo` is not good enough here: a hyphen is not a word character, so
# it matches inside "starfish-sudo" too.  Exact whole-line matching keeps the
# two groups distinguishable.
check "sudo was granted" "0" \
    "$(limactl shell "$VM" sh -c "id -nG ada | tr ' ' '\n' | grep -qx starfish-sudo; echo \$?")"

# Membership is not the point; being able to run something is.  Managed
# accounts have no password, so without the NOPASSWD rule in
# deploy/starfish-sudoers this is a group ada would sit in uselessly.
check "sudo actually works for the granted user" "0" \
    "$(limactl shell "$VM" sudo -u ada sudo -n /bin/true > /dev/null 2>&1; echo $?)"

# Ubuntu's own sudo group is left alone, so a local administrator already in it
# is unaffected by anything Starfish does.
check "Ubuntu's sudo group is untouched" "1" \
    "$(limactl shell "$VM" sh -c "id -nG ada | tr ' ' '\n' | grep -qx sudo; echo \$?")"
# Read as root: .ssh is 700 and owned by ada, so being unable to look inside it
# as anyone else is the mode doing its job.
check "the key directory is owned and moded correctly" "ada:ada 700" \
    "$(limactl shell "$VM" sudo stat -c '%U:%G %a' /home/ada/.ssh)"
check "the key file is owned and moded correctly" "ada:ada 600" \
    "$(limactl shell "$VM" sudo stat -c '%U:%G %a' /home/ada/.ssh/authorized_keys)"
check "the key was installed" "0" \
    "$(limactl shell "$VM" sudo sh -c 'grep -q ada-laptop /home/ada/.ssh/authorized_keys; echo $?')"

info "Checking the controller heard about it"
check "the host reported success" "0" \
    "$(grep -qE 'applied generation' "$STAGE/controller.log" && echo 0 || echo 1)"

printf '\n\033[32mAll systemd checks passed.\033[0m\n'
printf 'The VM is left running for the next run; remove it with: limactl delete -f %s\n' "$VM"
