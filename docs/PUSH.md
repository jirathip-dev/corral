# Corral Push Notifications

How state-change notifications actually reach the iPhone: the two-layer
model (reachability vs delivery), the personal single-user provisioning
decision, and the one-time APNs setup on the Mac that hosts `corrald`.

## Decision record (2026-09-04)

- iOS notifications have **two layers**:
  - **Tailscale = reachability** — the phone finds the Mac and talks to
    `corrald` over HTTP + SSE while the app is open.
  - **APNs = delivery** — background wake + banners when the app is closed
    or backgrounded.
- No network path (Tailscale/LAN) can deliver notifications to a closed
  app; only APNs or locally-scheduled notifications can.
- **Personal single-user**: the daemon on the Mac is the push sender. It
  needs **ONE APNs Auth Key (`.p8`)** on the host, referenced by
  `CORRAL_APNS_TEAM_ID` / `CORRAL_APNS_KEY_ID` / `CORRAL_APNS_AUTH_KEY_PATH`.
  One-time setup.
- iOS-side prerequisite — #389 (the `aps-environment` entitlement) — is
  landed at this writing: TestFlight/Release builds carry the entitlement.
  DEBUG builds deliver while the app is active via the local notification
  bridge; real background delivery is what this guide provisions.
- **No cloud relay / no multi-user service until a real user appears.** A
  future multi-user design (tiny publisher-owned relay on Fly/Worker that
  holds the `.p8` while user daemons stay keyless and POST transitions) is
  deliberately NOT built today.

## Environment reference (read by `corrald` at startup)

| Variable | Meaning |
| --- | --- |
| `CORRAL_APNS_TEAM_ID` | 10-char Team ID (Apple developer account → Membership). The JWT `iss` claim. |
| `CORRAL_APNS_KEY_ID` | The push key's 10-char ID (Keys page column, also in the `.p8` filename). The JWT `kid` claim. |
| `CORRAL_APNS_AUTH_KEY_PATH` | Absolute path to the `.p8` file (PKCS#8 PEM, `-----BEGIN PRIVATE KEY-----`). |
| `CORRAL_APNS_ENDPOINT` | `production` (default) or `sandbox`. Development-provisioned tokens need `sandbox`; TestFlight/App Store builds get production tokens. A mismatch is silently rejected by Apple (400 `BadDeviceToken`). |
| `CORRAL_APNS_TOPIC` | App bundle ID; defaults to `com.corral.fleetnotifier`. |

When unconfigured or misconfigured, the daemon runs exactly as before with
push disabled and logs one of these at startup:

- `push notifier not configured (set CORRAL_APNS_* to enable APNs)` — vars absent
- `push notifier disabled: bad CORRAL_APNS_AUTH_KEY_PATH` — `.p8` unreadable/invalid
- `push notifier disabled: apns provider init failed` — other init failure

## One-time `.p8` acquisition

1. Go to https://developer.apple.com/account/resources/authkeys → **+**.
2. Name: e.g. `Corral APNs`; check **ONLY** *Apple Push Notifications
   service (APNs)*.
3. Register → **DOWNLOAD the `.p8`** (one-time — it cannot be re-downloaded;
   lost key = revoke and create a new one).
4. Save it on the Mac:

   ```sh
   mkdir -p ~/.config/corral
   cp ~/Downloads/AuthKey_<KEY_ID>.p8 ~/.config/corral/apns-auth-key.p8
   chmod 600 ~/.config/corral/apns-auth-key.p8
   ```

   The `.p8` is a private key: keep it at `chmod 600`, never commit it to a
   repo, never paste it into chat/issue/PR. Every example below uses
   placeholders (`<KEY_ID>`, `<TEAM_ID>`), never real values.
5. Note the **Key ID** — the portal column, also embedded in the filename
   (`APNS<KEY_ID>.p8`, 10 chars).
6. Note the **Team ID** — Apple account Membership page (or the Team ID
   selected under Xcode Signing & Capabilities).

## Host wiring (launchd agent)

The daemon is installed as the `com.corral.corrald` launchd agent (see
`scripts/setup-corrald.sh`, plist at
`~/Library/LaunchAgents/com.corral.corrald.plist`). Do NOT re-run
`setup-corrald.sh` after hand-editing the plist — it regenerates the plist
from its template and would drop your `EnvironmentVariables`. Edit the
plist directly instead:

1. Add an `EnvironmentVariables` entry to
   `~/Library/LaunchAgents/com.corral.corrald.plist` (the script-generated
   plist already has one holding `PATH` — add the three keys next to it):

   ```xml
   <key>EnvironmentVariables</key>
   <dict>
     <key>PATH</key>
     <string>...</string>
     <key>CORRAL_APNS_TEAM_ID</key>
     <string><TEAM_ID></string>
     <key>CORRAL_APNS_KEY_ID</key>
     <string><KEY_ID></string>
     <key>CORRAL_APNS_AUTH_KEY_PATH</key>
     <string>/Users/<YOU>/.config/corral/apns-auth-key.p8</string>
   </dict>
   ```

2. Validate, then reload the agent. Plain `launchctl kickstart` does NOT
   make launchd re-read a rewritten plist — boot out and bootstrap:

   ```sh
   plutil -lint ~/Library/LaunchAgents/com.corral.corrald.plist
   launchctl bootout gui/$(id -u)/com.corral.corrald
   launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/com.corral.corrald.plist
   ```

   (Run from a normal shell; launchd management is blocked inside the
   Hermes gateway session.)

## Verification

1. Daemon log (`~/.config/corral/corrald-launchd.log`, the plist's
   `StandardOutPath`/`StandardErrorPath`): the startup line
   `push notifier armed (APNs)` with the endpoint + topic must appear, and
   none of the `push notifier not configured` / `push notifier disabled`
   warnings may. Check:

   ```sh
   grep -E "push notifier (armed|not configured|disabled)" ~/.config/corral/corrald-launchd.log
   ```

2. End-to-end: trigger a real state change (an agent transitions
   start / blocked / episode-end) **while the app is backgrounded** → a
   banner arrives via APNs. When the app is active, delivery falls back to
   local notifications.
