# Connectivity: bounded checks and safe Tailscale Serve setup

This document covers the machine-side companion to the supported iOS path:
`corrald` stays on loopback and is fronted by **real TLS via private
Tailscale Serve** (see "Remote access from iOS (Tailscale Serve)" in
[OPERATIONS.md](OPERATIONS.md)). Everything here is implemented in
`scripts/corral-status.sh` (mode dispatch), `scripts/lib-corral-connectivity.sh`
(probes + apply) and exercised by `scripts/test-corral-connectivity.sh`.
No daemon was added; no installer or CI was changed.

## The bounded check (read-only)

```sh
bash scripts/corral-status.sh --connectivity
```

One run distinguishes the five failure classes with an explicit result and a
concrete next action each:

| class | what is checked | example result |
|---|---|---|
| `herdr` | Herdr socket exists, is a unix socket, and is readable/writable by this user | `FAIL no unix socket at ~/.config/herdr/herdr.sock` |
| `corrald` | loopback `GET /healthz` answers `ok` (per-probe timeout) | `FAIL connection refused at http://127.0.0.1:8474/healthz (corrald is not running)` |
| `tailscale` | `tailscale status --json`: backend state, node DNS name | `FAIL backend state "NeedsLogin" - this host is not signed in` |
| `serve` | `tailscale serve status --json`: an HTTPS mapping for `<node>:<port>` -> the loopback origin, no Funnel | `FAIL no https:// mapping for <node>:443 -> http://127.0.0.1:8474` |
| `tls` | `https://<node>[:port]/healthz` with a **verified** certificate chain (no `-k`) | `FAIL TLS verification failed for https://<node>/healthz (curl exit 60)` |

Notes:

- The default check is **read-only**: it only ever runs
  `tailscale status --json` and `tailscale serve status --json`. No mutating
  Tailscale command, no `launchctl`/`systemctl`, no Serve change.
- Every probe is bounded by `CORRAL_STATUS_TIMEOUT_SECONDS` (default 5 s).
- A failing class makes the later classes that depend on it `SKIP`, and the
  exit code is the **first failing class in probe order**
  (`herdr` -> `corrald` -> `tailscale` -> `serve` -> `tls`):

| exit | meaning |
|---|---|
| 0 | all host-local checks passed |
| 1 | Herdr missing / not a socket / not accessible |
| 2 | usage, consent, or configuration error (nothing was attempted) |
| 3 | corrald stopped, hung, or unhealthy |
| 4 | Tailscale CLI missing, not running, signed out, or unrecognizable |
| 5 | Serve mapping missing, conflicting, Funnel-enabled, or unrecognizable (fail closed) |
| 6 | TLS/HTTP failure at the HTTPS origin |

- **Host-local vs phone reachability are reported separately.** A host-local
  pass proves the daemon, the Serve mapping, and the certificate chain on
  *this* host. It cannot prove the iPhone is signed in, online, or able to
  reach the tailnet, so every run ends with
  `phone-reachability: UNKNOWN`. Confirm from the device itself.

## Supported topology (and what is never done)

- `corrald` stays **loopback-only** (plain `scripts/setup-corrald.sh` /
  `scripts/setup-corrald-linux.sh`, no `--bind`).
- The only supported remote entry point is **private HTTPS via Tailscale
  Serve** on a tailnet whose every device may see fleet state (the read plane
  is credential-free to the tailnet, exactly like a tailnet bind).
- Never: Funnel (public exposure), raw/public/tailnet binds, TLS
  verification bypass (`-k`), silent ACL or account changes. The tool has no
  code path for any of these and refuses to act on a Funnel-enabled mapping.

## Prerequisites (sign-in and admin steps)

1. **Tailscale installed and signed in on the host** - the check reports
   `NeedsLogin`/`Stopped` until `tailscale up` has been run and the node is
   approved. This tool never signs in for you.
2. **MagicDNS enabled** for the tailnet (admin console -> DNS). The node's
   `<host>.<tailnet>.ts.net` name comes from this.
3. **HTTPS Certificates enabled** (admin console -> DNS -> HTTPS
   Certificates). One-time, admin-console-only - there is no CLI equivalent.
   Until it is enabled, `tailscale cert` fails and `tailscale serve` hangs;
   the check reports `serve: HTTPS certificates are not enabled for this
   tailnet` instead of guessing.
4. **The iPhone** must be on the same tailnet, signed in, with MagicDNS on.

These prerequisites are documented, not automated: there is no promise of
fully unattended setup, and physical-device acceptance stays open until it is
separately authorized and evidenced on a real phone.

## Adding the missing mapping (apply)

```sh
bash scripts/corral-status.sh --connectivity --apply --yes
```

- **Explicit consent is required.** Without `--yes` the command prints the
  exact Tailscale invocation it would run, contacts no service, changes
  nothing, and exits 2. `--yes` is the consent for `--apply` only.
- **Apply refuses to start unless** the loopback corrald is healthy, Tailscale
  is running and signed in, and HTTPS Certificates are enabled.
- **State is inspected immediately before the action** - twice: once to
  decide, once again right before invoking Tailscale. If a mapping appeared,
  changed, or became unreadable in between, the apply **fails closed** and
  changes nothing.
- **Unrelated mappings are never overwritten.** The apply only ever adds this
  node's `https://<node>:<port> -> <loopback origin>` mapping. If that
  hostport already maps elsewhere, the port is used by a non-HTTPS handler,
  Funnel is enabled, or the Serve config cannot be interpreted, it aborts
  with exit 5 and leaves everything alone. It never runs `serve reset`.
- The only mutating command it can run is
  `tailscale serve --bg --https=<port> <loopback origin>` (default port 443,
  default origin `http://127.0.0.1:8474`), with values validated before use:
  the origin must be a loopback `http://` URL and the port must be 1-65535.
  No `eval`, no `sh -c`, no string-built commands - arguments are passed as
  an argv array, so neither config nor origin can inject shell syntax.
- After the command it re-inspects and only reports success when the mapping
  verifies; a mismatch exits 5 without retrying.

## Environment reference

| variable | default | used for |
|---|---|---|
| `CORRALD_URL` | `http://127.0.0.1:8474` | loopback daemon origin (must be `127.0.0.1`/`localhost`/`[::1]` with an explicit port) |
| `HERDR_SOCKET` | `~/.config/herdr/herdr.sock` | Herdr API socket to check |
| `CORRAL_STATUS_TIMEOUT_SECONDS` | `5` | per-probe timeout (HTTP + Tailscale CLI) |
| `CORRAL_CONNECTIVITY_HTTPS_PORT` | `443` | Serve HTTPS port + TLS probe port |
| `CORRAL_CONNECTIVITY_APPLY_TIMEOUT_SECONDS` | `60` | bounded `tailscale serve` apply |
| `CORRAL_TAILSCALE_BIN` | `tailscale` | Tailscale CLI, resolved from PATH only |
| `CORRAL_CURL_BIN` | `curl` | curl used for the HTTP/TLS probes |
| `CORRAL_PYTHON_BIN` | `python3` | interpreter for the bounded JSON probes |

`tailscale` is resolved from `PATH` only. There is deliberately **no fallback
to a well-known install path** (e.g. the Mac App Store bundle): a hermetic
test `PATH` fully isolates the probes from a live `tailscaled`. If the CLI is
installed somewhere unusual, point `CORRAL_TAILSCALE_BIN` at it; the check
prints that next action when the CLI is missing.

## Redaction

- Subprocess stdout/stderr is discarded; only allowlisted, charset-checked
  tokens (backend state, node DNS name, sanitized proxy URL) are echoed.
- Credentials and pairing material never reach the report: unknown or
  malformed values are reported as `unrecognized`, and a defensive redaction
  pass masks `tskey-*` / login-link shapes even if a future message tries to
  echo them.

## Tests

```sh
bash scripts/test-corral-connectivity.sh              # all scenarios
bash scripts/test-corral-connectivity.sh --only healthy
```

The suite is hermetic: the real script runs with a fixture `PATH`
(stub `curl` + stub `tailscale`), a sandbox `HOME`, fixture state files and
**no network**. It asserts that

- the read-only check only ever records `status --json` /
  `serve status --json` invocations and never calls a mutator,
- no recorded `curl` invocation carries `-k`/`--insecure`,
- the host's real Tailscale CLI is not reachable from the fixture `PATH`,
- missing tools, timeouts, malformed status, HTTP/TLS failures, a healthy
  path, Serve conflicts/Funnel/unrecognized schemas, and the apply
  no-consent / replay / concurrent-conflict cases all behave as documented
  (34 scenarios: healthy path, per-class failures, redaction, default-mode
  compatibility, consent and conflict handling).

Fixtures, per-scenario stdout/stderr and raw exit codes are preserved under
`.logs-484/<run-id>/` (`CORRAL_CONNECTIVITY_LOG_DIR` overrides the root).
The suite never deletes anything - there is no cleanup trap.

## Scope and non-claims

- This is a client-side/scripts-only surface: no iOS or daemon change, no new
  daemon, no CI change, no change to the #402 installers.
- Simulator, string, and fixture tests cannot substitute for physical-device
  acceptance; the iPhone check is a separate, explicitly authorized step.
