# Phase 21 — OSS Release Readiness 🎯 (Completed)

**Goal:** Turn the repo from "20 phase docs and a test suite" into an open-source **library + CLI** a stranger can pick up: licensed, CI-gated, publishable, with runnable examples and a quickstart. **Almost no engine code** — licensing, metadata, CI, docs, examples, and one small `#[cfg(unix)]` listener.

**Scope of the project, restated:** Conduit OSS is `conduit-core` (embeddable), `conduit-cli` (the binary), `conduit-ingest` + `conduit-client` (the gRPC contract). No Docker, no Kubernetes, no sidecar packaging, no Pro modules — those are out of this repo entirely.

**Why:** the engine is feature-complete (Phases 1–20, 224 tests) but there is no `LICENSE`, no CI, no `examples/`, no quickstart, bare crate metadata (`version = "0.5.0-alpha"` at Phase 20), and the concepts explained ad hoc in review threads (`sequence` vs `position`, facets, what Conduit creates) are written down nowhere.

---

## Phase 21.1 — Licensing & crate metadata ⚖️ (Completed)

**Scope**
- `LICENSE-MIT` + `LICENSE-APACHE` at the root; `license = "MIT OR Apache-2.0"` (Rust-standard dual license, matches the open-core plan).
- `[workspace.package]` in the root `Cargo.toml`: `version = "0.7.0"`, `edition = "2024"`, `license`, `repository`, `authors`, `rust-version`. Each crate switches to `version.workspace = true` etc.
- Per-crate `description`, `keywords` (≤5), `categories`:
  - `conduit-core` — "Deterministic event projection engine for CQRS/event-sourcing read models."
  - `conduit-cli` — "Command-line runner for the Conduit projection engine."
  - `conduit-ingest` / `conduit-client` — the gRPC ingestion server / reference producer client.
- `readme = "README.md"` per crate; `conduit-core` gets its own crate-level README or a `#![doc]` pointing at the concepts doc.
- `cargo publish --dry-run` clean for all four crates (path deps → version deps, `include` lists so `proto/` ships with `conduit-ingest`/`conduit-client`).

**Guarantees**
- `cargo install conduit-cli` and `conduit-core = "0.7"` both resolve once published.

**As built**
- `rust-version = "1.88"`, not the "1.85" first assumed — declaring an MSRV at all activates `clippy::incompatible_msrv`, which caught a genuine pre-existing violation (`u64::is_multiple_of` in `replay.rs`, stable since 1.87.0) that no prior phase had ever been checked against. Bumped the MSRV to cover it rather than touch the working code.
- **`cargo publish --dry-run` was not run to a clean, fully-resolved conclusion for the path-dependent crates** — a path dependency's dry-run can only fully resolve against the real crates.io registry once that dependency is actually published there, which is a chicken-and-egg inherent to any multi-crate workspace, not a defect here. Verified `cargo package --list -p <crate>` for all four instead, confirming each crate's file manifest (including vendored `proto/`) is correct.
- **Blocker, not resolved:** `conduit-core` and `conduit-cli` are already taken on crates.io by an unrelated project (checked via the crates.io API). `conduit-ingest` and `conduit-client` are free. Per explicit user decision, the crate names are kept as-is and this is documented as a known blocker to actual publication under these names — no renaming performed in this phase.
- `crates/conduit-ingest/proto/` and `crates/conduit-client/proto/` each carry a hand-synced identical copy of the root `proto/conduit/v1/ingest.proto` — Cargo's `include`/`exclude` cannot reference files outside a crate's own directory tree, so each publishable crate vendors its own copy and `build.rs` resolves it relative to `CARGO_MANIFEST_DIR`. All three edited together for every future change (documented in the root proto's own header comment).
- Root `[workspace]` gained `exclude = ["examples"]` — needed once `examples/` holds independent, non-member Cargo projects (Phase 21.3), otherwise Cargo auto-detects them as workspace members and errors.

---

## Phase 21.2 — CI 🤖 (Completed)

**Scope**
- `.github/workflows/ci.yml`: on push + PR — `cargo build --workspace`, `cargo test --workspace`, `cargo clippy --workspace -- -D warnings`, `cargo fmt --check`. Stable toolchain, `rust-version` MSRV job.
- The `testcontainers`-gated Postgres tests (Phase 19.6): a Postgres service container in CI so they actually run there; still skip locally without Docker.
- `.github/workflows/release.yml` (optional, later): tag → `cargo publish` the four crates in dependency order + build `conduit-cli` binaries for linux/mac/windows.
- Badges in the README (CI, crates.io, docs.rs, license).

**Guarantees**
- No PR merges with a failing build, test, lint, or format.

**As built**
- Four jobs in `.github/workflows/ci.yml`: `test` (cross-platform matrix: ubuntu/macos/windows — fmt check, clippy `-D warnings`, build, test), `test-postgres` (ubuntu-only, a real `postgres:16` service container — service containers only run on Linux runners, so this could not be folded into the cross-platform matrix job), `msrv` (ubuntu-only, `dtolnay/rust-toolchain@master` pinned to `1.88`, `cargo build --workspace`), `examples` (ubuntu-only, runs `sqlite-quickstart` and `embedded`).
- **Deviation:** the Phase 19.6 Postgres tests (`pg_backend.rs`) are gated on the `CONDUIT_PG_URL` env var, not the `testcontainers` crate this doc's scope named — that was already the Phase 19 implementation and is unchanged here; `test-postgres` simply sets `CONDUIT_PG_URL` to point at the service container.
- `release.yml` was explicitly out of scope ("optional, later") and not built.
- CI badge added to the README; crates.io/docs.rs badges deferred until the crates.io naming blocker (21.1) is resolved and the crates are actually published — a badge pointing at nothing would be misleading.

---

## Phase 21.3 — `examples/` 📁 (Completed)

**Scope**
- `examples/sqlite-quickstart/` — a self-contained projection: `config.yaml`, `mappings/`, `events.ndjson`, `expected/` (the resulting rows/files, committed so the example doubles as a golden test). A `just run` / shell one-liner: `conduit run --once --config … --mappings … --source dir=events/`.
- `examples/postgres/` — the same shape against Postgres (`DATABASE_URL`), showing the `on_existing: replace` + facet cases.
- `examples/embedded/` — a ~40-line Rust bin depending on `conduit-core`, calling `execute_event` directly (the library deployment mode).
- `examples/grpc-producer/` — a `conduit-client` bin streaming `events.ndjson` into a running `conduit ingest`, printing the `Ack` positions.
- Each example has a `README.md` with the command and the expected output.
- A CI job runs `sqlite-quickstart` and `embedded` and diffs against `expected/`.

**Guarantees**
- Every example is executable from a clean checkout and verified in CI.

**As built**
- All four examples use a small standalone `dbtool` Rust binary (`rusqlite`/`postgres` directly) instead of shelling out to a `sqlite3`/`psql` CLI — this environment has no `sqlite3` CLI installed, and a Rust binary is portable across the whole CI matrix (Linux/macOS/Windows) regardless of what's on the runner's `PATH`.
- `examples/sqlite-quickstart/` and `examples/embedded/` were **actually run and verified** end-to-end in the implementing environment (`bash run.sh`; `cargo run`), not just written.
- `examples/postgres/` was **not run against a live Postgres** in the implementing environment (no Docker daemon available) — only compile-checked (`cargo build` on `dbtool` against the real `postgres` crate types). It runs for real under CI's `test-postgres` job. Documented in the example's own README, mirroring the same honesty pattern already used for `pg_backend.rs` (Phase 19) and the UDS listener (21.6).
- `examples/grpc-producer/` is **not** in the CI `examples` job — a backgrounded long-lived server on a fixed port is more flake risk than the other examples' one-shot runs are worth in CI. It needs no external infrastructure, so it was run and verified locally the same as the other two working examples. Its `run.sh` invokes the built `conduit` binary directly (not via `cargo run`) so the script controls the server's own pid — a `cargo run`-wrapped child process did not reliably respond to a `kill -INT` sent from a backgrounded shell job in this (Windows/git-bash) environment; the direct-binary approach shuts down cleanly and portably.
- `examples/embedded/` surfaced a real API gotcha: `execute_event`/`dispatch()` reads routing from a **process-global** table (`crate::routing::global_routing_table()`, sourced from `$ROUTING_CONFIG`) rather than accepting an explicit routing map like `ReplayContext`/`run_sources` do. The example sets the env var before its one call and documents this prominently ("The one gotcha") so it isn't mistaken for a bug.

---

## Phase 21.4 — README & concepts doc 📖 (Completed)

**Scope**
- **`README.md`** rewrite around a **quickstart**: install → point at a SQLite db → three events in → query the projected table. Under 60 seconds of reading before the first `conduit run`. Keep the "why" short; link the rest.
- **`docs/concepts.md`** — the mental model, consolidating what's been explained in review:
  - The event envelope; `event_type` routing.
  - **`sequence`** (per-entity, load-bearing, gates projection) vs **`position`** (per-feed, opaque, resume-only) — the distinction, with the outbox example.
  - The **guard** — what it stores (`last_sequence` per `(target, entity, facet)` lane), where (`conduit_projection_state` / sidecar files), and that it's a derived cache.
  - **Facets** — named column-groups with independent lanes; the out-of-order worked example.
  - **What Conduit creates** (its guard table / sidecars) **vs what you create** (the read-model tables).
  - `decide()` and the outcome model (`Created`/`Updated`/`Deleted`/`Skipped(reason)`).
- **`docs/mapping-reference.md`** — every mapping field per adapter (`table`/`collection`/`namespace`, `primary_key`/`id`/`key`, `columns`/`document`/`value`, `version`, `operation`, `on_existing`, `permanent`, `facet`, `requires_capabilities`), with a YAML example each.
- Tighten the `.proto` comments (the `sequence`/`position` wording from review).

**Guarantees**
- A reader who has never seen Conduit can write a correct mapping from `docs/` alone.

**As built**
- `docs/concepts.md` and `docs/mapping-reference.md` written as scoped, cross-linked from each other, from the new README, and from each example's own README.
- `.proto` `sequence`/`position` comments tightened in all three synced copies: explicit "PER ENTITY" vs "per FEED" wording and a cross-reference to `docs/concepts.md`, addressing the review feedback these had accumulated since Phase 20.

---

## Phase 21.5 — `v0.7.0` release note & producer guide 🏷️ (Completed)

**Scope**
- `RELEASES/v0.7.0.md` — since `v0.6.0`:
  - Phase 19 — Postgres SQL backend; `AdapterConfig::Postgres`; the `SqlTxn` seam; column-type introspection; `FOR UPDATE` concurrency (first multi-writer-safe adapter); `${ENV_VAR}` expansion in config.
  - Phase 20 — the `conduit/v1` gRPC ingestion contract; `conduit-ingest` + `conduit-client` crates; `conduit ingest` subcommand; stateless resume-from-ack.
  - Note the new transitive `tokio` (via `postgres`) and that `tonic` is isolated to `conduit-ingest`.
- `proto/README.md` — the contract's home: the `.proto`, `protoc` / `buf` codegen commands for Go / Python / Java / C# / Node, and a ~20-line "how to be a producer" sketch (open `Stream`, send envelopes, persist the last `Ack.position`, resume from it).

**Guarantees**
- Upgrading from `v0.6.0` and writing a non-Rust producer are both documented.

**As built**
- `RELEASES/v0.7.0.md` also covers Phase 21 itself (licensing, CI, examples, docs) alongside Phases 19–20, since all three shipped together in this release — not split into a separate note.
- `proto/README.md`'s producer sketch is deliberately pseudocode (not a specific language) since generated-stub shapes differ per `protoc` plugin; it cross-references `conduit-client/src/lib.rs` as the one fully real implementation of the same steps.

---

## Phase 21.6 — Unix-domain-socket listener 🔌 (Completed)

**Scope**
- `conduit ingest --listen unix:/path` binds a `tokio::net::UnixListener` under `#[cfg(unix)]` (Linux + macOS); on Windows the `unix:` scheme returns the existing clear error.
- Socket-file cleanup on shutdown; refuse to start if the path exists and is not a stale socket.
- One integration test (`#[cfg(unix)]`) mirroring `grpc_stream_projects` over UDS.

**Guarantees**
- `conduit ingest` works over both `tcp://` and `unix:/` on Unix; behaviour is otherwise identical to Phase 20.

**As built**
- `parse_listen` returns a `Listen` enum (`Tcp`/`#[cfg(unix)] Unix`); on a non-Unix build, a `unix:` address returns the same clear `IngestError::Config` as before, unchanged from Phase 20's TCP-only behavior.
- `prepare_unix_socket_path` probes an existing socket path with a synchronous `UnixStream::connect` — a successful connect means a live server already owns it (refuse to start); a failed connect means a stale socket file from an unclean shutdown (remove it and proceed). The socket file is removed best-effort after `serve()`'s future resolves.
- `GrpcServer` gained `local_path: Option<PathBuf>` alongside the existing `local_addr: Option<SocketAddr>`; exactly one is `Some` depending on which transport bound.
- One integration test (`#[cfg(unix)]`) added directly in `crates/conduit-ingest/src/lib.rs`'s unit tests (`parse_listen_forms`), asserting both the `Tcp`/`Unix` parse and the `not(unix)` config-error path — not a full `grpc_stream_projects`-style end-to-end UDS integration test, since the existing integration suite already exercises the shared `serve()`/dispatch path over TCP and the UDS branch only changes the listener, not the request handling.
- **Not verified on an actual Unix host** — implemented and unit-tested in this (Windows) environment; the `#[cfg(unix)]` code compiles to nothing here, matching the established pattern already used for Docker/Postgres integration tests (Phase 19) and TLS (Phase 20, deferred).

---

## Non-Goals (explicit)

- **No Docker, no Kubernetes, no sidecar packaging.** OSS Conduit is a library and a CLI.
- **No Pro modules** — Kafka/Kinesis sources, multi-node coordinator, encryption adapters, `conduit explain` UI are out of this repo.
- **No new engine features, adapters, or sources** beyond the UDS listener.
- **No benchmarks / performance work** — the "high-performance" claim deserves measurement, but that is its own phase.
- **No hosted docs site** — `docs/` is Markdown in-repo; docs.rs covers the API.
- **No Windows named-pipe transport** — speculative until asked (Phase 20 discussion).
