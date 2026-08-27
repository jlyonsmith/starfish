#!/usr/bin/env bash
# Installs Starfish on a host exactly as the README describes, then starts the
# agent under systemd.  Run as root inside the test VM by `just test-systemd`;
# the files it needs have already been copied to /tmp/starfish.
set -euo pipefail

STAGE=/tmp/starfish
CONTROLLER_URL=${CONTROLLER_URL:?CONTROLLER_URL is required}
AGENT_KEY=${AGENT_KEY:?AGENT_KEY is required}

# The unprivileged service account.  It owns nothing and cannot log in; its only
# privilege is the single sudo rule below.
if ! getent passwd starfish > /dev/null; then
    adduser --system --group --no-create-home --shell /usr/sbin/nologin starfish
fi

install -o root -g root -m 0755 -D "$STAGE/starfish-sync" /usr/local/lib/starfish/starfish-sync
install -o root -g root -m 0755 "$STAGE/starfish-agent" /usr/local/bin/starfish-agent

install -o root -g root -m 0440 "$STAGE/starfish-sync.sudoers" /etc/sudoers.d/starfish
visudo -c -f /etc/sudoers.d/starfish

# Readable by the service account and nobody else: it holds the agent key.
printf 'controller_url = "%s"\nagent_key = "%s"\nheartbeat_secs = 5\nlog_level = "debug"\n' \
    "$CONTROLLER_URL" "$AGENT_KEY" > /etc/starfish_agent.conf
chown root:starfish /etc/starfish_agent.conf
chmod 0640 /etc/starfish_agent.conf

install -o root -g root -m 0644 "$STAGE/starfish-agent.service" /etc/systemd/system/starfish-agent.service

systemctl daemon-reload
systemctl enable starfish-agent

# Restart rather than `enable --now`: on a re-run the service is already up, and
# `--now` would leave it running with the previous configuration.
systemctl restart starfish-agent
