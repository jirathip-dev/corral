# Corral on Linux (Bazzite): x86_64 release + systemd --user install

Corral is an iOS-only product, but its daemon (`corrald`) is not: per the
multi-server model (locked spec #394, A1), every machine runs its own
`corrald` beside its own local Herdr server. This page covers the Linux
x86_64 daemon distribution and the rootless, per-user install path used on
Bazzite (immutable OSTree OS — no RPM layering, no container). The iOS app
and macOS/launchd path are unchanged; see docs/QUICKSTART.md and
docs/OPERATIONS.md for device registration/grants and the macOS daemon.

Release assets (built and tested on Linux CI, checksummed):

- `corral-<tag>-linux-x86_64.tar.gz` — Linux x86_64 daemon bundle
- `corral-<tag>-linux-x86_64.tar.gz.sha256` — its SHA-256
- `corral-<tag>-macos.tar.gz`(+`.sha256`) — macOS daemon bundle (unchanged)

Bundles are per-platform and never relabeled; the installer refuses to
install a non-ELF staged binary.

## Prerequisites (no Rust toolchain, no root)

- An x86_64 Linux with a **systemd user manager**: a normal desktop login
  session provides one. Over SSH/headless, enable it once with
  `loginctl enable-linger "$USER"` (per-user, reversible) so the service
  starts at boot without a login session.
- Herdr running locally (this PC), serving its unix socket at the default
  `~/.config/herdr/herdr.sock`. `corrald` tolerates the socket being
  temporarily absent (bounded reconnect backoff) but shows no agents until
  herdr answers.
- `curl` and `tar` (present on Bazzite by default). `gh` is only needed for
  the automatic "latest release" resolution; you can install with an
  explicit `--url` instead.
- The release installer is a plain user-level script: it writes only under
  `$HOME` and refuses to run as root.

## Install

From a terminal in your desktop session (per-user systemd service):

```sh
# gh available:
bash <(curl -fsSL https://raw.githubusercontent.com/jirathip-dev/corral/main/scripts/install-corral.sh)

# or pin a release explicitly (works without gh):
RELEASE_URL=https://github.com/jirathip-dev/corral/releases/download/v0.1.0/corral-v0.1.0-linux-x86_64.tar.gz \
  bash <(curl -fsSL https://raw.githubusercontent.com/jirathip-dev/corral/main/scripts/install-corral.sh)
```

What the installer does (in order):

1. Downloads the Linux x86_64 bundle and its `.sha256`, verifies the
   checksum, and refuses anything mismatched **before** creating install
   state (no half-install).
2. Stages and validates the bundle (per-platform required files, ELF
   identity), then swaps it into `~/.local/share/corral/release/`
   (`release.previous` holds the old version for rollback).
3. Writes the hardened `systemd --user` unit
   `~/.config/systemd/user/corrald.service` (only when it changed), runs
   `systemctl --user daemon-reload`, then enables and starts
   `corrald.service` (fresh install), starts it (enabled but stopped), or —
   when the installed binary changed — restarts it. An unchanged binary
   (same-version reinstall) leaves a healthy running service untouched.
4. Health-checks `http://127.0.0.1:8474/healthz`; on failure it stops the
   service and the installer rolls the release directory back, exiting
   non-zero.

Resulting layout:

| Path | Purpose |
| --- | --- |
| `~/.local/share/corral/release/corrald` | the daemon binary (stable path across updates) |
| `~/.config/systemd/user/corrald.service` | the unit file |
| `~/.config/corral/` | host key, device registry, history — **never touched** by install/update/uninstall |

Verify:

```sh
systemctl --user status corrald.service          # active (running)
curl -s http://127.0.0.1:8474/healthz            # → ok
journalctl --user -u corrald.service -e          # daemon logs (journal)
```

## Service behavior (G3)

- **Loopback only.** The unit runs `corrald --socket ~/.config/herdr/herdr.sock
  --bind 127.0.0.1 --port 8474`. The daemon refuses public/unspecified binds;
  remote iOS access goes through a private Tailscale HTTPS Serve mapping
  below — never `--bind` a tailnet/private IP on Linux.
- **Bounded restart.** `Restart=on-failure` with `RestartSec=2`, capped by
  the unit's start limit (6 starts per 90 s). A crash loop fails the unit
  and stays down instead of churning: recover with
  `systemctl --user reset-failed corrald.service && systemctl --user start corrald.service`
  (or re-run the installer after fixing the cause). Herdr-socket
  reconnects use the daemon's own capped exponential backoff.
- **Config preservation.** Updates swap only the release directory; keys and
  the device registry in `~/.config/corral` (0600 files under a 0700 dir)
  survive install, update, and uninstall.
- **Hardening.** The unit sets `NoNewPrivileges`, `PrivateTmp`,
  `RestrictSUIDSGID`, `ProtectClock`, `UMask=0077`, and a deterministic
  PATH. `ProtectHome`/`ProtectSystem` are deliberately not used: the daemon
  must write `~/.config/corral` and read the herdr socket.

## Update

Re-run the installer (latest by default, or `--release <tag>`). It is
checksum-verified and idempotent: an unchanged binary means no restart, a
changed binary means one `systemctl --user restart`. There is no
source-checkout auto-updater on Linux (scripts/update-corral.sh is the
macOS launchd source-mode updater; release installs on every platform are
updated with install-corral.sh).

## Uninstall (config kept)

```sh
bash <(curl -fsSL https://raw.githubusercontent.com/jirathip-dev/corral/main/scripts/install-corral.sh) --uninstall
# or, from a checkout:
bash scripts/install-corral.sh --uninstall
```

Stops and disables `corrald.service`, removes the unit file and
`~/.local/share/corral` release files. **`~/.config/corral` is preserved**
(delete it manually to wipe keys/registry).

## Remote access from iOS (private Tailscale HTTPS Serve)

The daemon never leaves loopback. Reach it from the iPhone over the private
tailnet with real TLS via Tailscale Serve, mirroring the macOS setup in
docs/OPERATIONS.md ("Remote access from iOS"):

1. One-time, in the Tailscale admin console: **DNS → enable HTTPS
   Certificates** (tailnet-wide). Verify:
   `tailscale status --json | grep -i certdomains`.
2. Install/sign in to the Tailscale app on the iPhone with the SAME tailnet
   (MagicDNS on).
3. On the PC (the tailscale CLI ships with the Bazzite Tailscale client):

   ```sh
   tailscale serve --bg --https=443 http://127.0.0.1:8474
   ```

4. Verify a valid chain from the PC, then from the phone:

   ```sh
   curl -s -o /dev/null -w '%{http_code} verify=%{ssl_verify_result}\n' \
     https://<host>.<tailnet>.ts.net/healthz      # → 200 verify=0
   ```

In the app, the host URL is the plain HTTPS origin without a port:
`https://<host>.<tailnet>.ts.net`. Tear down later with
`tailscale serve status` / `tailscale serve reset`. Every device on the
tailnet can read fleet state (`/snapshot`, `/events`, `/history` are
credential-free), so use this only on a tailnet whose every device may see
it — the same rule as on macOS.

## Guardrails and status

- The installer refuses: root (`systemd --user` is per-user), an install
  root outside `$HOME` (no system-wide paths on immutable OSes), non-x86_64
  Linux hosts, and non-Linux bundle binaries.
- Automated proof (CI): the hermetic installer suite
  (`scripts/test-install-corral-linux.sh`, fake `$HOME` + stubbed
  systemctl/curl/uname) covers fresh install, idempotent reinstall/update,
  checksum failure, unhealthy service rollback, uninstall-with-config-
  preserved, and no-root/home-path refusals; `.github/workflows/linux.yml`
  runs it plus a real loopback daemon smoke on every push/PR, and the
  release workflow builds, tests, and checksums the Linux bundle on a Linux
  runner. `systemd-analyze verify` validates the generated unit on the
  ubuntu runner.
- Human-gated acceptance: end-to-end acceptance **on the actual Bazzite PC**
  (real install, health, restart, update, Tailscale HTTPS reachability,
  config preservation) is a separate human-gated step tracked on issue #402 —
  this page plus the scripts above are the runbook for that acceptance.
