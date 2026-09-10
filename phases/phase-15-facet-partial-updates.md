# Phase 15 — Facet Partial Updates 🎯 (Completed)

**Goal:** An event can update a *named subset* of an entity's projected state — one facet — leaving the rest untouched, and two facets of the same entity can be updated by independent event streams that arrive in any order without one falsely superseding the other.

**Why:** Phase 12 `on_existing: replace` overwrites the *whole* row / document / value. A `UserLoggedIn` event carrying only `user_id + timestamp` cannot update just `last_login` on the `users` row — it would have to carry `email`, `display_name`, and every other column, or not touch the row at all. This is still producer-computed data (Phase 15 is **not** deltas — see Non-Goals); it is just narrower than a full row.

The blocker Phase 12 named: one `conduit_projection_state` row per `(target_table, entity_key)`, one `last_sequence`. Two partial updates to disjoint columns delivered out of order —

```
UserEmailChanged  seq 5   sets email
UserLoggedIn      seq 6   sets last_login
```

— gate against the same `last_sequence`: `seq 6` applies and stores `6`; the later-arriving `seq 5` is judged `SkipStale` and the email change is silently lost. Phase 15 replaces the single per-entity lane with **one lane per facet**.

**When you do *not* need this:** if a narrow event would be the entity's only non-key content, project it into its own table / collection / namespace with a full-row `on_existing: replace` mapping (`user_logins(user_id PK, last_login)`). That needs zero new features and covers most "partial update" cases. Facets are for the residual: several independently-maintained facets that genuinely must live in **one** row.

---

## Phase 15.1 — Facet declaration & ownership validation 🧩 (Completed)

**Scope**
- New optional field on `SqlMapping`, `DocumentMapping`, `KvMapping`: `facet: Option<String>`, `#[serde(default)]`.
  - `None` → the **default facet** (the whole entity — every mapping today is this, unchanged).
  - `Some("contact")` → a named facet: this mapping writes only its own columns / keys and gates on its own lane.
- A **faceted entity** is a `(table | collection | namespace)` that has at least one mapping with a named facet.
- Validation (`runtime/validation.rs`, new `ValidationError` variants):
  - On a faceted entity, no mapping may use `on_existing: replace` — full-replace and facets are mutually exclusive (a full replace at sequence N would clobber facet columns whose lanes are at a higher sequence). The create mapping for a faceted entity must be `operation: upsert, on_existing: ignore` (insert once, never full-overwrite).
  - Every named facet's column / key set must be **pairwise disjoint**, and disjoint from the create mapping's non-key columns. Two lanes claiming `email` would race their independent sequence gates.
  - A named facet's key columns must match the entity's `primary_key` / `id` (it identifies the same entity).
  - `operation: delete` is only valid on the default facet (you delete an entity, not a facet — see 15.3).

**Guarantees**
- Zero change for every existing mapping — absent `facet` is the default facet, and nothing about the default lane's behaviour changes.
- Facet vocabulary is the *only* addition to the mapping schema; `operation` / `on_existing` / `permanent` / `version` keep their Phase 11–14 meanings.

**As built**
- `facet: Option<String>` added to `SqlMapping` / `DocumentMapping` / `KvMapping` (`#[serde(default)]`); `facet_key()` normalizes `None → ""`. Projections (`SqlProjection` / `DocumentProjection` / `KvProjection`) carry the normalized `facet: String`.
- Validation added to `runtime/validation.rs` (`validate_facets`, grouping mappings by table / collection / namespace): `FacetReplaceConflict`, `FacetDeleteNotDefault`, `FacetHasNoFields`, `FacetFieldOverlap`, `FacetKeyMismatch`.
- **Implicit facet write mode.** 15.1 says "no mapping may use `on_existing: replace`" on a faceted entity, but a facet update still has to be sequence-gated rather than idempotent-skipped. Resolved by having the adapter pass `OnExisting::Replace` to `decide()` for a *named* facet lane regardless of the mapping's declared `on_existing` (which stays `ignore`). `decide()` itself is unchanged — the adapter just chooses the mode. The `replace` prohibition in validation is about *whole-entity* replace.

---

## Phase 15.2 — Per-facet guard 🔑 (Completed)

**Scope**
- SQL: `conduit_projection_state` primary key extends from `(target_table, entity_key)` to `(target_table, entity_key, facet)`. The default facet is stored as `''`. Existing rows are already default-facet rows — no data migration, the `''` default covers them.
- Document: the Phase 13 sidecar becomes `{ facets: { "": {last_sequence, deleted, permanent}, "contact": {…} } }`.
- KV: the guard file gains the same `facets` map.
- `decide()` (`adapter/mod.rs`) is **unchanged** — it already takes one lane's `Option<GuardState>` and `event_seq`. The adapter now looks up the row for `(entity_key, event's facet)` instead of `(entity_key)`. This is the phase's proof that the Phase 12/13 decision core generalises to lanes with no edit.

**Guarantees**
- Each facet lane is an independent Phase 12/13 state machine: highest sequence within the lane wins, redelivery is a no-op, resurrection and permanent tombstones work per the existing rules.
- Disjoint-facet updates never interfere: `contact` seq 5 and `login` seq 6 both land regardless of arrival order.

**As built**
- SQL guard: `conduit_projection_state` gains `facet TEXT NOT NULL DEFAULT ''`, PK `(target_table, entity_key, facet)`. `SqliteAdapter::read_guard_lane` reads one lane. **Deviation:** there is *no* in-place migration — a guard table that predates Phase 15 (2-column PK) must be dropped. It is a derived cache and replay rebuilds it; the 3-column `ON CONFLICT` target cannot be satisfied by the old constraint, and `ALTER TABLE ADD COLUMN` can't add a PK column. All tests use fresh temp DBs; there are no deployed guard tables.
- Document / KV guard sidecars: the default facet stays in the flat fields (a pre-Phase-15 sidecar and every non-faceted entity round-trips byte-for-byte — this keeps the Phase 13/14 guard-content assertions valid unchanged); named-facet lanes live in a `facets: { name: {last_sequence, last_event_id, deleted, permanent} }` map. `ProjectionGuard::lane_state` / `KvGuard::lane_state` return the Phase 12/13 `GuardState` for a lane.
- `decide()` (`adapter/mod.rs`) is called unchanged — same signature, same body — with the lane's `Option<GuardState>`.

---

## Phase 15.3 — SQL partial update 🗄️ (Completed)

**Scope**
- A named-facet `SqlMapping` builds `INSERT INTO <table> (<key cols>, <facet cols>) VALUES (…) ON CONFLICT (<primary_key>) DO UPDATE SET <facet cols only = excluded.…>` — only the facet's columns appear in the `SET` list.
- **Entity-existence pre-check.** Before applying a facet update, the adapter reads the **default-facet** guard row for the entity:
  - absent (entity never created) → `Skipped(EntityAbsent)` (new `SkipReason`). A facet cannot partially-update a row that does not exist, and inserting a sparse row would violate `NOT NULL` on the create mapping's columns.
  - tombstoned → `Skipped(EntityAbsent)` (do not resurrect an entity through a facet; only a default-facet `upsert` resurrects — Phase 13.4).
  - live → run `decide()` on the facet lane, apply `Insert`/`Update` identically (both become `ON CONFLICT DO UPDATE SET <facet cols>` since the row already exists), upsert the facet's guard row.
- **Delete cascades to facets.** `operation: delete` on the default facet: after `DELETE FROM <table>`, tombstone **every** guard row for `(target_table, entity_key)` — default and all facets — in the same transaction. A later high-sequence facet update then hits a tombstoned default facet and is `Skipped(EntityAbsent)`; a default-facet `upsert` resurrection clears all lanes.
- All within the existing single transaction.

**Guarantees**
- A faceted `users` row ends with each column at the value written by the highest-sequence event *for that column's facet*.
- `Skipped(EntityAbsent)` for a facet update that races ahead of its entity's creation — see 15.5 for how replay avoids this.

**As built**
- `SqlMapping::build` emits, for a named-facet mapping, `INSERT INTO <table> (<all cols>) VALUES (…) ON CONFLICT (<primary_key>) DO UPDATE SET <non-key cols only>`. The entity-existence pre-check guarantees the `DO UPDATE` branch runs.
- `SkipReason::EntityAbsent` added (shared across SQL / document / KV — not a KV- or SQL-only variant). It is *not* a `decide()` outcome: the adapter reads the default-facet lane and returns `EntityAbsent` before calling `decide()` on the facet lane.
- Delete cascade / resurrection clear are one `UPDATE … WHERE target_table=? AND entity_key=? AND facet <> ''` inside the same transaction.

---

## Phase 15.4 — Document & KV partial update 📄 (Completed)

**Scope**
- **Document:** a named-facet `DocumentMapping` **shallow-merges** its resolved object into the existing entity document — its top-level keys replace, all other keys are preserved — then rewrites the file via atomic rename. Same default-facet existence pre-check and delete cascade as 15.3.
- **KV:** a named-facet `KvMapping` owns a disjoint subset of the value object's top-level keys; merged the same way.
- Single-writer assumption from Phase 12.4 / 14.2 is unchanged and now also covers the read-merge-write of a facet.

**Guarantees**
- Same convergence as 15.3, under a single writer.
- Only top-level keys are facet-owned — see Non-Goals on nested paths.

**As built**
- Document / KV adapters: for a named facet the write reads the existing entity file, shallow-merges the facet's resolved object's top-level keys (`existing_obj.insert(k, v)`), and rewrites via the existing atomic temp-then-rename. `Delete` and the default-facet full-write path are unchanged. A facet mapping whose `document` / `value` doesn't resolve to a JSON object is a `WriteFailed`.
- Same default-facet existence pre-check (`EntityAbsent`) and delete cascade (mark every `facets` entry `deleted`) as 15.3, and a default-facet resurrection clears every `facets` entry.

---

## Phase 15.5 — Sequence-ordered replay ⏱️ (Completed)

**Scope**
- `events_from_path` / the replay loader currently orders a directory by **filename** (`replay.rs`) and NDJSON by line order. `Event.sequence` has been captured since Phase 11 and used only for *gating*, never *ordering*.
- Phase 15 makes replay sort events by `(entity_key, sequence)` where the entity key is resolvable from the routed mapping, falling back to `(sequence)` then filename for events whose key cannot be resolved before routing.
- This makes **faceted replay deterministic regardless of file naming** — the create is applied before any facet update for the same entity — and retroactively strengthens the Phase 12–14 replay guarantee (which currently assumes filename order happens to match sequence order).

**Guarantees**
- Replaying a directory / NDJSON stream of one entity's events yields the same projected state as applying them in sequence order, whatever the file names.
- Live streaming is out of scope: an out-of-order facet-before-create in a live feed is still `Skipped(EntityAbsent)` (15.3). The contract is the same as Phase 12's sequence contract — the producer emits the create no later than the facet updates that depend on it.

**As built**
- `ReplayContext::run_stream_with_options` buffers the whole stream, then `sort_by_key(|e| e.sequence)` (a stable sort — the loader's sorted-file-name / line order breaks ties). **Simplification from the doc's `(entity_key, sequence)`:** a global stable sort by `sequence` alone is equivalent for the guarantee (each entity's events keep their relative order, so its create precedes its facet updates) and needs no routing/mapping resolution inside the loader. Cost: the stream is fully buffered rather than streamed — acceptable given determinism is the point of replay.

---

## Phase 15.6 — Determinism & verification suite 🧪 (Completed)

| Test | Asserts |
|---|---|
| `facet_disjoint_out_of_order.rs` | `contact` seq 5 and `login` seq 6 in both arrival orders → row has both the seq-5 email and the seq-6 login. |
| `facet_stale_within_lane.rs` | Two `contact` events seq 5 then seq 3 → the seq-3 one is `SkipStale`; email stays at the seq-5 value. |
| `facet_before_create_replay.rs` | Files named so the facet event sorts first → 15.5 reorders; final state correct. |
| `facet_before_create_live.rs` | Facet event dispatched before the create → `Skipped(EntityAbsent)`; documented behaviour. |
| `facet_delete_cascade.rs` | `[create s1, contact s2, delete s3, contact s7]` → entity absent; the seq-7 facet update is `Skipped(EntityAbsent)`. |
| `facet_resurrection.rs` | `[create s1, delete s2, create s3, contact s4]` → entity present, contact facet applied. |
| `facet_overlap_rejected.rs` | Two facets both listing `email` → `ValidationError` at startup. |
| `facet_replace_rejected.rs` | A faceted table with an `on_existing: replace` mapping → `ValidationError`. |
| `facet_cross_adapter.rs` | Same faceted stream into SQL + document → column/key values match. |

**Guarantee under test:** for a faceted entity, each facet's projected state is the Phase 12/13 pure function of that facet's delivered events; facets do not interfere; the entity exists iff its default facet's highest-sequence event is not a delete.

**As built** — all nine test files exist under `crates/conduit-core/tests/` with those names and pass. Two extra `validate_projection_config` cases (`FacetDeleteNotDefault`, `FacetKeyMismatch`) were added to `tests/validation.rs`. Full workspace: `cargo build`, `cargo test` (176 tests), `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check` all clean.

---

## Non-Goals (explicit)

- **Not deltas.** Every facet column / key value is fully resolved from the event payload / metadata. Conduit never computes `x = x + n` or reads the prior value to derive the new one. A facet write is a narrower *replace*, not an accumulate.
- **No nested / deep-path facets.** A facet owns whole top-level columns (SQL) or top-level keys (document / KV). `facet` cannot own `address.city` while another owns `address.zip`. Deep-merge has the "absent nested key = delete or no-op" ambiguity and is out of scope.
- **No per-column granularity.** The facet is the unit of independence. A column belongs to exactly one facet; an event either advances its facet's whole lane or is stale for all of it — no partial-success within one event.
- **Facets and `on_existing: replace` are mutually exclusive** per entity (15.1). Once an entity is partitioned into facets, all writes go through facets and the create is insert-only.
- **No event stashing / buffering.** A live out-of-order facet-before-create is skipped, not held for later reconciliation. Ordering is the producer's contract (15.5); replay handles it by sorting.
- **No cross-facet atomicity.** One event maps to one mapping to one facet; there is no multi-facet write and therefore no transaction spanning facets. A `delete`'s cascade across facet guard rows is the sole multi-lane operation and is confined to the default-facet adapter's existing transaction.
- **Narrow tables remain the recommendation** when a facet would be an entity's only non-key content.
