# rvisor

Rust-based process supervisor with a local IPC control plane and a `rvisor ctl` CLI.

It is aimed at the same operational space as `supervisord`, but uses TOML config and a local Unix socket control API instead of XML-RPC and INI.

## Features

- Run a foreground or daemonized supervisor from the same binary.
- Start, stop, restart, reread, update, and reload managed programs through `rvisor ctl`.
- Stream process events and tail program logs over local IPC.
- Support `autostart`, `autorestart`, `startsecs`, `startretries`, `stopwaitsecs`, `numprocs`, environment injection, and log rotation.
- Install and manage a user service through `systemd --user` on Linux or `launchd` on macOS.

## Install

Build from source:

```bash
cargo build
```

Install from the local checkout:

```bash
cargo install --path .
```

Or install directly from Git:

```bash
cargo install --git https://github.com/0x9bb1/rvisor.git
```

Run the test suite:

```bash
cargo test
```

## Quick Start

Generate or copy a config template:

```bash
rvisor init -o supervisord.toml
# or
cp examples/supervisord.toml ./supervisord.toml
```

Start the supervisor in the foreground:

```bash
rvisor -c ./supervisord.toml run
```

Control it from another shell:

```bash
rvisor -c ./supervisord.toml ctl status
rvisor -c ./supervisord.toml ctl start example
rvisor -c ./supervisord.toml ctl tail example --follow
```

Run it as a daemon instead:

```bash
rvisor -c ./supervisord.toml --daemon run
```

A minimal edit-and-apply workflow looks like this:

```bash
$EDITOR ./supervisord.toml
rvisor -c ./supervisord.toml ctl reread
rvisor -c ./supervisord.toml ctl update
rvisor -c ./supervisord.toml ctl status
```

## Example Config

Minimal example:

```toml
[supervisord]
sock_path = "/tmp/rvisor.sock"

[[programs]]
name = "example"
command = "sleep 60"
autostart = true
autorestart = "unexpected"
stdout_log = "/tmp/example.out.log"
stderr_log = "/tmp/example.err.log"
```

See [`examples/supervisord.toml`](/mnt/e/code/rvisor/examples/supervisord.toml) for a fuller example.

## Common Commands

Show process state:

```bash
rvisor ctl status
rvisor ctl status example
```

Control processes:

```bash
rvisor ctl start example
rvisor ctl stop example
rvisor ctl restart example
rvisor ctl signal TERM example
```

Apply config changes:

```bash
rvisor ctl reread
rvisor ctl update
rvisor ctl reload
```

Inspect logs and events:

```bash
rvisor ctl tail example
rvisor ctl tail example --follow
rvisor ctl logtail example --stderr --lines 100
rvisor ctl maintail --follow
rvisor ctl events
```

Useful output modes:

```bash
rvisor ctl status --json
rvisor ctl shell
rvisor version
```

## Config Resolution

If `-c` is omitted, `rvisor` searches for a config in this order:

1. `RVISOR_CONFIG` when set
2. `./supervisord.toml`
3. `./etc/supervisord.toml`
4. `/etc/supervisord.toml`
5. `/etc/rvisor/supervisord.toml`
6. `/etc/supervisor/supervisord.toml`
7. `../etc/supervisord.toml` relative to the executable
8. `../supervisord.toml` relative to the executable

## Service Management

Manage a user service:

```bash
rvisor -c ./supervisord.toml service install
rvisor service start
rvisor service status
rvisor service enable
rvisor service restart
```

The service subcommands use `systemd --user` on Linux and `launchd` on macOS. The implemented
subcommands are `install`, `uninstall`, `start`, `stop`, `status`, `enable`, `disable`,
`restart`, and `reload`.

On Linux, `rvisor service install` writes `~/.config/systemd/user/rvisor.service` and points
`ExecStart` at the current `rvisor` binary with `run` and the `-c` path you passed to install.
A typical setup looks like this:

```bash
rvisor -c ~/.config/rvisor/supervisord.toml service install
systemctl --user daemon-reload
systemctl --user start rvisor.service
systemctl --user status rvisor.service
```

On macOS, `rvisor service install` writes `~/Library/LaunchAgents/com.rvisor.plist` and uses
`launchctl` under the hood for start and stop operations:

```bash
rvisor -c ~/.config/rvisor/supervisord.toml service install
launchctl load ~/Library/LaunchAgents/com.rvisor.plist
launchctl list com.rvisor
```
