# epochctl

`epochctl` is the user-facing control CLI for the Epoch desktop. It is the command keybindings and
scripts should call, so nothing outside the shell has to know how the shell is wired.

It talks to two things:

| Target | Transport | Used for |
|--------|-----------|----------|
| **EpochShell** (Quickshell) | `qs ipc` subprocess | launcher, panels, reload |
| **EpochOxide** | Unix socket, newline-delimited JSON | search, activation, providers, menus |

## Why not just call `qs`?

You can. `qs -p ~/.config/epochshell ipc call launcher toggle` works today. `epochctl` exists
because of what that command does when it *doesn't* work:

```console
$ qs -p ~/.config/epochshell ipc call panel toggle audio
Target not found.
$ echo $?
0
```

Quickshell prints IPC-level failures on **stdout** and exits **0**. A keybinding wired straight to
`qs` fails silently, forever, with no signal that anything is wrong.

```console
$ epochctl panel toggle audio
epochctl: the running shell exposes no "panel" IPC target.
It is probably older than this epochctl -- rebuild and restart epochshell.
$ echo $?
5
```

Beyond that, `epochctl`:

- keeps `~/.config/epochshell` out of your compositor config,
- picks a shell instance deterministically when stale registrations have piled up,
- speaks EpochOxide's socket directly, with no subprocess,
- gives both backends one command namespace and one `--json` output shape.

## Commands

### Launcher

```bash
epochctl launcher toggle
epochctl launcher open
epochctl launcher close
epochctl launcher provider files      # jump straight into a provider
epochctl launcher provider keybinds
epochctl launcher status              # open | closed
```

Provider listing lives under [Backend](#backend), not here: providers belong to EpochOxide, and
`search`, `activate`, and `menu` all use them without the launcher being involved.

### Panels

```bash
epochctl panel toggle audio
epochctl panel open wifi
epochctl panel close tailscale
epochctl panel status battery
epochctl panel list
epochctl panel close-all
```

Panel names come from the shell itself, so `panel list` is always authoritative. With the current
shell that is `audio`, `battery`, `bluetooth`, `calendar`, `ethernet`, `homeassistant`, `media`,
`notifications`, `system`, `tailscale`, `weather`, and `wifi`. Asking for one that does not exist
lists the ones that do:

```console
$ epochctl panel toggle localsend
epochctl: unknown panel "localsend"
Known: audio, battery, bluetooth, calendar, ethernet, homeassistant, media, ...
```

### Shell

```bash
epochctl ping                         # alias for `shell ping`
epochctl reload                       # alias for `shell reload`
epochctl reload --hard                # rebuild the QML engine
epochctl shell info
epochctl shell targets
```

`reload` is soft by default. A hard reload tears down and rebuilds the whole QML engine, which
drops the notification daemon and polkit agent registrations with it.

### Backend

```bash
epochctl search firefox
epochctl search README --provider files --limit 5
epochctl providers
epochctl menu keybinds
epochctl activate apps firefox.desktop
```

`search` with no `--provider` asks EpochOxide which providers answer queries and searches all of
them. Run `providers` to see what you can pass to `--provider`.

### Diagnostics

```bash
epochctl doctor
```

```console
ok   quickshell cli    /etc/profiles/per-user/brian/bin/qs
ok   shell config      /home/brian/.config/epochshell
warn shell instances   1 running, 9 stale registrations
ok   ipc targets       panel, shell, launcher
ok   target launcher   epochctl launcher available
ok   target panel      epochctl panel available
ok   target shell      epochctl shell / ping / reload available
ok   epochoxide        7 providers at /run/user/1000/epochoxide.sock
ok   provider sync     shell and backend agree on 7 providers
```

`provider sync` compares the provider list the shell is holding against what the backend actually
reports. They drift when EpochOxide restarts after the shell did, or when the two are pointed at
different sockets; `epochctl reload` resettles it. The check is skipped against a shell too old to
report its providers.

## Global options

| Flag | Meaning |
|------|---------|
| `--config`, `-C` | Quickshell config directory |
| `--socket` | EpochOxide socket |
| `--instance`, `-i` | Target a shell instance by id (a unique substring works) |
| `--pid` | Target a shell instance by process id |
| `--newest`, `-n` | Use the newest instance instead of the oldest |
| `--any-display` | Do not filter instances by launch display |
| `--json` | Emit JSON instead of human-readable text |
| `--dry-run` | Print the `qs` command instead of running it |

`--dry-run` is the quickest way to see what a command actually does:

```console
$ epochctl --dry-run -n panel toggle audio
qs -p /home/brian/.config/epochshell -n ipc call panel toggle audio
```

## Environment

| Variable | Default |
|----------|---------|
| `EPOCHSHELL_CONFIG_DIR` | `$XDG_CONFIG_HOME/epochshell` |
| `EPOCHOXIDE_SOCKET` | `$XDG_RUNTIME_DIR/epochoxide.sock` |
| `EPOCHCTL_QS` | `qs`, falling back to `quickshell` |

Pointing `epochctl` at a checkout instead of the installed shell is just a config directory:

```bash
epochctl -C ~/projects/Epoch/epochshell/quickshell panel list
```

This matters during development. Quickshell groups instances by a hash of the `shell.qml` **path**,
so a shell running from a repo checkout is a different instance group from the installed one.

## Exit codes

| Code | Meaning |
|------|---------|
| `0` | Success |
| `1` | The shell or backend ran the request and reported a failure |
| `2` | Bad arguments |
| `3` | The shell is not running |
| `4` | A dependency is missing or unreachable (`qs`, the config, EpochOxide) |
| `5` | The running shell has no such IPC target or function |

Code `5` specifically means the running shell is older than this `epochctl` — rebuild and restart
EpochShell.

## Shell requirements

`epochctl` needs the shell to expose IPC targets, which EpochShell declares as `IpcHandler` blocks
at its root in `quickshell/Ipc.qml`:

| Target | Provides |
|--------|----------|
| `launcher` | `epochctl launcher` |
| `panel` | `epochctl panel` |
| `shell` | `epochctl shell`, `ping`, `reload` |

Handlers must live at the shell root, never inside `Bar.qml`'s `Variants { model: Quickshell.screens }`
— a handler declared under `Variants` is instantiated once per screen and the copies fight over the
same target name.

Every handler returns a JSON object, because `qs ipc call` exits 0 even when it printed
"Target not found." The payload is the contract, not the exit status.

`epochctl` still works against a shell that predates that contract: functions declared `: void`
return nothing, which is read as success.

## Building

```bash
cargo build --release
cargo test
```

With Nix:

```bash
nix build .#epochctl
nix run .#epochctl -- doctor
```

## Installing

### With EpochShell (recommended)

EpochShell takes `epochctl` as a flake input and installs it by default, pointed at its own config
directory and EpochOxide socket, so the two cannot drift apart:

```nix
programs.epochshell.enable = true;
```

Turn it off, or pin a different build, with:

```nix
programs.epochshell.epochctl.enable = false;
programs.epochshell.epochctl.package = myEpochctl;
```

### Standalone

```nix
{
  inputs.epochctl = {
    url = "github:bjsemrad/epochctl";
    inputs.nixpkgs.follows = "nixpkgs";
  };

  # ...

  imports = [ inputs.epochctl.homeManagerModules.default ];

  programs.epochctl.enable = true;
}
```

The module installs the package and shell completions. `configDir` and `socket` are optional
overrides for non-default locations; leave them unset to use the defaults above.

## Completions

```bash
epochctl completions bash    # or zsh, fish, elvish, powershell
```

The Nix package installs these for bash, zsh, and fish already.
