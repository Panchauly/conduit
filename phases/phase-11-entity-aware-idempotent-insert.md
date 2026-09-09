# Phase 11 — Entity-Aware Idempotent Insert ✅ (Completed)

**Goal:** A second creation event for an already-created entity is rejected cleanly and deterministically — never a raw driver error, never silent duplication — regardless of `event_id`. Scoped to insert-only projection (no update/upsert semantics; see Non-Goals).

**Why:** current idempotency (Phase 4) dedups on `event_id` only, not on the entity being projected.
- SQL: two different `event_id`s that resolve to the same primary key throw a raw, unhandled PK constraint violation — surfaced as a generic write failure, not a clean skip.
- Document (file adapter): output path is currently `root/{event_type}/{event_id}.json` — keyed by event, not entity. A redelivered creation event with a new `event_id` silently writes a **second file** for the same logical entity. No error, no guard, no detection. This is worse than the SQL case.

---

## Phase 11.1 — Entity Identity Resolution ✅ (Completed)

**Scope**
- **SQL:** `SqlMapping::build()` currently returns `(sql, values)` and discards which value corresponds to `primary_key`. Extend it to also return the resolved primary key value(s).
- **SQL — composite keys:** `primary_key` becomes `OneOrMany<String>` — a single column name (existing behavior, unchanged) or a list of column names, for tables whose real identity is composite (bridge/junction tables, many-to-many association tables). Every listed column must exist in `columns` (extends existing validation Rule D in `validate.rs`, which today only checks a single string).
  - `foreign_keys: HashMap<String, String>` on `SqlMapping` is currently dead — parsed, never read anywhere else in the codebase. Either wire it up as part of this work or remove it; don't leave a schema field that silently does nothing next to a new composite-key field that does.
- **Canonical key encoding:** the guard's stored/compared key cannot be a bare scalar string once composite keys exist. Naive concatenation collides (`("AB","C")` vs `("A","BC")`). Encode as an ordered JSON array of the resolved values in declared column order (`serde_json::to_string(&Vec<Value>)`), single-column keys included, so there is exactly one encoding path, not two.
- **Scalar requirement:** each resolved key value (single or composite) must be a JSON scalar (string/number/bool). `resolve_path` can return any JSON value including nested objects/arrays if a mapping points at the wrong place — reject non-scalar resolved key values at build time, fail fast, don't let a nonsense key reach the guard table.
- **Document:** `DocumentMapping` has no id concept today. Add a required `id` path field (resolved via the same `payload.` / `metadata.` expressions as other fields). Required, not optional — no silent fallback to `event_id` when omitted. A mapping that is genuinely event-scoped (append-only/log-style, no real entity) states that explicitly by pointing `id` at `metadata.event_id` (or wherever event_id lives in payload) — Conduit does not infer this.
- **Document adapter output path change:** `root/{event_type}/{event_id}.json` → `root/{event_type}/{entity_id}.json`. This is a behavior change to existing Phase 3/4 output, not an additive one — call it out in the changelog/release notes.
- **Event Model:** Add `sequence: u64` to `Event` (event.rs), required, no `#[serde(default)]`. Captured now for forward compatibility with a future Upsert phase; **not** used to gate accept/reject in this phase (see Non-Goals).

### No DB-generated (auto-increment/serial) identity

The mapped key — single or composite — must always resolve from the event (`payload.` / `metadata.`), never be assigned by the database. Mechanically this already holds: `SqlMapping::build()` supplies a value for every column it writes, including the key column(s); there's no path for "leave this column out, let the DB default it."

This isn't incidental — it's required by the same reasoning that ruled out passing a SQL-generated id downstream to other adapters (see Non-Goals below). An auto-increment value depends on database history (row count, prior deletes, which replica). Replaying the same event stream into a different starting database state produces different auto-increment values, breaking the Phase 10.4 deterministic-replay guarantee. Producer-minted identity (typically a UUID assigned when the event is created) is what makes replay reproducible and lets every adapter independently derive the same key with zero coordination.

**This is a real, stated cost on the target schema, not just a rule:**

| | |
|---|---|
| **Con (DBMS-level)** | Random UUID/business keys hurt index locality vs. a monotonic auto-increment integer — more page splits, index fragmentation, worse insert/scan performance at scale. Larger storage per key (16 bytes binary / 36 char text vs. 4–8 bytes), and every foreign key referencing it pays the same cost — directly relevant to bridge tables, which are FK-heavy by nature. Loses free chronological ordering (`ORDER BY id` ≈ insertion order) unless a time-ordered scheme like UUIDv7/ULID is deliberately chosen. Any existing table whose only key is a bare `SERIAL`/`AUTOINCREMENT` needs a new column added before Conduit can project into it — a real migration, not a config change. Often means maintaining two identity systems per table: the DB's own serial for internal joins/performance, and a separate key for Conduit's cross-store identity. |
| **Plus (Conduit's requirements)** | It's what makes deterministic replay possible at all (Phase 10.4). It's what lets SQL, document, and future cache/graph adapters independently resolve the *same* key with no coordination between them — the reason cross-adapter id-passing was rejected. It matches standard event-sourcing/CQRS practice: aggregate id minted at the command that creates the aggregate, carried in every event about it. |

Conduit isn't claiming producer-minted keys are generally better than auto-increment — for raw DBMS performance they usually aren't. It requires them because the project's central promise (deterministic, replayable, multi-store projection without adapter coordination) is architecturally incompatible with DB-generated identity. Anyone evaluating Conduit against an existing auto-increment-only schema should know upfront this is a required schema change, not an optional best practice.

## Phase 11.2 — SQL Guard ✅ (Completed)

**Scope**
- New table: `conduit_projection_state(target_table, entity_key, last_sequence, last_event_id, processed_at)`, `PRIMARY KEY (target_table, entity_key)`. `entity_key` is the canonical encoding from 11.1 (single scalar or ordered JSON array for composite keys).
- Before the real `INSERT`: does a row already exist for `(target_table, entity_key)`?
  - Yes → skip cleanly (`AdapterResult::skipped`), never attempt the `INSERT`, never let the DB throw.
  - No → run `INSERT`, then insert the guard row, same transaction, guard recorded **after** the write (same ordering as the existing Phase 4 guard).
- `last_sequence` is stored, not compared. First creation wins.

## Phase 11.3 — Document Guard ✅ (Completed)

**Scope**
- Guard file (or sidecar) keyed by `(event_type, entity_id)`, checked before write.
- Exists → skip.
- Doesn't exist → write projection to the entity-keyed path, then commit the guard via atomic rename (closes the existing check-then-write race in the current `.done` marker approach).

## Phase 11.4 — Verification Suite ✅ (Completed)

- `duplicate_entity_different_event_id.rs` — two distinct `event_id`s resolving to the same entity_id → second is skipped; first write's data is what persists. This is the test that didn't exist before Phase 11 and is the actual point of the phase.
- `duplicate_event_id_replay.rs` — existing Phase 4 behavior, re-verified against the new guard table/path.
- `document_path_keyed_by_entity.rs` — confirms the output path change is by entity, not event.
- `composite_key_bridge_table.rs` — many-to-many association mapping with a two-column `primary_key`; duplicate `(left_id, right_id)` pair across different `event_id`s is skipped; a genuinely different pair is not.
- `non_scalar_key_rejected.rs` — mapping's `id`/`primary_key` path resolves to a JSON object/array → build fails fast, never reaches the guard table.

---

## Non-Goals (explicit)

- **No Upsert / CAS.** There is currently no `UPDATE` or `ON CONFLICT` path anywhere in the SQL adapter (`SqlMapping::build()` only ever emits `INSERT`). A stale-sequence-overwrite guard (compare-and-swap) protects a race that can't happen without update semantics existing first. `sequence` is captured in this phase so the schema doesn't need to change when Upsert lands later, but it does not drive accept/reject logic here — that would be guarding against a scenario the system can't produce yet.
- **No cross-adapter identity passing.** Considered and rejected: having the SQL adapter mint/generate an entity identity (e.g. DB auto-increment) and pass it downstream to document/cache adapters.
  - **Breaks determinism:** Phase 10.4 guarantees replay is 100% reproducible. A DB-generated id can differ between runs (fresh database, cleaned rows, etc.), so downstream adapters would diverge from the original run on replay.
  - **Doesn't fit the existing execution model:** Phase 9's dependency graph (`depends_on`, topological sort) controls *order* only — `AdapterExecutionMeta` is `{priority, depends_on}`, no data channel between adapters. Threading a generated value between adapters is a new architectural surface, not a small addition, and it makes independently-designed adapters hierarchically dependent on one another.
  - **Correct fix instead:** entity identity is resolved deterministically from the event itself (`payload.` / `metadata.` path), exactly like every other mapped field. Every adapter — SQL, document, future cache — independently resolves the *same* entity_id from the *same* event. If a producer doesn't yet have a stable id to put in the event, that's an upstream problem (event producers should mint ids, e.g. UUIDs, at creation time) — it does not belong inside Conduit.
- **No public adapter-authoring contract.** Not opening the project publicly in the near term, so `StorageAdapter` trait stability for third-party contributors is out of scope for this phase.
- **No composite-key support beyond identity.** Composite `primary_key` (11.1) only affects the entity guard key. It does not add composite foreign keys, multi-column joins, or any other relational feature — `foreign_keys` remains out of scope unless explicitly picked up as part of 11.1's cleanup.
