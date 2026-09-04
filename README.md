# Corral

[![License](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue)](LICENSE-MIT)

**See every agent. Know what needs you.**

Corral is a read-only fleet board for [herdr](https://github.com/herdrdev/herdr) agents, on your iPhone. It shows everything your fleet is doing — every agent's state, repo, branch, and time-in-state, grouped by repo, with blocked agents pinned to the top — plus bounded recent output per agent and optional state-change notifications. Corral only watches: there are no drive controls (no prompt / approve / interrupt / kill), by design.

**iOS-only product.** Corral is the `FleetNotifier` iOS app plus `corrald`, a small Rust daemon that runs on the same machine as herdr and collapses the fleet into a live snapshot served over HTTP + SSE. `corrald` reads the herdr unix socket (`~/.config/herdr/herdr.sock`) and binds loopback by default (`127.0.0.1:8474`); public addresses are refused.

```
herdr socket ─→ corrald (daemon) ─→ iOS app: board · recents · notifications
```

## Setup

### 1. Daemon (macOS host)

From a checkout, build and run:

```sh
cargo build --release -p corrald
./target/release/corrald --socket ~/.config/herdr/herdr.sock
```

Verify it is up:

```sh
curl -s http://127.0.0.1:8474/healthz    # → ok
```

To keep it running at login, install it under launchd — `scripts/setup-corrald.sh` builds, installs the `com.corral.corrald` agent (KeepAlive, port 8474), and is idempotent:

```sh
bash scripts/setup-corrald.sh
```

Prebuilt daemon-only releases (no Rust toolchain) install with `scripts/install-corral.sh`.

### 2. Connect iOS (TestFlight build)

1. Install the TestFlight build of FleetNotifier.
2. Open the app → **Settings** (gear, top right) → **Host**: your Tailscale hostname (`https://<host>.<tailnet>.ts.net`) or `127.0.0.1:8474` for a same-LAN/dev setup.
3. **Register / pair this device** with the daemon's registration token (in the daemon config dir or out-of-band). A fresh device is read-only: zero grants.
4. **Enable Notifications** for state-change pushes (start / blocked / done). Simulator and DEBUG builds use the local notification bridge; real background APNs delivery awaits the host-side provisioning checkpoint (an APNs `.p8` auth key + `CORRAL_APNS_*` env).
5. The host provisions the signed read grant (`read_tail` for recents) **out-of-band** — there is no grant UI or admin surface in the app or over HTTP.

Remote access note: the app talks plain HTTP only on loopback — point it at a Tailscale-hosted `https://` origin (Tailscale Serve fronting the loopback daemon), never at a tailnet IP bind. Details in [docs/OPERATIONS.md](docs/OPERATIONS.md).

## Demo

The iOS showcase is a Simulator demo from the TestFlight source revision:

[![iOS board](https://jirathip-dev.github.io/corral/ios/board.png)](https://jirathip-dev.github.io/corral/ios/)

A static board snapshot (phone + wide layouts, fully fictional sample data) is kept in [docs/demo/](docs/demo/).

## Docs

- [docs/QUICKSTART.md](docs/QUICKSTART.md) — run the daemon, register a device, grant `read_tail`, read the fleet
- [docs/OPERATIONS.md](docs/OPERATIONS.md) — device lifecycle, out-of-band grants, remote access from iOS (Tailscale Serve), troubleshooting
- [docs/PUSH.md](docs/PUSH.md) — push architecture (Tailscale reachability vs APNs delivery), `.p8` provisioning + launchd wiring
- [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) — the read model, signed read plane, security boundaries
- [docs/DEVELOPING.md](docs/DEVELOPING.md) — workspace layout, quality gates

## License

Dual-licensed under [MIT](LICENSE-MIT) OR [Apache-2.0](LICENSE-APACHE).
