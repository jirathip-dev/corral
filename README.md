# Corral

[![License](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue)](LICENSE-MIT)

**See every agent. Know what needs you.**

Corral is a small app that runs on the same machine as your [herdr](https://github.com/herdrdev/herdr) fleet and gives you one place to watch it — from a desktop board or your phone.

If you run a fleet of coding agents (Claude Code, Codex, OpenCode, and others), herdr handles the spawning and supervising. Corral sits on top and answers the things you actually want to know:

- **What's happening right now?** One live view of every agent: its raw state, repo, branch, time-in-state, and pane — grouped by repo, no terminal spelunking.
- **What needs you?** Blocked agents are pinned to the top of the board, and every repo group lists its own blocked agents first.
- **What did it just do?** Tap an agent for the bounded recent-output tail, live and auto-scrolled.
- **Did something change?** The iOS app can notify on state changes (start, blocked, done).

Corral is **read-only by design**: it monitors the fleet. There are no drive controls — no prompt, approve, interrupt, kill, attach, or worktree-starting surfaces, in the daemon or in any client.

```
herdr socket ─┐
git watcher ──┤ → corrald (daemon) → ┬─ live fleet snapshot + events (SSE, loopback)
              └                      └─ signed reads: register device · read_tail (recents)
```

## Demo showcase

**[Open the live web demo](https://jirathip-dev.github.io/corral/)** — the read-only board renders bundled fictional sample data by default; connecting to a live daemon is optional. The iOS showcase is a Simulator demo from the TestFlight source revision: [![iOS board](https://jirathip-dev.github.io/corral/ios/board.png)](https://jirathip-dev.github.io/corral/ios/).

## Features

- **One live view of the fleet** — every agent's state, repo/branch, time-in-state, and a small pane reference, grouped by repo with the raw herdr status vocabulary (working / idle / blocked / unknown, plus a wire `done` the board treats as finished — ranked and rendered with `idle`, so finished panes never read as active).
- **Blocked agents surfaced first** — waiting agents are pinned to the top of the board and listed first inside their repo group, with the herdr state chip the fastest visual cue.
- **Recent output** (`read_tail`) — tap any agent for its bounded live tail (≤200 lines, daemon-capped), segmented into blocks, auto-scrolled.
- **Event history + daily digest** — every state change is recorded; query a window over HTTP or take a per-agent daily digest offline.
- **State-change notifications (iOS)** — start, blocked, and done alerts; real APNs delivery awaits the host-side provisioning checkpoint (an APNs `.p8` auth key + `CORRAL_APNS_*` env); simulator/DEBUG builds use the local notification bridge.
- **Signed and private by default** — binds to loopback, denies public addresses; the live snapshot/SSE plane is served only on loopback/private networks, and the per-agent read drives (`read_tail`) are Ed25519-signed. Fresh registrations are read-only (zero grants); the two signed read capabilities are provisioned out-of-band by the host.

## Architecture

The full design is in [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md). In short:

- **`corrald`** — the daemon. Reads the fleet (herdr + a git watcher) into a live snapshot, serves it over HTTP + SSE, and serves signed read requests (`read_tail`).
- **`corrald-ui`** — the desktop board (egui, also compiles to a read-only WASM demo).
- **iOS notifier** — the phone app: board, recents, notifications.
- **Layering** — model → harness → runtime → control plane. Everything is written down in the docs.

## Install

### iOS (not yet distributed)

The iOS client is not on the App Store or TestFlight yet. Distribution requires an Apple Developer account and a signing/verification pass the repository does not claim today (see [ios/README.md](ios/README.md)); simulator builds are the supported path.

### The desktop board + daemon (macOS)

Grab a tagged release — no Rust toolchain needed. The installer downloads a checksummed bundle, verifies its SHA-256, installs the daemon under launchd, and stages the desktop board:

```sh
bash <(curl -fsSL https://raw.githubusercontent.com/jirathip-dev/corral/main/scripts/install-corral.sh)
```

From a checkout, the same script runs directly:

```sh
bash scripts/install-corral.sh
bash scripts/install-corral.sh --release v0.1.0 --bind 127.0.0.1 --port 8474
```

Uninstall removes the launchd agents and the app, keeps your config and keys:

```sh
bash scripts/install-corral.sh --uninstall
```

### Dependencies

- **herdr** — the runtime that spawns and supervises the agents. Install it first. (Corral reads the fleet from herdr's socket; without herdr it still serves the API but shows no agents.)
- **macOS** for the release build and the iOS app. The daemon also builds on Linux.

### Set up the daemon (herdr + launchd)

`corrald` runs as a launchd agent. From a checkout, `scripts/setup-corrald.sh` builds and installs it (idempotent — safe to re-run):

```sh
bash scripts/setup-corrald.sh            # build + run under launchd on 127.0.0.1:8474
```

Once it's up, register a device — registration is read-only by default (zero grants), and the host provisions the signed read grants (`read_tail` for recents) out-of-band on the registry. See the full walkthrough in [docs/QUICKSTART.md](docs/QUICKSTART.md).

## Security posture

Corral is private by default:

- **Loopback by default, public refused** — binds `127.0.0.1`; public IPs and `0.0.0.0` are hard refusals. Over a tailnet/private network, prefer a WireGuard device-authenticated tunnel.
- **Signed reads, default deny** — every signed drive request carries an Ed25519 device signature; a registered device has zero grants until the host provisions capabilities out-of-band (there is no HTTP grant surface and no admin token on any device). The only capabilities that exist are signed reads (`read_tail`, plus the daemon-retained `read_diff` with no client UI).
- **Redacted by default** — secrets are redacted at the adapter boundary before any bytes leave the machine; key material stays `0600` under a `0700` config dir.

## Docs

- [docs/QUICKSTART.md](docs/QUICKSTART.md) — run the daemon, register a device, read the fleet
- [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) — the read model, signed read plane, security boundaries
- [docs/OPERATIONS.md](docs/OPERATIONS.md) — device lifecycle, out-of-band grants, remote access from iOS, keychain, audit log
- [docs/DEVELOPING.md](docs/DEVELOPING.md) — workspace layout, module map, quality gates

## Status

**Pre-1.0, macOS-first.** Daemon and desktop board run on macOS (daemon also Linux); the iOS client builds for the simulator, with no physical-device or TestFlight claim yet. Linux support is partial.

## License

Dual-licensed under [MIT](LICENSE-MIT) OR [Apache-2.0](LICENSE-APACHE).
