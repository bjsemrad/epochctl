# epochctl

`epochctl` is the user-facing control CLI for the Epoch desktop. It is the command keybindings and
scripts should call, so nothing outside the shell has to know how the shell is wired.

It talks to two things:

| Target | Transport | Used for |
|--------|-----------|----------|
| **EpochShell** (Quickshell) | `qs ipc` subprocess | launcher, panels, reload |
| **EpochOxide** | Unix socket, newline-delimited JSON | search, activation, providers, menus, capture, nix |

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
shell that is `audio`, `battery`, `bluetooth`, `calendar`, `capture`, `ethernet`, `homeassistant`,
`localsend`, `media`, `nix`, `notifications`, `record`, `system`, `tailscale`, `weather`, and
`wifi`. Asking for one that does not exist
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

### Capture

```bash
epochctl capture screenshot                      # drag out a region
epochctl capture screenshot window               # the focused window
epochctl capture screenshot window --select      # click the window to capture
epochctl capture screenshot fullscreen           # the focused monitor
epochctl capture screenshot fullscreen -o DP-3   # a named monitor
epochctl capture screenshot all                  # every monitor, as one image
epochctl capture status
```

```console
$ epochctl capture screenshot window
mode       window
window     thor: epochoxide
region     6,46 2148x1388
saved      /home/brian/Pictures/Screenshots/screenshot-20260112-144233.png
size       2864×1850, 298 KB
clipboard  copied
```

Every shot is saved to EpochOxide's `screenshot_dir`, copied to the clipboard, and announced to
the shell, which shows the image itself in the notification. Each of those is a flag away:

| Flag | Effect |
|------|--------|
| `--no-copy` | Leave the clipboard alone |
| `--no-save` | Copy the shot and leave the file in the cache |
| `--no-notify` | Take it quietly |
| `--dir <DIR>` | Save this one somewhere else |
| `--cursor` | Include the mouse pointer (screenshots only) |
| `--delay`, `-d` | Wait N seconds after any selection is made |

Cancelling a selection prints `cancelled` and exits **0** — pressing Escape is a decision, not a
failure, and a keybinding should not report one.

```bash
epochctl capture ocr                       # read a region and copy the text
epochctl capture ocr window                # read the focused window
epochctl capture ocr --lang eng+deu
epochctl capture ocr --keep                # keep the image as well as the text
```

`ocr` prints the recognised text on stdout and nothing else, so `epochctl capture ocr > notes.txt`
holds text rather than a status line; whether it was copied is what the notification says. A region
with nothing legible in it prints `no text found` and exits 0. It needs `tesseract`, which
`capture status` reports on. Reading happens in EpochOxide, so tesseract has to be on the daemon's
`PATH` -- the Nix modules put it there -- not necessarily in your shell's.

```bash
epochctl capture record                    # record a region
epochctl capture record fullscreen         # record the focused monitor
epochctl capture record window --select    # click the window to record
epochctl capture record status             # what is running, if anything
epochctl capture record stop               # finish the file
```

```console
$ epochctl capture record status
mode      fullscreen
monitor   eDP-1
file      /home/brian/Videos/Recordings/recording-20260112-144233.mp4
running   0:42

$ epochctl capture record stop
saved     /home/brian/Videos/Recordings/recording-20260112-144233.mp4
length    0:42
size      6.5 MB
```

The daemon owns the recorder, so the recording survives the command that started it, keeps running
across a `stop` from any other terminal or from the bar, and shows up in the shell's indicator
whichever way it was started. Only one runs at a time: starting a second is refused with how long
the first has been going. `stop` with nothing running prints `not recording` and exits 0, so a key
bound to it is safe to press twice. Recording needs `wf-recorder`; `capture status` says so.

`epochctl panel toggle capture` and `epochctl panel toggle record` open the same options as panels
in the bar drawer, for the times a menu is easier than remembering which key does which mode.

`capture status` shows where shots land and which of `grim`, `slurp`, `wl-copy`, and `notify-send`
are actually installed. Window capture also needs a compositor that reports where its windows are;
`status` says whether this one does.

### Nix

```bash
epochctl nix status                 # what the last check found
epochctl nix check                  # resolve every input now
epochctl nix hosts
epochctl nix update                 # opens a terminal running your update command
epochctl nix rebuild thor           # opens a terminal running thor's rebuild command
```

```console
$ epochctl nix check
flake      /home/brian/nixconfig
checked    moments ago
locked     3 hours ago
updates    1 of 23 inputs can move
  nixpkgs              6aefcda -> d6524aa  github:NixOS/nixpkgs/nixos-unstable
```

Checking never writes to your flake -- EpochOxide resolves the inputs into a throwaway lock file
and compares. `status` reads the last result and is cheap; `check` talks to every input's host, so
it has no timeout.

`update` and `rebuild` open a terminal running commands *you* configured, from the flake's
directory, and nothing else. `rebuild` with no host named is refused when the flake defines several
-- picking one for you is how the wrong machine gets rebuilt.

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
ok   capture           saving to /home/brian/Pictures/Screenshots
ok   provider sync     shell and backend agree on 7 providers
```

`capture` reports the screenshot tools EpochOxide can reach. They are installed separately from
EpochShell, and a missing one is otherwise only noticed at the moment someone presses their
screenshot key.

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
