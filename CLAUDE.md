# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Commands

```bash
cargo build                  # debug build
cargo build --release        # release build
cargo test                   # run all tests
cargo clippy                 # lint
cargo fmt                    # format
```

Run a single test:
```bash
cargo test <test_name>
```

## Architecture

rvisor is a Unix process supervisor daemon written in Rust. It is a single binary that acts as both the daemon (`rvisor run`) and the control client (`rvisor ctl <command>`).

### IPC Model

The daemon exposes a **Unix Domain Socket** (default `/tmp/rvisor.sock`). The protocol is **JSON over a length-delimited framing** (via `tokio-util` `LengthDelimitedCodec`). The CLI subcommands in `main.rs` connect to this socket and send/receive JSON.

### Key Modules

| Module | Role |
|--------|------|
| `main.rs` | CLI entry point (clap), daemonization (double-fork), signal handling, async runtime setup |
| `actor.rs` | Actor loop that owns the `Rvisor` state machine; receives `Command` messages over an `mpsc` channel and dispatches them; the `RvisorHandle` is the only way external code interacts with the supervisor |
| `supervisor.rs` | Pure state types (`ProgramState`, `ProgramStatus`, `ProgramHandle`, `Event`, `RereadSummary`, `UpdateSummary`, `ReloadSummary`) and the `Rvisor` struct with all transition logic |
| `ipc.rs` | Unix socket server (daemon side) and client helpers (CLI side); handles streaming for log-tail and event-feed commands; config file watching via `notify` |
| `process.rs` | Spawns child processes via Tokio, captures stdout/stderr, size-based log rotation |
| `persist.rs` | State snapshot serialization — saves/loads `StateSnapshot` (JSON) at `<sock>.state` for daemon restart recovery |
| `config.rs` | TOML parsing into `Config` / `ProgramConfig`; config search path logic |
| `service.rs` | Service manager integration for `systemd --user` (Linux) and `launchd` (macOS) |
| `logging.rs` | Initializes `tracing-subscriber` with `RUST_LOG` env filter (default: `info`) |

### Data Flow

```
CLI subcommand (main.rs)
  └─ connects to Unix socket (ipc.rs client helpers)
       └─ sends JSON command
            └─ supervisor.rs handles command
                 └─ process.rs spawns / kills processes
```

### Configuration Format

TOML (not INI like the original supervisord). Search order:
1. `RVISOR_CONFIG` env var
2. `./supervisord.toml`, `./etc/supervisord.toml`
3. `/etc/supervisord.toml`, `/etc/rvisor/supervisord.toml`, `/etc/supervisor/supervisord.toml`
4. `../etc/supervisord.toml`, `../supervisord.toml` (relative to executable)

### Daemonization

`rvisor -d run` performs the classic double-fork before starting the Tokio runtime. The child writes its PID to `pidfile` and redirects stdio. This logic lives at the top of `main.rs`.

### IPC Protocol

`ipc::Request` fields: `command`, `program`, `lines`, `stream` (`stdout`/`stderr`), `follow`, `signal`, `offset`, `bytes`, and `since`.
`ipc::Response` fields: `ok`, `message`, `data` (JSON value).

Available `ctl` commands: `start`, `stop`, `restart`, `status`, `signal`, `reread`, `update`, `reload`, `shutdown`, `pid`, `logtail`, `tail`, `maintail`, `events`, `avail`, `clear`, `add`, `remove`, `fg`, and `shell`.

### Config File Watching

`ipc.rs` uses the `notify` crate to watch the directory containing the config file. On modification of the active config file, it triggers an automatic `reread` + `update` cycle inside the daemon.

### Actor Pattern

The supervisor uses the actor pattern to avoid shared-state concurrency. The actor loop runs in a dedicated Tokio task and is the single owner of `Rvisor` state. All external interactions go through typed `Command` variants with `oneshot` reply channels; `RvisorHandle` (a thin `mpsc::Sender<Command>` wrapper) is cloned and passed to the IPC layer.

### Tests

- `tests/actor.rs` — actor/supervisor logic tests; spawn real child processes but no daemon.
- `tests/milestones.rs` — end-to-end tests that start a real daemon and communicate over the socket; require a Unix environment. A `tempfile`-backed config and socket path keep them self-contained.

```bash
cargo test --test actor       # actor unit tests
cargo test --test milestones  # integration tests
```
