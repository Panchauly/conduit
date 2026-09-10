# Phase 17 — Event Sources 🎯 (Completed)

**Goal:** Give Conduit an explicit, resumable **source** abstraction — the symmetric other half of the `StorageAdapter` (sink) side. Today events reach the engine only as a file path on the CLI. Phase 17 makes "where events come from" a pluggable, checkpointed component, and turns `conduit run` into a continuous poll → dispatch → commit loop.

**Why:**
- `architecture.md` §2.1: *"Events are the **only** input to Conduit"* — but there is no input *mechanism*. `conduit run --event <file>` takes one file; `conduit replay --path <dir>` drains a directory once. No resume, no continuous operation, no way to point Conduit at anything but a local file.
- The sink side is now deep and finished: 4 storage kinds, sequence-gated `decide()`, facets, tombstones, capability-safe validation, dependency graphs. The source side is a bare function. The architecture is lopsided.
- Every Phase 11–16 guard makes the sink **idempotent**. That is exactly the property that lets an **at-least-once** source + a checkpoint give **effectively-once projection**. Phase 17 is where that latent guarantee is actually closed — nothing before it can crash-and-resume.

**Staying in scope:** `architecture.md` §1 — Conduit is *"intentionally not a streaming platform."* A source does **not** own transport, durability, partitioning, or fan-out. It reads from something that already has those properties (a directory, a stdin pipe, later a Kafka topic or an outbox table) and hands Conduit ordered batches with a resumable position. That is consumption, not a broker.

---

## Phase 17.1 — The `EventSource` trait & config 🚰 (Completed)

**Scope**
- New trait, mirroring `StorageAdapter`:

  ```rust
  pub trait EventSource {
      fn id(&self) -> &str;
      /// Next batch of events in delivery order, or empty when caught up.
      /// Each event carries the SourcePosition needed to commit past it.
      fn poll(&mut self, max_batch: usize) -> Result<Vec<SourcedEvent>, SourceError>;
      /// Durably record that everything up to `position` has been projected.
      fn commit(&mut self, position: SourcePosition) -> Result<(), SourceError>;
  }

  pub struct SourcedEvent { pub event: Event, pub position: SourcePosition }
  pub struct SourcePosition(String);   // opaque, source-defined, lexative-comparable within one source
  ```

- `ConduitConfig` gains `sources: Vec<SourceConfig>` — a `#[serde(tag = "type")]` enum, same pattern as `AdapterConfig`:
  - `directory` — watches a dir for new `*.json` / NDJSON files (formalises today's `replay` loader, including the Phase 15.5 `(entity_key, sequence)` batch sort).
  - `stdin` — NDJSON on stdin, one event per line (for `cat events.ndjson | conduit run`).
- Routing is unchanged — events route by `event_type` regardless of which source produced them. A `source_id` is attached for observability only.

**Guarantees**
- Sources are as explicit and config-declared as adapters — no implicit "look for events in the cwd."
- The trait is transport-agnostic: a future Kafka / outbox source implements the same three methods.

**As built**
- New `crate::source` module: `mod.rs` (`EventSource`, `SourcedEvent`, `SourcePosition`, `SourceError`), `checkpoint.rs`, `directory.rs`, `stdin.rs` (generic over `R: BufRead` so tests feed a `Cursor`), `runner.rs`. `EventSource` gained a fourth method, `committed_position()`, for `conduit sources` and resume introspection.
- `SourceConfig` (`#[serde(tag = "type")]`, `directory` / `stdin`) with `#[serde(default)]` on `ConduitConfig.sources` — existing configs parse unchanged; `ConduitConfig::validate` rejects duplicate source ids (`ConfigError::DuplicateSourceId`). `path` / `state_dir` resolve relative to the config file's directory, like `routing.file`.
- The `directory` source never splits a file across batches (position = file name); `stdin` positions are zero-padded line numbers.

---

## Phase 17.2 — Checkpointing & resume 📍 (Completed)

**Scope**
- Source state persisted to `conduit_source_state` — a sidecar file `{state_dir}/{source_id}.json` holding `{ position: SourcePosition, committed_at }` (structurally the same idea as the projection guard, kept separate).
- On startup a source resumes from its committed position; with none, it starts from the source's beginning (directory: no files seen; stdin: line 0).
- `commit(position)` is called by the run loop **only after** the whole batch has been dispatched and every adapter outcome recorded (success, skip, or DLQ'd). A crash before `commit` re-polls the same batch — safe because every sink is idempotent (Phases 11–16).

**Guarantees**
- **Effectively-once projection:** at-least-once redelivery from a source + sequence-gated idempotent sinks ⇒ the projected state after a crash-and-resume is byte-identical to a clean run. This is the guarantee the whole 11–16 arc was building toward.
- A source's committed position never moves backward.

**As built**
- `checkpoint.rs`: `{state_dir}/{source_id}.json` = `{ position, committed_at }`, written via write-temp-then-rename. `committed_at` is Unix seconds (no time-formatting dependency).
- `commit` is a no-op when `position <= committed` (monotonicity enforced at the source, not just by the loop).
- Named `conduit_source_state` in the doc; the file is per-source rather than a single table — the projection guard's SQL analog doesn't apply since sources aren't SQL.

---

## Phase 17.3 — The continuous run loop 🔁 (Completed)

**Scope**
- `conduit run` becomes: build adapters + sources once → loop { for each source: `poll` → sort batch by `(entity_key, sequence)` (Phase 15.5) → `dispatch_batch` → on full success `commit` } → sleep briefly when all sources are caught up.
- `--once` flag: drain every source to its current end, commit, exit 0. This **subsumes `replay`** — `conduit replay --path dir` becomes `conduit run --once` with a `directory` source. `replay` stays as a thin deprecated alias for one release.
- Graceful shutdown on SIGINT/SIGTERM: stop polling, finish the in-flight batch, `commit`, exit. Never a partial-batch commit.
- `--event <file>` (single-event `run`) stays as a convenience — internally a one-shot `directory`-style source over a single file.

**Guarantees**
- The loop commits a source position only on an all-adapters-resolved batch; a batch containing a hard adapter failure (not a DLQ'd poison event) is not committed and is retried.
- Deterministic across restarts: stopping mid-stream and resuming yields the same projected state as running straight through.

**As built**
- `crate::source::runner`: `run_sources` (builds adapters from config) delegates to `run_sources_with_adapters` (pre-built adapters — the test injection point). `RunMode::{Once, Continuous { poll_interval }}`, `SourceRunOptions { mode, max_batch, retry_budget, dlq_dir }`, `SourceRunReport` with per-batch `BatchSummary` and a `StoppedReason` (`CaughtUp` / `Shutdown` / `RetryExhausted`).
- **Batch sort is by `event.sequence`** alone, not `(entity_key, sequence)` — the same simplification the replay loop already makes (Phase 15.5 "As built"): a global stable sort delivers each entity's events in the order `decide()` needs, without resolving routing in the loop.
- Graceful shutdown: `conduit run` installs a `ctrlc` handler (new dependency, `conduit-cli` only) that sets an `AtomicBool`; the loop checks it between batches and during the idle sleep, finishes the in-flight batch, commits, and returns `StoppedReason::Shutdown`.
- `--event <file>` stays on the **existing** one-shot `pipeline::run` path (not re-expressed as a source) — zero behaviour change for that flag.
- `replay` is **unchanged** — not turned into a code-level alias. `run_once_equals_replay.rs` proves the equivalence; deprecating the subcommand is left for a release that also touches the CLI help.

---

## Phase 17.4 — Observability & CLI surface 🔎 (Completed)

**Scope**
- `ExecutionReport` gains `source_id` and `source_position` alongside the existing `trace_id` / version fields (stable-API addition, release-noted).
- `conduit run` text/JSON output: per-batch summary (source, position range, events dispatched, per-outcome counts, DLQ count), and a periodic "caught up" line.
- `conduit explain` / `dry-run` accept `--source` in place of `--event` to preview routing/projection for the next batch without writing or committing.
- `conduit sources` — list configured sources and their committed positions (like `conduit explain` for the input side).

**Guarantees**
- No new observability *mechanism* — `source_id` / `source_position` are fields on the existing report; the batch summary reuses `AdapterOutcome` counts.

**As built**
- `ExecutionReport` gains `source_id: Option<String>` + `source_position: Option<String>` (`#[serde(default, skip_serializing_if = "Option::is_none")]` — additive, no break; `execute_event*` signatures unchanged). `ExecutionReport::with_source(id, pos)` builder; the runner sets the fields on each dispatched report.
- `conduit run` (source mode) renders a per-batch line (source, position range, per-outcome counts, dlq, retries, committed?) plus totals and `StoppedReason`; `--output json` prints the whole `SourceRunReport`.
- `conduit sources` (text/JSON) lists each configured source and its committed position.
- **Deferred:** `explain --source` / `dry-run --source`. `conduit sources` covers input-side introspection; previewing the next uncommitted batch through `explain` is a follow-up (it needs a non-committing poll path).

---

## Phase 17.5 — Failure semantics & DLQ integration ⚠️ (Completed)

**Scope**
- **Poison event** (an event that fails projection deterministically): DLQ'd via the existing `replay.rs` DLQ envelope, the batch continues, the position **is** committed (the event is parked, not lost). Re-ingesting the DLQ remains a manual `conduit run --source directory=<dlq>`.
- **Transient adapter failure** (I/O, lock contention): the batch is abandoned uncommitted and retried on the next loop iteration, up to a configurable retry budget; exhausted → the run loop exits non-zero with the failing batch's position, for operator intervention.
- `FailurePolicy` (Phase 6.2: `fail_fast` / `continue_on_error`) governs within-batch behaviour, unchanged; Phase 17 only adds the across-batch retry/commit decision.

**Guarantees**
- No event is silently dropped: it is projected, skipped by a guard, DLQ'd, or the loop halts with a resumable position.
- A committed position always corresponds to a batch whose every event was projected, skipped, or DLQ'd — never one still mid-retry.

**As built**
- The runner **cannot distinguish** a poison event from a transient failure a priori, so the policy is one mechanism: a batch with any `ExecutionStatus::Failed` event is re-dispatched (whole batch — succeeded events re-run as idempotent skips) up to `retry_budget` times; still failing → if `dlq_dir` is set, each failing event is written to `{dlq_dir}/{event_id}.{position}.json` (`{ event, report }`), the batch is treated as resolved and **committed**; if not set, the loop returns `StoppedReason::RetryExhausted` + `halt_position` and the CLI exits non-zero. This is the "no DLQ envelope in the codebase yet" reality — Phase 17 builds the minimal one.
- **stdin resume is within-run only** (Phase 17 non-goal, made concrete): a fresh pipe has no history, so a restart of `cat … | conduit run` starts over. The checkpoint is still written and honoured if the same lines happen to be re-fed.

---

## Phase 17.6 — Determinism & verification suite 🧪 (Completed)

| Test | Asserts |
|---|---|
| `source_directory_resume.rs` | Project half a directory, kill before `commit`, restart → final state byte-identical to a straight run; no double-application. |
| `source_committed_not_reprocessed.rs` | After `commit`, restart → committed files are not re-polled (position honoured). |
| `source_duplicate_batch_idempotent.rs` | Force the same batch through twice (simulated pre-commit crash) → sinks converge, guards unchanged. |
| `source_stdin_ndjson.rs` | NDJSON piped to `run --once` → same projection as the equivalent directory source. |
| `source_out_of_order_batch.rs` | A batch with entity events shuffled → 15.5 sort applies them in sequence order; state correct. |
| `source_multi.rs` | Two directory sources feeding disjoint entity sets → both project; positions tracked independently. |
| `source_poison_event_parked.rs` | One deterministically-failing event in a batch → DLQ'd, batch commits, rest projected. |
| `source_transient_failure_retry.rs` | Adapter fails transiently then succeeds → batch retried, committed once, no duplication. |
| `run_once_equals_replay.rs` | `run --once` over a directory == the old `replay` output for the same fixtures. |

**Guarantee under test:** for any source, the projected state after an arbitrary sequence of crashes, restarts, and redeliveries equals the state of a single clean pass over the same events — because every sink is idempotent and positions commit only after full batch resolution.

**As built** — all nine test files exist under `crates/conduit-core/tests/` with those names and pass, sharing fixtures via `tests/common/mod.rs`. Plus unit tests in `source/stdin.rs` (position encoding, line parsing) and `runtime/config.rs` (`SourceConfig` parsing, duplicate-id rejection). Full workspace: `cargo build`, `cargo test` (201 tests), `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check` all clean.

---

## Non-Goals (explicit)

- **No Kafka / external broker source.** File (`directory`) and `stdin` only — the SQLite / file-backed precedent. Kafka, Postgres-outbox, and HTTP-ingest sources are later phases implementing the same `EventSource` trait.
- **No exactly-once *delivery*.** Conduit provides effectively-once *projection* via idempotent sinks; the source contract is at-least-once. A source may redeliver; Conduit's guards absorb it.
- **No backpressure, rate limiting, or flow control.** The loop polls a bounded batch and projects it; tuning throughput is out of scope (and overlaps the deferred "performance" work).
- **No source-side filtering or transformation.** Routing already drops unmapped event types; upcasting (Phase 10) already transforms payloads. A source hands over raw events only.
- **No distributed / multi-instance coordination.** One projector instance owns its sources' positions. Leader election, partition assignment, and horizontal scale-out are a separate concern.
- **No ordering guarantee across sources**, and no support for one entity's events spanning two sources — a documented producer contract, like Phase 12's sequence contract.
- **`conduit replay` is not removed**, only re-expressed as `run --once` and kept as a deprecated alias for one release.
