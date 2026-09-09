# Phase 12 — Sequence-Gated Upsert 🎯 (Planned)

**Goal:** An entity already projected by Conduit can be *updated* by later events, not only created once. Which write wins is decided by the event's `sequence`, not its delivery order — so a mixed create+update stream replayed in any order converges to one deterministic state. Scoped to full-entity replacement (no partial-column or delta updates; see Non-Goals).

**Why:** the engine is insert-only today.
- **SQL:** `SqlMapping::build()` only ever emits `INSERT INTO` (`adapter/sql/mapping.rs`). A second event for an existing entity is skipped by the Phase 11 guard — there is no path to change a projected row.
- **Document:** the file adapter's Phase 11.3 guard skips any event whose entity already has a guard marker (`adapter/document/file.rs`). A `UserRenamed` / `OrderShipped` / `BalanceChanged` event has nowhere to land.
- **Replay determinism** (Phase 7, Phase 10.4) currently depends on the *input* being pre-ordered: `events_from_path` sorts directory entries by **filename** (`replay.rs`), and `Event.sequence` — required on the envelope since Phase 11 — is captured but never consulted. Sequence-gated compare-and-swap is what makes upsert projections order-independent.

Phase 11 laid the groundwork deliberately: `Event.sequence: u64` (required, no default), the `conduit_projection_state` guard table already carries `last_sequence` (stored, not compared), and `AdapterCapability::Upsert` is already a defined enum variant with nothing behind it.

---

## The sequence contract (assumption)

`sequence` is assigned by the **event producer / event store**, upstream of Conduit — exactly like `event_id`, `version`, and metadata. Conduit reads it, never generates it. Same principle as Phase 11's producer-minted identity.

- **Per-entity, monotonic.** For successive events about one entity, `sequence` strictly increases. A per-aggregate version *or* a global monotonic log offset (Kafka offset, EventStore commit position) both satisfy this. Different entities have independent sequences; the CAS only ever compares against `last_sequence` for the same `(target_table, entity_key)`.
- **Gaps are allowed.** `1, 2, 7, 50` is fine — CAS needs ordering, not contiguity.
- **Not the timestamp.** Wall-clock event time must not be used for gating (clock skew, collisions, no monotonicity guarantee). `execution/time.rs` virtual time is unrelated — a deterministic clock for time-dependent projections, not an ordering key.
- **Unrelated to `version`.** `version` is the payload *schema* version (upcasting). An entity on schema `version 3` may be at `sequence 47`.

Conduit sees one event at a time and **cannot validate** the contract. If a producer reuses a sequence for two distinct events of one entity, the later-delivered one is judged stale and dropped (silent loss). If sequence is non-monotonic, the "newer" update is rejected. This is a real cost on the producer, stated and not enforced — as with Phase 11's identity requirement.

### Entity id canonical form

The guard is keyed by `entity_key` (Phase 11.1 canonical encoding). Conduit compares the **raw resolved string** — `json_scalar_to_string` does no normalization. A producer that emits the same entity's id inconsistently (`550e8400-…` vs `{550E8400-…}`) produces two `entity_key`s and therefore two entities. The producer must emit the id in one canonical textual form. Also unenforceable, also the producer's responsibility.

---

## Phase 12.1 — Adapter outcome model 📊 (Planned)

**Goal:** Replace the `success: bool` + `AdapterError::Skipped(String)` encoding with a machine-readable outcome, before any write-mode logic depends on telling six results apart.

**Scope**
- New `AdapterOutcome` in `adapter/mod.rs`:

  ```rust
  enum AdapterOutcome {
      Created,
      Updated,
      Skipped(SkipReason),   // AlreadyProjected | StaleSequence | UnsupportedVersion
      Failed(AdapterError),
  }
  ```

- `AdapterResult` carries `AdapterOutcome` instead of `success: bool` + `error: Option<AdapterError>` + the `Skipped(String)`-in-the-error-field hack. Keep small helpers (`is_success`, `is_skipped`) so call sites stay readable.
- Surface the outcome in `ExecutionReport` / `AdapterExecutionReport` (`execution/report.rs`) and the CLI `--output json`. These are stable public API (commit `c236b3f`), so the JSON shape change is called out in release notes.
- Migrate every Phase 10 / 11 call site: `skipped_version` → `Skipped(UnsupportedVersion)`, the Phase 11 idempotent skip → `Skipped(AlreadyProjected)`, plain success → `Created`.

**Guarantees**
- No behavior change. `Updated` and `Skipped(StaleSequence)` are defined but never produced until 12.3+.
- Tests that string-matched a skip message now match an enum variant.

---

## Phase 12.2 — Write mode & the gated decision 🚦 (Planned)

**Scope**
- New field on `SqlMapping` and `DocumentMapping`: `on_existing`, `#[serde(default)]`, `#[serde(rename_all = "snake_case")]`:

  ```rust
  enum OnExisting { #[default] Ignore, Replace }
  ```

  - `ignore` — entity already projected → clean `Skipped(AlreadyProjected)`. Identical to Phase 11 behavior; it is the default, so every existing mapping is unchanged.
  - `replace` — entity exists → overwrite the whole projected state, gated by `sequence`.
- Pure decision function — exhaustively unit-tested, no adapter, no I/O:

  ```rust
  fn decide(mode: OnExisting, exists: bool, stored_last_seq: Option<u64>, event_seq: u64) -> WriteDecision
  // WriteDecision = Insert | Update | SkipIdempotent | SkipStale
  ```

| mode | exists? | `event_seq` vs `stored_last_seq` | decision |
|---|---|---|---|
| `ignore` | no | — | `Insert` |
| `ignore` | yes | — | `SkipIdempotent` |
| `replace` | no | — | `Insert` |
| `replace` | yes | `event_seq > stored` | `Update` |
| `replace` | yes | `event_seq <= stored` | `SkipStale` |

**Guarantees**
- `decide` is total and side-effect free.
- `WriteDecision` maps 1:1 onto `AdapterOutcome` (`Insert`→`Created`, `Update`→`Updated`, `SkipIdempotent`→`Skipped(AlreadyProjected)`, `SkipStale`→`Skipped(StaleSequence)`).

---

## Phase 12.3 — SQL upsert 🗄️ (Planned)

**Scope**
- `SqlMapping::build()` gains an upsert form for `on_existing: replace`:

  ```sql
  INSERT INTO <table> (<cols>) VALUES (<?>)
  ON CONFLICT (<primary_key cols>) DO UPDATE SET <col = excluded.col, ...>
  ```

  Every mapped column is in the `SET` list (full replace). The conflict target is the Phase 11.1 `primary_key` column(s) — single or composite.
- `SqliteAdapter::handle()` (`adapter/sql/sqlite.rs`), all inside the existing transaction:
  - Read `last_sequence` for `(table, entity_key)` from `conduit_projection_state`.
  - Run `decide(…)`. `SkipStale` / `SkipIdempotent` → return the matching `Skipped(…)`, commit no-op.
  - `Insert` → the statement (harmless as an upsert when no row exists), then `INSERT` the guard row (unchanged from Phase 11).
  - `Update` → the upsert statement, then `UPDATE conduit_projection_state SET last_sequence = ?, last_event_id = ?, processed_at = datetime('now')` for `(table, entity_key)`.
  - SQLite serializes writers, so read-decide-write is atomic against concurrent dispatch.
- `SqliteAdapter` declares `AdapterCapability::Upsert`.

**Guarantees**
- `replace` + entity absent behaves exactly like `ignore` + entity absent (`Created`).
- Redelivery of an applied event → `Skipped(StaleSequence)` (`event_seq == last_sequence`); row and guard byte-identical.
- Out-of-order delivery converges: applying sequences `{3, 1, 2}` in any order leaves the row at the state carried by sequence 3.
- Guard `last_sequence` is the max sequence ever applied to that entity; it never decreases.

---

## Phase 12.4 — Document upsert 📄 (Planned)

**Scope**
- The Phase 11.3 guard marker becomes a small JSON sidecar: `{ "last_sequence": u64, "last_event_id": String }`, still committed via write-temp-then-rename (`file.rs::commit_guard`).
- `FileDocumentAdapter::handle()`:
  - Read + parse the sidecar if present → `stored_last_seq`.
  - `decide(…)`. `SkipStale` / `SkipIdempotent` → matching `Skipped(…)`, no write.
  - `Insert` / `Update` → write the full projected document to the entity path (`{root}/{event_type}/{entity_id}.json`), overwriting, then rewrite the sidecar via atomic rename.
- `FileDocumentAdapter` declares `AdapterCapability::Upsert`.

**Guarantees**
- Same convergence guarantees as 12.3 **under a single writer**.
- Document write + sidecar update are two filesystem operations; a crash between them re-processes the event on the next run — the sidecar still shows the old sequence, so `Update` re-applies the same full state (idempotent).

**Known limitation:** the read-decide-write is not atomic across processes. Two concurrent writers can both read `last_sequence = 5` and race. SQL is safe via the transaction; the file adapter assumes a single writer. Multi-writer document safety (lockfile, or moving document state into a real KV store) is a non-goal — see below.

---

## Phase 12.5 — Capability-safe validation ✅ (Planned)

**Scope**
- `on_existing: replace` implies `requires_capabilities: [upsert]` for that mapping (Phase 6.4 / Phase 8 machinery).
- `runtime/validation.rs` rejects at startup any config routing a `replace` mapping to an adapter that does not declare `AdapterCapability::Upsert` — new `ValidationError` variant, consistent with the existing capability-mismatch check.

**Guarantees**
- A `replace` mapping on a non-upsert adapter fails validation, not at runtime.
- Both built-in adapters declare `Upsert` after 12.3 / 12.4, so this is future-proofing for the cache / graph adapters — but the guarantee holds regardless.

*(May fold into 12.3 / 12.4 if it stays this small.)*

---

## Phase 12.6 — Determinism & verification suite 🧪 (Planned)

New integration tests in `crates/conduit-core/tests/`, fixtures under `tests/fixtures/`:

| Test | Asserts |
|---|---|
| `upsert_out_of_order.rs` | Apply sequences `3, 1, 2` for one entity (SQL and document) → final projected state == the state carried by sequence 3 alone. |
| `upsert_redelivery.rs` | Apply the same event twice → second returns `Skipped(StaleSequence)`; row / file / guard byte-identical to after the first. |
| `replay_create_then_update.rs` | `[create s1, rename s2, rename s3]` replayed from empty → byte-identical to applying once; extends `replay_versioned_stream.rs`. |
| `replay_shuffled_stream.rs` | The same stream fed in three different orders → identical final state each time. |
| `reject_mode_unchanged.rs` | An update event under `on_existing: ignore` → `Skipped(AlreadyProjected)`; all Phase 11 idempotency tests still green. |
| `upsert_composite_key.rs` | Bridge-table row (`primary_key: [left, right]`) updated in place by a newer-sequence event for the same pair; a different pair inserts. |
| `upsert_capability_rejected.rs` | Config routing a `replace` mapping to a non-upsert adapter → validation error at startup. |
| `upsert_cross_adapter.rs` | The same create+update stream projected into SQL and document → entity state matches between the two. |

**Guarantee under test:** for `on_existing: replace` mappings, the final projected state is a pure function of the *set* of delivered events (the one with the highest `sequence` per entity), independent of delivery order, duplicates, and batching.

---

## Non-Goals (explicit)

- **No producer-side deltas.** Conduit never computes `balance = balance + amount`. Every mapped value is fully resolved from the event payload / metadata, exactly as in every prior phase. An event that logically increments a value carries the *computed result* (`new_balance`), not the delta — the arithmetic happens on the command side where aggregate invariants are already enforced. This is a design position, not a deferral.
- **No partial-column / field-level updates.** A `replace` mapping produces the *entire* row / document; it cannot write a subset of columns and leave the rest. This is philosophically compatible with Conduit (the written value is still producer-computed, just narrower) but breaks the single per-entity `last_sequence` gate: two partial updates to disjoint columns arriving out of order would falsely reject the lower-sequence one and lose its change. A correct design needs per-`(entity, field-group)` sequence lanes and a create-vs-update mapping taxonomy — its own phase (candidate Phase 13). Escape hatch today: project the narrow event into its own entity / table with a full-row `replace` mapping.
- **No delete / tombstone semantics.** An event that removes a projected entity is out of scope — its own phase.
- **No conflict resolution beyond highest-sequence-wins.** No field-level merge, no last-write-by-timestamp, no vector clocks.
- **No multi-writer safety for the document adapter** (see 12.4).
- **No gap detection.** Conduit does not track or alert on missing sequences.
- **`on_existing: ignore` stays order-sensitive for live out-of-order delivery.** If an entity legitimately receives more than one event, use `replace`. Under `ignore` the first-delivered event wins regardless of sequence (unchanged from Phase 11); replay stays deterministic because the input is sorted.
- **No migration of existing projected rows.** Adding `on_existing: replace` to a mapping does not retro-apply updates that were skipped while it was `ignore`; it takes effect for events processed after the change.
