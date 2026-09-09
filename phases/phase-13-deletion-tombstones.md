# Phase 13 — Deletion & Tombstones 🎯 (Completed)

**Goal:** An event can *remove* a projected entity. The projection reflects end-of-life, not only creation (Phase 11) and mutation (Phase 12). Deletion is sequence-gated like an upsert, so a delete and a late update converge to one deterministic state regardless of delivery order.

**Why:** after Phase 12 the engine does Create + Update but not Delete.
- Read models that represent a lifecycle — active orders, open sessions, current subscriptions, cart contents, any "active X" / "open Y" projection — have no way to express "gone." `OrderCancelled` cannot remove a row from an active-orders projection.
- `StorageKind::KeyValue` already routes (`routing.rs:17`), and the test fixture routes `CacheInvalidated → kv-cache` (`tests/fixtures/routing.json`). Cache invalidation *is* a delete; there is no delete verb for the future cache adapter (Phase 14) to implement against.
- The only removal pattern available today is projecting a soft-deleted flag as an ordinary column via `on_existing: replace`. That works but pushes filtering onto every consumer, leaves the document file sitting on disk, and gives a cache entry no way to expire.

---

## Phase 13.1 — Delete operation declaration 🏷️ (Completed)

**Scope**
- New field on `SqlMapping` and `DocumentMapping`: `operation`, `#[serde(default)]`, `#[serde(rename_all = "snake_case")]`:

  ```rust
  enum Operation { #[default] Upsert, Delete }
  ```

  - `upsert` — Phase 11 / 12 behavior, sub-governed by `on_existing`. Default, so every existing mapping is unchanged.
  - `delete` — remove the entity identified by the resolved key.
- A `delete` mapping resolves **only the entity key** — SQL: the path expressions for the `primary_key` column(s); document: the `id` path. It has no projection body.
  - SQL: `columns` may contain only the key column(s); any other column → `ValidationError` (no silently-ignored config, per *no hidden defaults*).
  - Document: `document` must be absent or empty → `ValidationError`.
- `AdapterOutcome` (Phase 12.1) gains `Deleted`. `SkipReason` gains `AlreadyDeleted`.
- The Phase 12.2 decision function is extended:

  ```rust
  fn decide(op: Operation, mode: OnExisting, exists: bool,
            stored: Option<GuardState>, event_seq: u64) -> WriteDecision
  // GuardState = { last_sequence: u64, deleted: bool }
  // WriteDecision = Insert | Update | Delete | SkipIdempotent | SkipStale | SkipAlreadyDeleted
  ```

| op | live row? | tombstone? | `event_seq` vs `last_sequence` | decision |
|---|---|---|---|---|
| `delete` | — | none | — | `Delete` (writes row removal + tombstone) |
| `delete` | yes | — | `>` stored | `Delete` |
| `delete` | — | yes | `>` stored | `SkipAlreadyDeleted` (bump `last_sequence`) |
| `delete` | — | — | `<=` stored | `SkipStale` |
| `upsert` | — | yes | `>` stored | `Insert` (resurrection — see 13.4) |
| `upsert` | — | yes | `<=` stored | `SkipStale` |

**Guarantees**
- `operation` and `on_existing` are orthogonal; `on_existing` is read only when `operation: upsert`.
- `decide` stays total and side-effect free.

---

## Phase 13.2 — SQL delete & tombstone 🗄️ (Completed)

**Scope**
- `conduit_projection_state` gains `deleted INTEGER NOT NULL DEFAULT 0`. The guard row is a **tombstone** when `deleted = 1` — it is never removed, so a later stale event (a replayed older `create`, a re-delivered delete) is still gated.
- `SqliteAdapter::handle()` for `operation: delete`, inside the existing transaction:
  - Read the guard state for `(table, entity_key)`.
  - `decide(…)`. `SkipStale` / `SkipAlreadyDeleted` → matching `Skipped(…)`, commit no-op (bump `last_sequence` for `SkipAlreadyDeleted`).
  - `Delete` → `DELETE FROM <table> WHERE <pk col> = ? [AND <pk col> = ? …]` (composite keys from Phase 11.1), then upsert the guard row with `deleted = 1, last_sequence = ?, last_event_id = ?`.
  - A delete for an entity with no live row still writes the tombstone — this is what makes delete-before-create converge (the later create is rejected as stale).
- `SqliteAdapter` declares `AdapterCapability::Delete`.

**Guarantees**
- Outcome `Deleted` whenever a tombstone is written, whether or not a live row existed.
- Redelivered delete → `Skipped(AlreadyDeleted)`; table and guard byte-identical.
- Delete-before-create: `DELETE` at sequence M writes a tombstone; the `create` at sequence < M is later rejected `SkipStale`; entity stays absent.

---

## Phase 13.3 — Document delete & tombstone 📄 (Completed)

**Scope**
- The Phase 12.4 sidecar gains `"deleted": bool`. Never removed once written.
- `FileDocumentAdapter::handle()` for `operation: delete`:
  - Read + parse the sidecar → guard state.
  - `decide(…)`. Skips as in 13.2.
  - `Delete` → remove the entity file if present, then rewrite the sidecar with `deleted: true, last_sequence: M` via atomic rename.
- `FileDocumentAdapter` declares `AdapterCapability::Delete`.
- Single-writer caveat from Phase 12.4 carries over unchanged.

**Prerequisite — collection-keyed output path.** The Phase 11 document path is `{root}/{event_type}/{entity_id}.json` (`file.rs:156`, keyed by `event.event_type`). A delete event has a *different* `event_type` than the create (`OrderCancelled` vs `OrderCreated`), so an `event_type`-keyed path points the delete at the wrong directory. The output path must be keyed by the mapping's stable target identity — `DocumentMapping.collection` — instead: `{root}/{collection}/{entity_id}.json`. **This same bug already affects Phase 12 document upsert** (an update event with its own `event_type` would write a new file rather than overwrite), so the path change ideally lands in Phase 12.4 and Phase 13 assumes it. It is a behavior change to Phase 11 output — call it out in release notes, as Phase 11 did for its own path change.

**Guarantees** — same as 13.2, under a single writer.

---

## Phase 13.4 — Resurrection & permanent tombstones ♻️ (Completed)

**Scope**
- **Default: resurrection.** An `upsert` event whose `sequence` exceeds a tombstone's `last_sequence` re-inserts the entity (`deleted → 0`, `last_sequence` bumped). Rationale: the projection must reflect the highest-sequence truth for an entity; if a `UserReactivated` at sequence M follows a `UserDeleted` at N < M, the entity legitimately exists again. Determinism holds — replay in any order, the highest-sequence event per entity wins, delete or not.
- **Opt-in permanence.** A `delete` mapping may set `permanent: true` (`#[serde(default)]`). Its tombstone rejects **all** later events for that key — `Skipped(Tombstoned)`, `SkipReason::Tombstoned`. For legal erasure (GDPR right-to-be-forgotten) where a replayed or late event must never bring the entity back.

**Guarantees**
- Without `permanent`, delete is just another sequence-gated state — symmetric with upsert.
- With `permanent`, the tombstone is terminal; no event, replay, or sequence value resurrects the key.

---

## Phase 13.5 — Capability-safe validation ✅ (Completed)

**Scope**
- `operation: delete` implies `requires_capabilities: [delete]` (Phase 6.4 / Phase 8 machinery).
- `runtime/validation.rs` rejects at startup any config routing a `delete` mapping to an adapter not declaring `AdapterCapability::Delete` — new `ValidationError` variant, mirroring the Phase 12.5 upsert check.

**Guarantees**
- A `delete` mapping on a non-delete adapter fails validation, not at runtime.
- Both built-in adapters declare `Delete` after 13.2 / 13.3.

**Documented, not enforced:** a delete event's route (the adapters `OrderCancelled` maps to) should cover the same adapters as its create event's route, or the entity is removed from some stores and left in others. Conduit routes by event type and sees one event at a time, so it cannot verify this — same class of producer-side contract as Phase 12's sequence numbering.

---

## Phase 13.6 — Determinism & verification suite 🧪 (Completed)

New integration tests in `crates/conduit-core/tests/`, fixtures under `tests/fixtures/`:

| Test | Asserts |
|---|---|
| `delete_full_lifecycle.rs` | `[create s1, update s2, delete s3]` (SQL and document) → entity absent; guard is a tombstone at sequence 3. |
| `delete_before_create.rs` | `delete s2` then `create s1` → entity absent (the create is `SkipStale`). |
| `delete_shuffled_stream.rs` | The lifecycle stream fed in several orders → identical final state (absent) each time. |
| `delete_redelivery.rs` | An exact redelivery (same `sequence`) → `Skipped(StaleSequence)`, per the 13.1 decision table's `<= stored` row (same "redelivery" contract as Phase 12.3's upsert case); table/file/guard unchanged. A *different*, newer-sequence delete on an already-tombstoned entity → `Skipped(AlreadyDeleted)`, `last_sequence` bumped. |
| `resurrection.rs` | `[create s1, delete s2, recreate s3]` → entity present with sequence-3 state; `recreate s0` after the delete → `SkipStale`. |
| `permanent_tombstone.rs` | `delete permanent s2`, then `upsert s5` → `Skipped(Tombstoned)`; entity stays absent. |
| `delete_composite_key.rs` | Bridge-table row removed by `primary_key: [left, right]`; a different pair is untouched. |
| `delete_capability_rejected.rs` | Config routing a `delete` mapping to a non-delete adapter → validation error at startup. |
| `delete_cross_adapter.rs` | The same lifecycle stream into SQL and document → both end absent, tombstones agree. |

**Guarantee under test:** for a given entity, the final projected state (present with some value, or absent) is a pure function of the *set* of delivered events — the highest-`sequence` event decides, `delete` and `upsert` symmetrically — independent of delivery order, duplicates, and batching. `permanent: true` is the sole exception and is terminal.

---

## Non-Goals (explicit)

- **No cascade delete.** Removing a parent entity does not remove children. Conduit does not model relational ownership — `foreign_keys` remains out of scope (Phase 11 non-goal stands). A domain that needs cascading emits a delete event per affected entity.
- **No bulk / predicate delete.** Every `delete` targets exactly one resolved entity key. `DELETE FROM t WHERE status = 'stale'` is not expressible and will not be.
- **No explicit undelete / restore API.** Resurrection happens only through a higher-sequence `upsert` event (13.4). There is no out-of-band "restore entity X" operation.
- **No soft-delete column feature.** Projecting a `deleted_at` flag is already possible as an ordinary `on_existing: replace` column; Phase 13 is about *removal*, not about standardizing a flag.
- **No TTL / time-based expiry.** Entities are removed by events, never by a clock. Time-based cache eviction is a Phase 14 (cache adapter) concern.
- **No tombstone compaction.** `conduit_projection_state` tombstone rows accumulate indefinitely; garbage-collecting them (safe only once no replayable event predates the tombstone) is deferred.
- **Delete-route ⊇ create-route is not enforced** (see 13.5).
