# Phase 14 — Cache Adapter (Key-Value) 🎯 (Completed)

**Goal:** A third storage kind — a flat key-value store — projected through the *same* `StorageAdapter` trait, mapping system, sequence-gated guard, and capability model as SQL and Document. The real deliverable is a yes/no answer: **does the adapter abstraction generalize without leaking into the engine core?**

**Why:**
- `StorageKind::KeyValue` already exists (`routing.rs:17`) and the test fixture routes `CacheInvalidated → kv-cache` (`tests/fixtures/routing.json`), but `AdapterConfig` has only `Sqlite` and `File` — a config routing to a `kv-*` adapter passes routing validation and then fails at construction. The kind is half-wired; nothing can fulfil it.
- Two of the four storage kinds named in the project pitch (SQL, document, cache, graph) exist.
- Phase 13 was the last prerequisite. A cache adapter with no invalidation is useless; `operation: delete` + tombstones now exist, so the KV adapter has a complete Create / Update / Delete model to implement — this ordering was deliberate.
- The KV adapter is the *simplest* remaining adapter — no query language, no schema, no relational structure. That makes it the cleanest possible test of whether `StorageAdapter`, `KvMapping` ⟷ the shared mapping concepts, `decide()` (Phase 12.2 / 13.1), `AdapterOutcome`, and the `AdapterCapability` checks actually generalize. Better to find abstraction leaks here than while building the graph adapter (Phase 16).

### What "generalizes cleanly" means (the phase's pass condition)

Adding this adapter must require **zero semantic changes** to:
- `dispatch.rs` / `replay.rs` — no new `StorageKind` match arm, no KV-specific branch.
- `runtime/dependency_graph.rs` — the KV adapter participates in `priority` / `depends_on` / execution frontiers with no special handling.
- `decide()` — the KV adapter calls it unchanged; if it needs a KV-specific case, that is a leak and a finding.
- `AdapterOutcome` / `SkipReason` — no new variant that exists only for KV.

New wiring is expected and fine: an `AdapterConfig::KeyValue` variant, a third mapping map threaded through `build_adapters_from_config`, a `keyvalue/` module mirroring `sql/` and `document/`. What must *not* happen is the engine core learning that key-value exists as a special case.

---

## Phase 14.1 — Config & mapping 🗂️ (Completed)

**Scope**
- `AdapterConfig` gains a `#[serde(rename = "keyvalue")]` variant:

  ```rust
  KeyValue(KeyValueAdapterConfig)   // id, priority, config: { root: String }, capabilities, depends_on
  ```

  Mirrors `FileAdapterConfig` — a file-backed store keeps tests deterministic and adds no external service dependency (Redis is a later backend, the way Postgres is a later SQL backend).
- New `crates/conduit-core/src/adapter/keyvalue/` module: `mod.rs`, `adapter.rs` (error type), `mapping.rs`, `loader.rs`, `runtime.rs`, `validate.rs`, `preview.rs`, `store.rs` (the concrete file-backed impl — analog of `sqlite.rs` / `file.rs`).
- `KvMapping`:

  ```yaml
  event: CacheInvalidated
  namespace: user-view          # keyspace segment; analog of `table` / `collection`
  version: 1
  key: payload.user_id          # path expression, resolved like the document `id`
  value: { name: payload.name, tier: payload.tier }   # JSON template, resolved like the document body
  operation: upsert             # upsert (default) | delete   — Phase 13.1
  on_existing: replace          # ignore (default) | replace  — Phase 12.2
  permanent: false              # delete-only, Phase 13.4
  requires_capabilities: []
  ```

  - `key` must resolve to a JSON scalar (reuses `is_identity_scalar` — Phase 11.1).
  - `value` may be a scalar, object, or array; ignored/rejected when `operation: delete` (no projection body — Phase 13.1 rule).
- `load_keyvalue_mappings` — YAML dir loader, same shape as `load_document_mappings`, duplicate-event detection included.

**Guarantees**
- A `KvMapping` reuses the exact `operation` / `on_existing` / `permanent` / `version` semantics of the other adapters — no KV-specific mapping vocabulary.

---

## Phase 14.2 — File-backed store & guard 💾 (Completed)

**Scope**
- Value path: `{root}/{namespace}/{key}.json`. Guard path: `{root}/.conduit/{namespace}/{key}.guard.json`, content `{ last_sequence: u64, deleted: bool, permanent: bool }` — structurally identical to the Phase 13 document sidecar / SQL guard row.
- Writes (value and guard) via write-temp-then-rename, reusing the `commit_guard` pattern from `file.rs`.
- `KeyValueStore::handle()` — read guard state → `decide(operation, on_existing, stored, event.sequence)` → apply:
  - `Insert` / `Update` → write the resolved value, then the guard.
  - `Delete` → remove the value file if present, write the tombstone guard (even with no prior value — delete-before-create convergence, Phase 13.2).
  - `SkipStale` / `SkipIdempotent` / `SkipAlreadyDeleted` / `Tombstoned` → the matching `Skipped(SkipReason)`, no write (bump `last_sequence` on `SkipAlreadyDeleted`).
- `KeyValueStore` declares `AdapterCapability::{Write, Idempotent, Upsert, Delete}` — **not** `Transactions` (single-key, file-backed, no multi-key atomicity).
- Single-writer assumption, identical to the document adapter (Phase 12.4). Cross-process read-decide-write is not atomic; stated, not solved.

**Guarantees**
- `decide()` is called unchanged — same function, same `GuardState`, same `WriteDecision`.
- Redelivery / out-of-order / resurrection / permanent-tombstone behaviour is inherited from `decide()`, not re-implemented.

---

## Phase 14.3 — Runtime, factory, dispatch wiring 🔌 (Completed)

**Scope**
- `KvRuntimeBuilder::new(kv_mappings)` — analog of `SqlRuntimeBuilder` / `DocumentRuntimeBuilder`; upcasts the payload to the mapping's target version before resolving `key` / `value` (Phase 10.3 integration, unchanged).
- `build_adapters_from_config` takes a third argument `kv_mappings: HashMap<String, KvMapping>` and builds `AdapterConfig::KeyValue` entries.
- `conduit dry-run` / preview: a `keyvalue/preview.rs` rendering the resolved key + value without writing (analog of the existing previews).

**Guarantees**
- `dispatch.rs`, `replay.rs`, `runtime/dependency_graph.rs` are **not modified** beyond threading the new mapping map — no `StorageKind::KeyValue` match arm in any of them. If one is needed, the phase surfaces it as an abstraction leak rather than adding it silently.

---

## Phase 14.4 — Capability-safe validation ✅ (Completed)

**Scope**
- `runtime/validation.rs` — the actually-wired checks (`sql/validate.rs` / `document/validate.rs` are dead code, never called outside their own module, superseded by Phase 8's `runtime/validation.rs` — a `keyvalue/validate.rs` mirroring them would just be a third unused file, so this phase adds the checks to the live validator instead): `key` is a valid `payload.`/`metadata.` path expression, `value` present iff `operation: upsert`, `version ≥ 1`, `namespace` non-empty; a routed `kv-*` adapter must have a `KvMapping` for each event type on its route; `operation: delete` ⇒ `requires_capabilities: [delete]`; `on_existing: replace` ⇒ `[upsert]` — the same rules already enforced for SQL and document, now covering the third kind.
- Closes the current gap: routing to a `keyvalue` adapter with no `KeyValue` config is caught at validation, not at construction.

**Guarantees**
- No new validation *mechanism* — the KV rules are instances of the existing capability / mapping-coverage checks.

---

## Phase 14.5 — Determinism & verification suite 🧪 (Completed)

The Phase 12 / 13 scenarios, re-run against the KV adapter, plus the three-way cross-adapter checks:

| Test | Asserts |
|---|---|
| `kv_upsert_out_of_order.rs` | Sequences `3, 1, 2` for one key → value == the sequence-3 state. |
| `kv_redelivery.rs` | Same event twice → second `Skipped(StaleSequence)`; value / guard byte-identical. |
| `kv_delete_lifecycle.rs` | `[set s1, update s2, invalidate s3]` → key absent; guard is a tombstone at 3. |
| `kv_delete_before_create.rs` | `invalidate s2` then `set s1` → key absent. |
| `kv_resurrection.rs` | `[set s1, invalidate s2, set s3]` → key present at sequence-3 value. |
| `kv_permanent_tombstone.rs` | `invalidate permanent s2`, then `set s5` → `Skipped(Tombstoned)`. |
| `kv_capability_rejected.rs` | Config routing a `delete` KV mapping to a non-delete adapter → validation error. |
| `kv_cross_adapter.rs` | The same stream into SQL + document + KV → all three converge (present with matching state, or all absent). |
| `kv_dispatch_unchanged.rs` | A KV adapter with `depends_on` a SQL adapter runs in the correct frontier — no dispatch code path specific to `KeyValue`. |

**Guarantee under test:** the KV adapter's final state for a key is the same pure function of the delivered event set that Phase 12 / 13 established for SQL and document — because it runs the same `decide()`, not a parallel implementation.

---

## Non-Goals (explicit)

- **No TTL / time-based expiry.** The one genuinely cache-specific feature, and it breaks replay determinism — an entry vanishing on a wall clock means replaying the same event stream yields different state at different times. The first cache adapter stays deterministic. TTL is a later opt-in (`ttl_seconds` on the mapping) that must carry an explicit "this mapping is no longer replay-deterministic" flag; out of scope here.
- **No Redis / external backend.** File-backed only, like the first SQL adapter was SQLite-only. A Redis `KeyValueStore` backend is a later phase, parallel to a Postgres SQL backend.
- **No multi-key operations or transactions.** Each event touches exactly one resolved key. No `MGET`/`MSET`, no cross-key atomicity — the adapter does not declare `AdapterCapability::Transactions`.
- **No scan / query / key-pattern matching.** Conduit writes keys; it does not read them back or enumerate them. `KEYS user:*` is not expressible.
- **No value-level merge.** `operation: upsert` with `on_existing: replace` writes the whole resolved value (Phase 12 semantics). Partial-value updates are the same deferred problem as partial-column SQL updates (Phase 15).
- **Not the graph adapter.** `StorageKind::Graph` stays unimplemented (Phase 16).
