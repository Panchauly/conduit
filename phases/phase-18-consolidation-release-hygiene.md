# Phase 18 — Consolidation & Release Hygiene 🎯 (Completed)

**Goal:** Pay down the debt that accumulated across Phases 11–17 — dead code, stale docs, a missing changelog, an unenforced-contract pile — and pin the project's scope in writing. **No behavior changes, no new features.** Every sub-phase is delete / document / configure.

**Why:** seven feature phases shipped in a row. `cargo` warns on every invocation, `CLAUDE.md` and `architecture.md` describe a CLI and an adapter set that no longer exist, there is no `RELEASES/` entry since `v0.5.0-alpha` despite several behavior changes, and the producer-side assumptions Conduit *depends on but cannot check* are scattered across six phase files. The scope debate in the Phase 17 thread also needs a permanent home.

---

## Phase 18.1 — Delete dead validation modules 🧹 (Completed)

**Scope**
- `crates/conduit-core/src/adapter/sql/validate.rs` (51 lines) and `crates/conduit-core/src/adapter/document/validate.rs` (83 lines) plus their `pub mod validate;` lines in the respective `mod.rs`.
- These predate Phase 8. Live validation is `runtime/validation.rs`; nothing outside these two files calls their public functions (`keyvalue/` and `graph/` were correctly built with **no** `validate.rs` — Phases 14.4 / 16.5 put their checks in `runtime/validation.rs`). Deleting them makes all four adapters consistent.
- Confirm-then-delete: `cargo build --workspace && cargo test --workspace` green after removal.

**Guarantees**
- Validation behaviour is byte-identical — the deleted code was unreachable.

---

## Phase 18.2 — Workspace manifest 🔧 (Completed)

**Scope**
- Add `resolver = "3"` to the `[workspace]` table in the root `Cargo.toml` (members are edition 2024). Removes the `virtual workspace defaulting to resolver = "1"` warning printed on every `cargo build` / `clippy` / `test`.
- Verify no dependency resolution changes (`cargo tree` diff, `Cargo.lock` unchanged or reviewed).

**Guarantees**
- Build output is warning-free; resolver behaviour matches what edition 2024 already implies.

---

## Phase 18.3 — Line-ending normalization 📐 (Completed)

**Scope**
- Add `.gitattributes`: `* text=auto eol=lf`, `*.rs text eol=lf`, plus `*.json` / `*.md` / `*.yaml`. Binary fixtures (if any) marked `binary`.
- One renormalization commit (`git add --renormalize .`) so the "LF will be replaced by CRLF" warnings stop and diffs stay clean on Windows.

**Guarantees**
- No source content change — only recorded line endings.

**As built**
- `.gitattributes` added (`* text=auto eol=lf` + explicit `eol=lf` for `.rs` / `.toml` / `.lock` / `.json` / `.jsonl` / `.ndjson` / `.md` / `.yaml` / `.yml`). No binary fixtures exist.
- `git add --renormalize .` reported **nothing to change** — the tracked blobs were already LF; the per-commit "LF will be replaced by CRLF" messages were checkout-side (`core.autocrlf`), which `.gitattributes eol=lf` now settles.

---

## Phase 18.4 — Doc refresh 📄 (Completed)

**Scope**
- **`CLAUDE.md`:**
  - `cargo run -p conduit-cli -- validate` → there is no `validate` subcommand. Actual: `run`, `sources`, `explain`, `dry-run`, `replay`. Fix the "Common Developer Commands" block.
  - "Currently Only Two of Four Advertised Storage Kinds Exist" and any SQL/document-only framing → all four kinds exist (Phases 14, 16).
  - Note the shared `decide()` core (`adapter/mod.rs`) and the `AdapterOutcome` / `SkipReason` model (Phase 12.1).
- **`architecture.md`:**
  - Collapse the duplicated `## 3. High-Level Components` heading.
  - §3.1 CLI: `validate` / `test-event` commands don't exist — replace with the real list.
  - §3.2 Routing: `fn route(event: &Event) -> Vec<StorageKind>` is not the real signature (`route` returns `Vec<AdapterId>` via the routing table) — correct it.
  - Storage-model mentions → SQL, Document, Key-Value, Graph.
  - Add the **Scope boundary** section (18.7 — landed early, separately).
- **`README.md`:** align the "SQL, document, cache, graph" pitch with what's now implemented; state the projection-only scope (see 18.7).
- **`phases.md`:** "Beyond Phase 17" — reconcile with the projection-only decision (no event-store substrate; sources are consume-an-existing-log).

**Guarantees**
- Docs describe the code that exists on `master` at the time of the commit.

**As built**
- `architecture.md`: duplicate `## 3. High-Level Components` heading removed; §3.1 CLI list, §3.2 routing signature (`route_with_rules → Vec<AdapterId>`), §3.3 dispatch (dependency order + `FailurePolicy`), §4 (all four adapters + a `decide()` / `AdapterOutcome` paragraph), §6 (entity-keyed sequence-gated model + effectively-once), §7 (`AdapterOutcome` replaces `success: bool`), §8 (Phase 17's bounded across-batch retry noted as the one deliberate exception), §10 status all corrected.
- `README.md`: pitch aligned to the four implemented adapters; projection-only scope stated; links to `architecture.md` / `phases.md` / `producer-contract.md` / `RELEASES/v0.6.0.md`.
- `phases.md`: "Beyond Phase 18" reconciled with projection-only.
- **`CLAUDE.md` lives at `C:\Projects\CLAUDE.md` — outside the `conduit/` repo — so its rewrite is not part of this PR.** It was corrected in place: the stale `tokio::spawn` async-dispatch claim, the `validate` subcommand, the two-adapter framing, the `execute.rs`/`result.rs` files that don't exist, and the run-together formatting.

---

## Phase 18.5 — `RELEASES/v0.6.0.md` 🏷️ (Completed)

**Scope**
- Changelog covering Phases 11–17, with **behaviour changes called out** for anyone upgrading from `v0.5.0-alpha`:
  - Idempotency is now entity-keyed, not `event_id`-keyed (Phase 11). The Phase 4 `conduit_events` guard table is superseded by `conduit_projection_state`.
  - SQL writes use `INSERT … ON CONFLICT … DO UPDATE` for `on_existing: replace` (Phase 12).
  - Guard-table schema gained `deleted` / `permanent` (Phase 13) and a `facet` PK column (Phase 15) — **a pre-existing guard table must be dropped and rebuilt by replay** (no in-place migration; it's a derived cache).
  - Document output path re-keyed from `{event_type}/` to `{collection}/` (Phase 13.3) — a behaviour change to Phase 3/4 output.
  - `ExecutionReport` / `AdapterExecutionReport` JSON shape changed: `AdapterOutcome` enum replaced `success: bool` + skip-message (Phase 12.1); `source_id` / `source_position` added (Phase 17).
  - New `sources` config block; `conduit run` is now a continuous poll → dispatch → commit loop with `--once` (Phase 17).
- Note `Event.sequence` is required (since Phase 11) and its role in gating + ordering.

**Guarantees**
- The release note is sufficient to upgrade a `v0.5.0-alpha` deployment without surprises.

---

## Phase 18.6 — `phases/producer-contract.md` 📑 (Completed)

**Scope** — written ahead of the rest of Phase 18, alongside 18.7, since the scope thread needed it:
- One document consolidating every assumption Conduit **depends on but cannot verify**, currently scattered across Phases 11–17:
  - `Event.sequence` is monotonically increasing for successive events of one entity (per-aggregate version *or* global log offset); gaps allowed; not the timestamp; unrelated to `version`.
  - The entity id resolves to a stable, canonically-formatted scalar (same string every time — no `{uuid}` vs `uuid` drift).
  - Each entity's events are delivered in sequence order (the batch sort tolerates local reordering; it does not tolerate permanent drops or a lower sequence arriving after its lane advanced under `on_existing: ignore`).
  - One entity's events do not span two sources.
  - A `delete` event's route covers the same adapters as its `create` event's route.
  - Producers emit whole-entity state (fat events), not deltas.
- For each: what breaks if violated, and why Conduit can't check it (sees one event at a time).

**Guarantees**
- The engine/producer boundary is stated in one place instead of six.

---

## Phase 18.7 — Scope boundary in `architecture.md` 🧭 (Completed)

**Scope** — added ahead of the rest of Phase 18, since the Phase 17 thread settled it:
- A top-level section stating the three-way ownership split:

  | Owner | Responsibilities |
  |---|---|
  | **Application / command side** | business logic, command validation, state transitions, emitting events (synchronously invoking Conduit as a library, or pushing to a socket / broker) |
  | **Transport / external world** | durability, wire protocols, network transport, consumer offsets (behind the `EventSource` trait) |
  | **Conduit** | resolve where an event goes (routing), verify it is safe to write (validation), order execution deterministically (dependency graph), project it into target storage models without corrupting state (`decide()` + guards) |

- Record: **Conduit is projection-only.** It does not produce, ingest, buffer, or store events. An event-store substrate was considered and rejected (see the Phase 17 thread) — producing the log is out of scope.

**Guarantees**
- The "what is Conduit" question has a one-screen answer.

---

## Phase 18.8 — Phase 17 offset correction ✏️ (Completed)

**Scope**
- Amend `phases/phase-17-event-sources.md` (17.2): Conduit's checkpoint file is a **fallback for sources with no offset store of their own** (a plain directory, a pipe). A source backed by a system that owns offsets — a Kafka consumer group, a log with its own cursor — uses *that*, per the 18.7 scope boundary ("consumer offsets" belong to Transport). No code change in this phase; `directory` / `stdin` keep the checkpoint file. It is guidance for the future Kafka / log-table source.

**Guarantees**
- Doc-only; the Phase 17 implementation is unchanged.

---

## Non-Goals (explicit)

- **No behaviour changes.** If deleting `validate.rs` or setting the resolver changes a single test outcome, stop and investigate — it means something wasn't actually dead.
- **No new features, adapters, or sources.**
- **No full `architecture.md` rewrite** — fix the stale specifics (18.4) and add the boundary (18.7); a from-scratch rewrite is its own effort.
- **No guard-table migration tooling.** The `RELEASES` note documents "drop and rebuild by replay"; automating it is out of scope.
