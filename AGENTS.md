# Repository Guidelines

## Project Structure & Module Organization
- `src/` contains the Rust modules: `main.rs` (CLI + daemon entry), `supervisor.rs` (state machine), `ipc.rs` (Unix socket IPC), `process.rs` (child process lifecycle), `config.rs` (TOML config parsing), `service.rs` (systemd helpers), and `logging.rs`.
- `tests/` holds integration tests (notably `tests/milestones.rs`).
- `examples/` provides sample configs such as `examples/supervisord.toml`.
- `target/` is build output; don't edit or commit generated files.

## Build, Test, and Development Commands
- `cargo build`: debug build.
- `cargo build --release`: optimized build.
- `cargo test`: run all tests (also what CI runs).
- `cargo test <test_name>` or `cargo test --test milestones`: run a specific test.
- `cargo fmt`: format with rustfmt.
- `cargo clippy`: run lint checks.

## Coding Style & Naming Conventions
- Rust 2021 edition with standard rustfmt defaults; run `cargo fmt` before committing.
- Prefer `snake_case` for functions/variables, `CamelCase` for types, `SCREAMING_SNAKE_CASE` for constants.
- Keep modules focused; new functionality should map to existing module boundaries (IPC, supervisor state, process control, config).

## Testing Guidelines
- Integration tests are in `tests/milestones.rs` and spin up a real daemon with a temp config and socket.
- Tests assume a Unix-like environment (Unix domain sockets).
- Aim to add or update tests when changing IPC commands, supervisor state transitions, or config parsing.

## Commit & Pull Request Guidelines
- Commit messages in this repo are short, imperative, and prefix-free (e.g., “Fix cross-platform build failures in release CI”).
- PRs should include a clear description, the commands used to validate changes (e.g., `cargo test`, `cargo clippy`), and note any config or behavior changes.

## Architecture Notes
- The single binary acts as both daemon (`rvisor run`) and CLI client (`rvisor ctl ...`).
- IPC is JSON over length-delimited Unix domain sockets; config changes are detected with `notify`.
