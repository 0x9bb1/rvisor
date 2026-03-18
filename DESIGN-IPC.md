# IPC Actor Design

## Goal

Move supervisor state management to a single actor task so all state transitions are strictly
ordered and owned by one executor. This removes cross-task locks while keeping the external CLI
and IPC protocol unchanged. The refactor is driven by tests: every state transition is covered
by a unit test written before the handler code.

## Scope

- Replace `Arc<Mutex<Supervisor>>` with an actor that owns `Supervisor` exclusively.
- IPC handlers hold a `SupervisorHandle` (cheap-to-clone `mpsc::Sender` wrapper) instead of
  `Arc<Mutex<Supervisor>>`.
- All internal events (process exit, readiness timer, stop completion) report back to the actor
  via the **same mpsc channel** — fully channel-based, no shared memory.
- No changes to socket protocol (JSON + length-delimited) or CLI surface.
- **One-shot migration**: `Arc<Mutex<Supervisor>>` and all lock sites are removed in a single
  PR. No intermediate states, no backwards-compatibility shims.

## Non-Goals

- Changing the wire protocol or transport.
- Adding multi-host federation or clustering.
- Rewriting process spawning semantics beyond necessary refactoring.
- **Renaming `Supervisor` → `Rvisor`**: this is a mechanical rename with no architectural
  impact. It will be done in a **separate PR** (either before or after this refactor) to keep
  the diff reviewable and the rename trivially revertable.

## Current State

- `Supervisor` is shared via `Arc<Mutex<_>>` and mutated from multiple async tasks.
- Long operations already avoid holding locks across fork/exec via a multi-phase pattern, but
  this discipline is implicit and fragile.
- `start_program_inner` recursively calls itself (via spawned tasks) for autorestart — hard to
  trace and test.
- `update` has a race window: it releases the lock before calling `stop/start`, allowing
  unrelated commands to interleave and observe inconsistent state.

## Proposed Architecture

A dedicated `actor_loop` task owns `Supervisor` exclusively. All callers communicate with it
through a single bounded `mpsc::Sender<Command>`.

### Command Enum (complete)

```rust
enum Command {
    // ── External commands (from IPC handlers) ──────────────────────────
    Status   { program: Option<String>, reply: oneshot::Sender<Vec<ProgramStatus>> },
    Start    { program: Option<String>, reply: oneshot::Sender<anyhow::Result<String>> },
    Stop     { program: Option<String>, reply: oneshot::Sender<anyhow::Result<String>> },
    Restart  { program: Option<String>, reply: oneshot::Sender<anyhow::Result<String>> },
    Signal   { program: Option<String>, signal: String, reply: oneshot::Sender<anyhow::Result<String>> },
    Reread   { config: SupervisordConfig, reply: oneshot::Sender<anyhow::Result<RereadSummary>> },
    Update   { config: SupervisordConfig, reply: oneshot::Sender<anyhow::Result<UpdateSummary>> },
    Reload   { config: SupervisordConfig, reply: oneshot::Sender<anyhow::Result<ReloadSummary>> },
    Avail    { reply: oneshot::Sender<Vec<String>> },
    Pid      { reply: oneshot::Sender<u32> },
    Clear    { program: Option<String>, reply: oneshot::Sender<anyhow::Result<String>> },
    Add      { name: String, config: SupervisordConfig, reply: oneshot::Sender<anyhow::Result<String>> },
    Remove   { name: String, reply: oneshot::Sender<anyhow::Result<String>> },
    EventsSubscribe { reply: oneshot::Sender<broadcast::Receiver<Event>> },
    LogTail  { params: LogTailParams, reply: oneshot::Sender<anyhow::Result<LogTailReply>> },
    Shutdown { reply: oneshot::Sender<anyhow::Result<()>> },

    // ── Internal commands (from background worker tasks) ───────────────
    InternalReady   { name: String, pid: i32 },
    InternalExit    { name: String, pid: i32, exit_code: i32 },
    InternalStopped { name: String, pid: i32, result: anyhow::Result<()> },
}
```

**Note on `Reread` / `Update` / `Reload` / `Add`**: the IPC handler reads the config file with
`spawn_blocking(|| config::load(...))` **before** sending the command, and passes the parsed
`SupervisordConfig` as part of the message. The actor never touches the filesystem — no
blocking I/O inside the actor loop. If `config::load` returns an error, the IPC handler returns
`ok: false` to the client immediately without sending any command to the actor.

### SupervisorHandle

The public interface — a cheap-to-clone wrapper around `mpsc::Sender<Command>`. `ipc.rs` holds
this instead of `Arc<Mutex<Supervisor>>`.

```rust
#[derive(Clone)]
pub struct SupervisorHandle {
    tx: mpsc::Sender<Command>,
}

impl SupervisorHandle {
    pub async fn status(&self, program: Option<String>) -> anyhow::Result<Vec<ProgramStatus>>;
    pub async fn start(&self, program: Option<String>) -> anyhow::Result<String>;
    pub async fn stop(&self, program: Option<String>) -> anyhow::Result<String>;
    pub async fn restart(&self, program: Option<String>) -> anyhow::Result<String>;
    pub async fn signal(&self, program: Option<String>, signal: String) -> anyhow::Result<String>;
    pub async fn reread(&self) -> anyhow::Result<RereadSummary>;
    pub async fn update(&self) -> anyhow::Result<UpdateSummary>;
    pub async fn reload(&self) -> anyhow::Result<ReloadSummary>;
    pub async fn avail(&self) -> anyhow::Result<Vec<String>>;
    pub async fn pid(&self) -> anyhow::Result<u32>;
    pub async fn clear(&self, program: Option<String>) -> anyhow::Result<String>;
    pub async fn add(&self, name: String) -> anyhow::Result<String>;
    pub async fn remove(&self, name: String) -> anyhow::Result<String>;
    pub async fn events_subscribe(&self) -> anyhow::Result<broadcast::Receiver<Event>>;
    pub async fn logtail(&self, params: LogTailParams) -> anyhow::Result<LogTailReply>;
    pub async fn shutdown(&self) -> anyhow::Result<()>;
}
```

`reread`, `update`, `reload`, `add` each call `spawn_blocking` to load config before sending
the command. All other methods are pure channel sends.

Each method awaits the reply oneshot with a **30 s timeout**; on expiry it returns
`Err("supervisor actor timed out")`. If the actor is dead (channel closed), the method returns
`Err("supervisor actor is not running")` — no silent hangs.

### Queue Behavior

The mpsc channel is bounded at **256**.

- **External commands** (from IPC handlers): `tx.send_timeout(cmd, Duration::from_secs(5))`.
  If still full after 5 s, the IPC handler returns `ok: false, message: "server busy"` to the
  client.
- **Internal commands** (`InternalReady`, `InternalExit`, `InternalStopped`): worker tasks use
  `tx.send(cmd).await` — unbounded wait, never dropped. Dropping an internal message would
  leave `pending_*` maps unresolved and cause permanent state stalls; the queue being full for
  5+ s from internal pressure indicates a more serious bug, not a normal backpressure scenario.

**Behavior change from current**: the old design waited indefinitely for the mutex. The new
design has a 5 s enqueue timeout for external commands. In practice the queue will be empty for
a process supervisor at this scale; the timeout is a safety valve, not a normal operating
condition.

## Flow: Start a Program

1. IPC handler calls `handle.start(Some("foo"))`.
2. Actor receives `Command::Start` → validates state → sets `STARTING` → emits event → calls
   `process::spawn_program(...)` to get `SpawnedProcess { pid, child, log_handles }`, then
   spawns two worker tasks:
   - **Readiness timer**: holds `tx.clone()`, sleeps `startsecs`, sends
     `InternalReady { name, pid }`.
   - **Exit monitor**: takes ownership of `child` **and** `log_handles` (both moved in, never
     stored in `ProgramHandle`). Awaits `child.wait()`, then drains all `log_handles` to
     completion, then sends `InternalExit { name, pid, exit_code }`.
3. Actor stores only `pid: i32` in `ProgramHandle` — no `Arc`, no `JoinHandle`, no shared
   memory. Sends `"foo started"` reply immediately — handler unblocks.
4. `InternalReady` arrives: actor checks `pid == handle.pid && state == STARTING`, transitions
   to `RUNNING`, emits event.
5. `InternalExit` arrives: actor checks `pid`, decides BACKOFF/EXITED/FATAL. If restarting,
   spawns a delay task that sends `Command::Start` back after the backoff — no recursion.

**Log handles ownership**: `log_handles` are moved into the exit monitor at spawn time and
never stored in `ProgramHandle`. The stop worker does not touch log handles; the exit monitor
always drains them after `child.wait()` returns, regardless of whether the process exited
naturally or was killed by the stop worker. `InternalStopped` arrives before `InternalExit`
in the kill case — the actor resolves the stop reply on `InternalStopped`, then ignores the
subsequent `InternalExit` (pid mismatch check). Log flushing is guaranteed either way.

## Flow: Stop a Program

Stopping can take `stopwaitsecs` seconds. The actor must not block its loop:

1. Actor receives `Command::Stop` → validates state → sets `STOPPING` → sends stop signal →
   **stores the reply oneshot** in `pending_stops: HashMap<String, oneshot::Sender<...>>` →
   spawns a **stop worker task**.
2. Actor returns to its `recv()` loop immediately — other programs can start/stop concurrently.
3. Stop worker: polls `process_alive(pid)` every 100 ms until dead or deadline, sends SIGKILL
   if needed, awaits log handles. Then sends `InternalStopped { name, pid, result }` — always,
   even on error (e.g., process vanished, SIGKILL failed). `result` carries the error if any.
   The "always sends" guarantee is enforced via a finally-like pattern: the worker body returns
   `anyhow::Result<()>`, and the `tokio::spawn` wrapper unconditionally sends
   `InternalStopped` with that result after the body returns or panics
   (`AssertUnwindSafe` + `catch_unwind`).
4. Actor handles `InternalStopped`: checks pid, sets `STOPPED`, emits event, resolves the
   stored oneshot with `result` → IPC handler gets reply.

`Command::Stop { program: None }` (stop-all) inserts one entry per program into `pending_stops`
with a shared aggregating counter; the reply is sent once all entries resolve.

## Flow: Update

1. **IPC handler** (not actor): calls `spawn_blocking(|| config::load(...))` to read and parse
   config. If parsing fails, the IPC handler returns `ok: false` immediately — the actor is
   never involved and no command is sent.
2. IPC handler sends `Command::Update { config, reply }` to actor.
3. Actor receives `Command::Update` → computes diff inline (add/remove/change sets).
4. For removed/changed-running programs: sets `STOPPING`, spawns stop worker, stores name in
   `pending_update.waiting`.
5. Stores reply oneshot + `UpdateSummary` skeleton in `pending_update: Option<PendingUpdate>`.
6. Returns to `recv()` loop immediately.
7. As `InternalStopped { name, result }` messages arrive: actor removes name from
   `pending_update.waiting`. When set is empty, starts added/restarted programs inline, sends
   the accumulated `UpdateSummary` reply.

**Isolation caveat**: while waiting for `InternalStopped`, unrelated commands (e.g. `status`,
`start foo`) can interleave. This is acceptable. The meaningful race window from the old design
(lock released between diff and apply) is closed because the actor applies the diff atomically
in step 3–4 before returning to the recv loop.

## Flow: Restart

`Restart` = stop-then-start. Since stop is async, the actor tracks this in `pending_restart`:

1. Actor receives `Command::Restart { program, reply }`.
2. Triggers stop for the target program(s) — same as `Command::Stop`, but stores name(s) in
   `pending_restart: HashMap<String, oneshot::Sender<...>>` instead of `pending_stops`.
3. Returns to `recv()` loop.
4. When `InternalStopped { name, result }` arrives and `name` is in `pending_restart`: removes
   it, triggers start inline, sends restart reply after start handler completes (or propagates
   error if `result` is `Err`).

## Actor Loop Structure

```rust
loop {
    let Some(cmd) = rx.recv().await else { break };  // channel closed → clean exit
    match cmd {
        Command::Start    { .. } => handle_start(&mut supervisor, &tx, cmd),
        Command::Stop     { .. } => handle_stop(&mut supervisor, &tx, &mut pending_stops, cmd),
        Command::Restart  { .. } => handle_restart(&mut supervisor, &tx, &mut pending_restart, cmd),
        Command::Update   { .. } => handle_update(&mut supervisor, &tx, &mut pending_update, cmd),
        Command::InternalStopped { name, pid, result } => {
            handle_internal_stopped(&mut supervisor, &tx, name, pid, result,
                &mut pending_stops, &mut pending_restart, &mut pending_update);
        }
        Command::InternalReady { name, pid }           => handle_internal_ready(&mut supervisor, name, pid),
        Command::InternalExit  { name, pid, exit_code } => handle_internal_exit(&mut supervisor, &tx, name, pid, exit_code),
        // ... other variants
    }
}
```

**Pending state fields on the actor loop** (not on `Supervisor`):

| Field | Type | Purpose |
|-------|------|---------|
| `pending_stops` | `HashMap<String, oneshot::Sender<Result<String>>>` | stop reply awaiting `InternalStopped` |
| `pending_restart` | `HashMap<String, oneshot::Sender<Result<String>>>` | restart reply awaiting `InternalStopped` then start |
| `pending_update` | `Option<PendingUpdate>` | update reply awaiting all `InternalStopped` |

```rust
struct PendingUpdate {
    reply:    oneshot::Sender<anyhow::Result<UpdateSummary>>,
    summary:  UpdateSummary,
    waiting:  HashSet<String>,  // programs whose InternalStopped is still in flight
    to_start: Vec<String>,      // programs to start once waiting is empty
}
```

## Concurrency Model

```
IPC handler task ──┐  send_timeout(5s)
IPC handler task ──┤  mpsc::Sender<Command>       ┌─ actor_loop (owns Supervisor)
IPC handler task ──┼─────────────────────────────►│
                   │                               │  spawns workers with tx.clone()
Readiness timer ───┤◄─ oneshot reply (30s timeout)─┘
Exit monitor ──────┘
Stop worker ───────┘  (always sends InternalStopped, even on error)
```

- Only `actor_loop` mutates `Supervisor`.
- Worker tasks hold `mpsc::Sender<Command>` (for `Internal*` messages) — no shared memory.
- `ProgramHandle` stores only `pid: i32`; all other per-process resources (`child`, `log_handles`)
  are moved into worker tasks at spawn time and never shared.
- `broadcast::Sender<Event>` lives inside the actor; callers receive a `Receiver` via
  `EventsSubscribe`. The `event_seq` counter is a plain `u64` inside the actor — no atomics.
- Config file I/O (`config::load`) happens in IPC handler tasks via `spawn_blocking`, never
  inside the actor loop.
- The channel primitives (mpsc, oneshot, broadcast) use internal synchronization, but this is
  below the abstraction boundary — no user-visible shared memory exists anywhere in the design.

## TDD Strategy

### Relationship to "One-Shot Migration"

"One-shot migration" means the PR is atomic from a git perspective: it ships with
`Arc<Mutex<Supervisor>>` fully removed. It does **not** mean code is written in one sitting.
TDD is the development methodology inside that PR: code is committed incrementally, one Command
variant at a time, with tests green at each step.

### Principles

- **Tests first, implementation second.** For each `Command` variant, write a failing test that
  asserts the expected state transition, then write the handler code to make it pass.
- **Test the actor directly, not through the socket.** Unit tests call `spawn_actor(config)`
  which returns a `SupervisorHandle`. No sockets, no network.
- **Existing integration tests are the regression gate.** `tests/milestones.rs` runs against a
  real daemon. They must pass unchanged at the end of the refactor.

### Test Layers

**Layer 1 — Actor unit tests** (`tests/actor.rs`)

Tests use a helper `spawn_test_actor(config)` that starts the actor loop with a test config.
Programs that run real processes use `command = "true"` or `command = "sleep 9999"`.

```rust
#[tokio::test]
async fn start_transitions_to_running() {
    let handle = spawn_test_actor(one_program("echo", "echo hi", startsecs=0)).await;
    handle.start(Some("echo".into())).await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;  // let InternalReady arrive
    let s = handle.status(Some("echo".into())).await.unwrap();
    assert_eq!(s[0].state, "RUNNING");
}
```

Tests to write (one failing test per row before implementing the handler):

| # | Test name | Verifies |
|---|-----------|----------|
| 1 | `status_returns_stopped_for_new_program` | initial state |
| 2 | `start_transitions_starting_to_running` | happy path start |
| 3 | `start_already_running_is_idempotent` | guard on Running state |
| 4 | `stop_running_program_transitions_to_stopped` | stop flow + InternalStopped |
| 5 | `stop_worker_error_still_resolves_pending` | InternalStopped with Err propagates |
| 6 | `restart_stops_then_starts` | restart sequence |
| 7 | `exit_autorestart_always_triggers_backoff` | autorestart=always |
| 8 | `exit_autorestart_unexpected_on_expected_code` | autorestart=unexpected |
| 9 | `exit_autorestart_never_transitions_to_exited` | autorestart=never |
| 10 | `startretries_exceeded_transitions_to_fatal` | retry cap |
| 11 | `update_stops_removed_starts_added` | update diff |
| 12 | `update_restarts_changed_running_programs` | update changed |
| 13 | `signal_running_program` | signal dispatch |
| 14 | `clear_logs_truncates_files` | clear command |
| 15 | `add_program_from_config` | add command |
| 16 | `remove_stops_running_program` | remove command |
| 17 | `shutdown_stops_all_programs` | shutdown flow |
| 18 | `events_subscribe_receives_state_transitions` | broadcast channel |
| 19 | `handle_returns_error_when_actor_is_dead` | fault tolerance |
| 20 | `handle_returns_error_when_queue_is_full` | backpressure / send_timeout |

**Layer 2 — Integration tests** (`tests/milestones.rs`)

Existing tests are preserved unchanged as the acceptance gate. They test the full stack:
real daemon process, real socket, real JSON protocol.

### TDD Workflow

```
for each Command variant:
  1. write failing unit test (happy path)
  2. implement actor handler
  3. write failing unit test (error paths)
  4. fix handler
  5. cargo test   # all tests green
  6. git commit

after all variants:
  7. cargo test --test milestones   # must pass green
  8. cargo clippy && cargo fmt
```

## Benefits

- Strong ordering: state transitions are strictly serialized.
- `update` diff-apply race window closed: diff is computed and state is mutated before the
  actor yields; concurrent commands can no longer revive a program mid-update.
- `start_program_inner` self-recursion replaced by `InternalExit → actor → Start` — linear,
  traceable, testable.
- No blocking I/O inside the actor loop: config reads go through `spawn_blocking` in the IPC
  handler before reaching the actor.
- Actor fault tolerance: closed channel or timeout surfaces as a structured error, not a hang.
- TDD ensures every state transition is verified before it can regress.

## Tradeoffs

- All commands serialized through the actor, including read-only `status`/`avail`/`pid`.
  (Acceptable: these are fast in-memory reads; queue depth is expected to be near zero.)
- Large one-shot change — `supervisor.rs` and `ipc.rs` are rewritten entirely.
- More wiring: command enum, `SupervisorHandle`, internal reply tracking.
- **Behavior change**: old design waited indefinitely for mutex; new design has a 5 s enqueue
  timeout. Acceptable for a process supervisor where the queue is normally empty.

## Risks and Mitigations

| Risk | Mitigation |
|------|------------|
| Actor stall from blocking I/O | Config reads via `spawn_blocking` in IPC handler; actor loop contains no `std::fs` calls |
| Stop worker crashes without sending InternalStopped | Stop worker wraps entire body in a handler that always sends `InternalStopped { result: Err(...) }` on panic or early exit |
| pending_* entries leak if InternalStopped is never sent | Stop worker contract: always sends InternalStopped; enforced by test #5 |
| Actor task panics, all handles hang | `SupervisorHandle::send_command` detects closed channel and returns structured error; IPC handler returns `ok: false` |
| `InternalStopped` arrives for stale pid | All `Internal*` handlers validate `pid == handle.pid` before acting |
| `log_handles` shared between stop worker and exit monitor | `log_handles` moved into exit monitor at spawn time; stop worker never touches them; `ProgramHandle` stores only `pid: i32` |
| `update` diff-apply race window | Diff computed and state mutated atomically before actor yields; subsequent interleaving is benign |
| Queue full under load | `send_timeout(5s)` in `SupervisorHandle`; IPC handler returns `ok: false, "server busy"` |

## Open Questions (resolved)

- **Priority queues?** No. Single FIFO queue is sufficient for this workload.
- **Status caching?** No. Status is a cheap in-memory read inside the actor.
- **Queue length?** 256. `send_timeout(5s)` as backpressure.
- **IPC handler oneshot timeout?** 30 s; returns `ok: false` on expiry.
- **Streaming commands (logtail --follow, events)?** Initial setup goes through the actor (one
  command to get the log path or broadcast receiver), then the streaming loop runs in the IPC
  handler task outside the actor queue — same as today.
- **Rename Supervisor → Rvisor?** Deferred to a separate PR.
