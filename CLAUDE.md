# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project

`starfish` synchronizes user accounts from a PostgreSQL database onto Ubuntu hosts. A
Rust 2024-edition Cargo workspace of six crates. `README.md` is the reference for
deployment, configuration tables, and the security rationale — read it before changing
anything to do with privileges, TLS, or database roles.

## Commands

```sh
cargo build                     # build; binaries are starfishd, starfish-admin,
                                # starfish-agent, starfish-sync
cargo clippy
cargo fmt
just build-macos                # release build, aarch64-apple-darwin, natively
just build-linux-arm64          # release build, aarch64-unknown-linux-gnu, in Docker
just build-linux-amd64          # release build, x86_64-unknown-linux-gnu, in Docker
just build-all                  # all three
just test                       # cargo test --workspace; needs nothing external
cargo test -p <crate> <name>    # a single test by substring match
just test-db                    # controller against PostgreSQL (see below)
just test-ubuntu                # privileged helper against ubuntu:24.04 in Docker
just test-systemd               # agent under systemd in a Lima VM
just test-all
just bundle                     # release archives into scratch/publish
just publish                    # upload them to a draft GitHub release
```

The three integration suites skip silently unless their environment variable is set
(`STARFISH_TEST_DATABASE_URL`, `STARFISH_TEST_DOCKER`), which is what the `just` recipes
supply. `just test-db` **drops and recreates** the database it is pointed at.

The `build-` recipes produce release binaries for all three deployment targets under
`target/<triple>/release`. `build-macos` is a plain native build, because cross compiling
to Darwin would need the macOS SDK. The two Linux targets build inside
`docker/Dockerfile.build` on a local Colima VM, which bind mounts the working tree and
keeps the crates.io registry in a named volume. That image picks its cross compiler from
the VM's own architecture, so one Linux target is always native to it; the other needs
both a `gcc-*-linux-gnu` **and** its `libc6-dev-*-cross`, which is only *recommended* by
the compiler and without which `ring` — the one crate here that compiles C — builds
against the wrong `/usr/include`.

`just release` runs `test-all` and then cross compiles both Linux targets, so it needs
Docker, a Lima VM and a local PostgreSQL all working. `just bundle` builds the archives
from whatever is already in `target/<triple>/release` and uploads nothing; `just publish`
sends them to a **draft** GitHub release whose notes are written by hand.

## Architecture

Data flows in one direction: the database is the source of truth, the controller reads it
and pushes to agents, and agents apply to hosts. Nothing flows back except reports.

```
starfish_admin ──writes──> PostgreSQL <──reads── starfishd ──ws/wss──> starfish_agent ──sudo──> starfish-sync
       └────── refresh, over a Unix socket ─────────┘                    (unprivileged)         (root)
```

- **`starfish_db`** — Toasty models (`User`, `SshKey`, `HostGroup`, `Host`,
  `SecurityGroup`, and the `HostGroupUser` / `UserSecurityGroup` join tables) plus the
  connection helpers in `connect.rs` (password files, URL redaction, `sslmode` warnings).
  The schema is *created*, not migrated: `push_schema` runs on first use and a model change
  afterwards needs the tables updated by hand.
- **`starfish_msg`** — the wire protocol, shared by everything else. Encoded as MessagePack
  with `to_vec_named`, so **fields are matched by name and adding one does not break an
  older peer**. WebSockets frame messages themselves; the Unix socket does not, so
  `frame::{read,write}` add a big-endian `u32` length prefix. `PROTOCOL_VERSION` is checked
  against the agent's `Hello`.
- **`starfishd`** — the controller. `controller.rs` holds shared state (db handle, registry,
  generation counter); `agent_registry.rs` maps host id to a connected agent's `mpsc` sender
  and uses `SessionId` so a stale session cannot evict its replacement; `agent_session.rs`
  runs one WebSocket connection; `admin_socket.rs` serves `AdminRequest::Refresh`;
  `server.rs` owns the listeners, optional TLS, and the `CancellationToken` shutdown.
- **`starfish_agent`** — connects, applies whatever `HostConfig` arrives, heartbeats on a
  timer, reconnects with a 1→60s doubling backoff. It never touches the database and never
  changes the host itself.
- **`starfish_sync`** — the privileged helper. Reads a `HostConfig` on stdin, writes a
  `SyncReport` on stdout, takes **no arguments**.
- **`starfish_admin`** — the CLI. Talks to the database directly for everything except
  `refresh`, which talks only to the controller and deliberately does not require a database
  connection.

### Invariants worth knowing before editing

- **The privilege split is the security model.** All policy runs as root inside
  `starfish_sync`; the sudoers rule is one line with no arguments to glob. Never move a
  decision about *what* to change into the agent, and never give the helper command-line
  arguments — that is what makes the rule safe. `validate.rs` re-checks everything
  (name charset, uid ≥ 1000, no colon or newline in a full name, key paths under the home
  `getent` reports) because it cannot assume the controller or database is trustworthy.
- **Syncing is additive except for group membership.** Users and groups are created, never
  deleted. Membership is removed only for groups the controller sent, plus `sudo` — that set
  is built in `sync::sync` and is the only revocation path. `authorized_keys` is owned
  outright and overwritten.
- **Host changes go through `system::System`.** The `Ubuntu` implementation shells out to
  `useradd`, `usermod`, `groupadd`, `gpasswd`, `getent` and `id` (`gpasswd`, never
  `usermod --groups`, which would drop unmanaged groups). The trait exists so `sync.rs` can
  be tested against `FakeSystem` on macOS; keep new host operations behind it.
- **Every user and group is attempted independently** and reported as
  `Created`/`Updated`/`Unchanged`/`Failed`. One bad entry must never abort the rest.
- **The generation counter** is seeded from the wall clock so an agent reconnecting after a
  controller restart never sees a configuration numbered below what it already applied.
- **The controller's database grants are `SELECT` plus `UPDATE` on three `hosts` columns**
  (`deploy/roles.sql`). Any new controller write breaks a least-privilege deployment.
- **The agent's systemd sandbox cannot be tightened.** `ProtectHome=yes`,
  `ProtectSystem=strict` and `NoNewPrivileges=yes` each silently break account management
  because the helper inherits the unit's namespace; `just test-systemd` pins them off.

### Deployment

`scripts/install-agent.sh` and `scripts/install-controller.sh` are the shipped installers,
one per release archive, and both assume their binaries and deployment files sit beside
them. They are convergent: every file is compared before it is written and the service is
restarted only when something it reads changed, so a second run reports nothing changed.
`just test-systemd` runs the *agent* installer rather than a copy of it, which is the only
coverage either script has.

- **The controller's socket is not where its default says.** `/run` is root-only, so
  `starfishd.service` uses `RuntimeDirectory=starfishd` and the config points
  `admin_socket` at `/run/starfishd/starfishd.sock`; `deploy/starfishd.tmpfiles` symlinks
  `/run/starfishd.sock` to it so `starfish-admin` still works undecorated. `/run` is
  emptied on boot, which is why that link is made by tmpfiles and not by the installer.
- **`admin_socket_group` needs the controller in that group.** It hands the socket over
  with `chown`, which the kernel permits only for a group the process belongs to, so the
  installer writes a `SupplementaryGroups=` drop-in. Without it the controller starts,
  fails on the socket, and restarts forever.
- **The controller's unit is hardened where the agent's cannot be** — `ProtectSystem=strict`,
  `NoNewPrivileges=yes` and an empty `CapabilityBoundingSet`. It never changes the host, so
  none of the constraints described above for the agent apply to it.
- **`deploy/roles.sql` is re-runnable and takes psql variables** (`db_name`,
  `owner_password`, `controller_password`); an empty password leaves the role's existing one
  alone. Roles are created inside `DO` blocks that check `pg_roles` first. It still has to
  run *after* the schema exists, because `GRANT ... ON hosts` needs the table.
- **`psql -c` does not interpolate `:variables`** — only file and stdin input does, which is
  why the installer feeds its SQL in on stdin rather than with `-c`.

### Configuration

`starfishd` and `starfish_agent` both merge a TOML file and the command line with figment,
**command line winning**, into `ServerConfig` / `AgentConfig`. Both use `single-instance`,
so only one of each runs at a time — which is why the end-to-end suite is a single test.

## Versioning

`stampver` drives versions from `version.json5` into the three binary crates' `Cargo.toml`;
`just release [incrPatch|incrMinor|incrMajor]` tags and pushes. Do not hand-edit those
version fields.
