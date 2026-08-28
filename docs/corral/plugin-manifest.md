# Corral sidecar plugins

Corrald has one deliberately closed sidecar integration: `fleet-ops`. It is
loaded from `~/.config/corral/plugins/fleet-ops/plugin.toml` (the directory
may be a symlink). Other directories are ignored and are never executed.

Manifest schema (TOML v1):

```toml
id = "fleet-ops"
name = "Fleet Ops"
version = "1.0.0"
platforms = ["macos"]
plugin_schema = "1"

[[cards]]
id = "registry"
title = "Registry"
command = ["herdr-fleet", "list", "--json"]
interval_sec = 60
json = true

[[actions]]
id = "refresh"
title = "Refresh"
command = ["herdr-fleet", "refresh"]
confirm_message = "Refresh fleet operations?"
```

`command` is always an argv array. Corrald never accepts a command string from
the client and never invokes a shell. Card commands run on pane refresh with a
30-second timeout and 256 KiB stdout limit. `json = true` parses stdout as
JSON; other cards display stdout as text. Failures become error cards.

Actions are selected by manifest action id. The UI must show the manifest's
exact argv in the confirmation modal. Cancel does not start a process or write
the audit log. Confirmed actions append to `~/.hermes/logs/corral-plugin-audit.log`.

## Install the fleet-ops sidecar

```sh
mkdir -p "$HOME/.config/corral/plugins"
ln -sfn "$HOME/Projects/fleet-operations/plugin" \
  "$HOME/.config/corral/plugins/fleet-ops"
```

The engine is configless with respect to fleet identity: it does not read
`fleets.json`. The sidecar owns its own configuration and identity validation.
