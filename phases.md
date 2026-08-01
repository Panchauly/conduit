# Conduit — Project Phases

This document tracks the **evolution, guarantees, and scope** of the Conduit projection engine.

Conduit is an **event-first, deterministic projection engine** that routes events to explicit adapters using schema-driven mappings.

---

## Phase 1 — Core Event Ingestion ✅ (Completed)

**Goal:** Establish the foundational event model and adapter abstraction.

### Scope
- Event structure (`event_id`, `event_type`, `payload`, `metadata`)
- `StorageAdapter` trait
- Adapter execution contract
- Adapter result and error model

### Guarantees
- Events are immutable
- Adapters are side-effect executors only
- No implicit enrichment

---

## Phase 2 — Adapter Abstractions & Runtime ✅ (Completed)

**Goal:** Support multiple storage backends via a unified runtime.

### Scope
- SQL adapter
- Document adapter
- Adapter priority ordering
- Adapter runtime builders

### Guarantees
- Adapter execution order is deterministic
- Adapter failures are observable
- No hidden adapter behavior

---

## Phase 3 — Explicit Mapping System ✅ (Completed)

**Goal:** Separate *what happened* from *how it is stored*.

### Scope
- SQL mappings (YAML)
- Document mappings (YAML)
- Field-level projection
- Event-to-schema validation

### Guarantees
- Same event can be projected differently per adapter
- No schema inference
- Mapping errors fail fast

---

## Phase 4 — Routing System (Adapter ID Based) ✅ (Completed)

**Goal:** Make routing explicit, deterministic, and replica-safe.

### Scope
- `routing.json`
- Routing by **adapter ID** (not storage kind)
- Removal of implicit/default routing
- Support for multiple adapters of same storage kind

### Example
```json
{
  "UserCreated": ["sql-primary", "doc-readmodel"],
  "CacheInvalidated": ["kv-cache"]
}
```

### Guarantees
- No routing → no execution
- Same event can fan out to multiple adapters
- Read/write replicas are first-class

---

## Phase 5 — Configuration & Validation ✅ (Completed)

**Goal:** Ensure invalid systems fail at startup, not runtime.

### Scope
- `ConduitConfig`
- Adapter configuration
- Unique adapter IDs
- Config versioning

### Guarantees
- Duplicate adapter IDs are rejected
- Invalid configs never reach execution
- Runtime behavior is fully declared in config

---

## Phase 6 — Observability & Execution Semantics ✅ (Completed)

### Phase 6.1 — Execution Reporting ✅ (Completed)

**Scope**
- `ExecutionReport` (event-level) and `AdapterExecutionReport` (adapter-level)
- Time injection at runtime; duration derived from timestamps (deterministic structure)
- `execute_event()` returns `ExecutionReport` only — stable public API, single contract
- `dispatch()` returns `ExecutionReport`; no legacy `Vec<AdapterResult>` in the execution path
- Crate-root re-export: `use conduit_core::ExecutionReport`
- CLI: text report (duration, ✓ / ~ / ✗, exit code 0 / 1); `--output json` for full report

**Guarantees**
- No adapter-level leakage; report is the only execution contract
- Fail-fast semantics preserved
- CLI output deterministic (no `{:?}`); JSON unchanged

---

### Phase 6.2 — Failure Policies ✅ (Completed)

**Scope**
- `failure_policy` in `ConduitConfig`: `fail_fast` (default) or `continue_on_error`
- `dispatch(event, adapters, failure_policy)`; `execute_event` passes config policy
- Fail-fast: stop on first adapter failure; continue-on-error: run all routed adapters, report all outcomes

**Guarantees**
- Default remains fail-fast; config is optional (`#[serde(default)]`)
- Adapter-level failure policy left for future

---

### Phase 6.3 — Explain & Dry-Run ✅ (Completed)

**Scope**
- `conduit explain --config <path> --event <path>`: show event type and routed adapter IDs (no execution)
- `conduit dry-run --config <path> --event <path>`: deterministic preview, same as explain (no writes)
- `route_with_rules(event, rules)` in core for explicit routing (used by explain)

**Guarantees**
- No side effects; routing resolved from config’s routing file path

---

### Phase 6.4 — Adapter Capability Contracts ✅ (Declaration shape)

**Scope**
- Optional `capabilities: Vec<String>` per adapter config (e.g. `["write", "idempotent"]`)
- `AdapterConfig::capabilities()`; type alias `AdapterCapabilities`
- Mapping ↔ adapter capability checks: Phase 8 (`requires_capabilities` on mappings)

---

## Phase 8 — Capability-safe validation ✅ (Completed)

**Scope**
- **`requires_capabilities`** on SQL and document mappings (`Vec<AdapterCapability>`; enum serde, no string caps).
- **`runtime/validation.rs`:** `validate_projection_config` — routing ↔ adapter IDs, per-target mapping presence, capability subset check, orphan mapping rules (SQL/doc mapped but never routed to that adapter kind).
- **`validate_routing_adapter_ids`:** lightweight check when mappings are not loaded.
- **`ValidationReport` / `ValidationIssue`:** aggregated multi-error `Display` for ops.
- **CLI:** `run`, `replay`, `dry-run` run full validation after load. **`explain`:** optional `--mappings <dir>` for full validation; without it, only adapter IDs **routed for that event** must exist in config (so routing files may list other adapters not yet configured).

**Recommended priority improvements (implemented)**

| # | Item | Behavior |
|---|------|----------|
| 1 | **Mapping key consistency** | SQL/document map keys must equal the mapping’s `event` field (same identity as routing). |
| 2 | **Adapter lookup map** | O(1) adapter resolution via id → config (no linear scan per route target). |
| 3 | **Duplicate adapter detection** | Duplicate ids in `adapters[]` reported in validation; duplicate adapter id on the **same** route entry also rejected. |
| 4 | **Empty document templates** | Reject `document: null`, `{}`, or `[]` (must define at least one projected field). |

**Guarantees**

| Guarantee | Meaning |
|-----------|---------|
| Mappings validated | Structural rules (non-empty table/columns/document); no dead SQL/doc mappings relative to routing |
| Adapters validated | Declared capabilities satisfy each routed mapping’s `requires_capabilities` |
| Routing validated | Every routed adapter id exists in config; every route target has the right mapping kind |
| Replay safety | Same bundle checks before replay (incompatible projections blocked at startup) |

**Not in scope:** new adapter types, extra mapping syntax beyond `requires_capabilities`, runtime capability negotiation, dynamic discovery.

---

## Phase 7 — Replay & Deterministic Rehydration ✅ (Completed)

**Scope**
- **`ReplayContext`:** builds adapters **once** + `adapter_metadata_map`; `run_stream` calls `dispatch_with_routing` per event (Phase 9 dependency order).
- **Explicit routing:** replay loads `routing.json` via path (not process-global cwd); `dispatch_with_routing` for tests/replay.
- **Streaming loader:** `events_from_path` → `Iterator<Item = Result<Event, ReplayLoadError>>` — directory (`*.json` sorted lexicographically), single JSON file, or NDJSON line-by-line.
- **`ReplayReport`:** counts, `stopped_early`, `first_failure_event_id`, `per_event` summaries; serde JSON.
- **Failure policy:** replay respects config `fail_fast` / `continue_on_error` (stop after first failed event vs process all).
- **CLI:** `conduit replay --config --mappings --events <file|dir> [--output text|json]`; exit `1` if any event failed.

**Guarantees**
- Deterministic directory order (sorted paths).
- Integration tests: ordering, NDJSON, fail-fast stop, continue-on-error.

**Phase 7 follow-ups (optimizations)**
- Multiple SQL (or file) adapters per config: same mapping bundle is **cloned** per adapter; routing can list several IDs per event; run order by **Phase 9** (`depends_on` graph, else priority then id).
- Directory loader uses `serde_json::from_reader`; NDJSON uses a single file read + in-memory cursor (no second open).
- `ReplayRunOptions`: cap successful per-event rows (`max_per_event_summaries`), stderr progress (`progress_interval`), strict routing (`validate_routing`); `ReplayReport.per_event_summaries_omitted` when capped.
- CLI: `--max-per-event`, `--progress-every`, `--validate-routing`.

---

## Phase 9 — Projection Dependency Graphs ✅ (Completed)

**Goal:** Order adapter execution by explicit `depends_on` edges among routed adapters, deterministically.

### Scope

| Area | Details |
|------|---------|
| **Config** | `depends_on: Vec<AdapterId>` on sqlite/file adapters (default empty). Each id must exist in config; no self-dependency (`ConfigError`). |
| **Metadata** | `adapter_metadata_map(config)` → `priority` + `depends_on` per id; topo uses this map only. |
| **Graph** | Nodes = unique ids on the route (`HashSet`). Edge `dep → adapter` when `dep` is in `depends_on` and both routed. |
| **Order** | Kahn topological sort; ready queue: ascending `priority`, then adapter id. |
| **Cycles** | `DependencyCycle` in validation; sorted adapter list for stable errors. Dispatch returns failed `ExecutionReport` (`_dispatch` row), never panics. |
| **Route closure** | Every `depends_on` for a routed adapter must appear on that event’s route (`DependencyNotOnRoute`). |
| **Validation** | `validate_projection_config` includes per-event dependency checks. `validate_routing_and_dependencies_for_event_type` for explain / routing-only checks. |
| **Dispatch / replay** | Same order via `adapter_meta` on every event. |
| **CLI** | `conduit explain` prints execution order, **execution layers** (Phase 9.5), and stable JSON (`execution_order`, `execution_layers`, `warnings`). |

### Execution pipeline

```
event → routing → dependency graph (routed only) → topological sort → dispatch adapters
```

### Guarantees

| Requirement | Result |
|-------------|--------|
| Dependencies respected | Dependents run after declared deps (on same route) |
| Cycle detection | Invalid configs rejected at validation; unvalidated dispatch → failed report |
| Deterministic ordering | Same priority+id tie-break; cycle errors list sorted ids |
| Replay consistency | `ReplayContext` stores `adapter_meta`; same sort as run |
| Backward compatible | Empty `depends_on` → order by priority then id among independents |

---

## Phase 9.5 — Dependency layers (optional guidance) ✅ (Completed)

**Goal:** Make projection structure easy to read in `conduit explain` without changing execution (still Phase 9 topo).

### Explain / JSON

- **Text:** `Execution layers:` with `Layer N: adapter1, adapter2` — adapters listed in **real execution order** within each layer.
- **JSON (stable):** `execution_order`, `execution_layers` as `[{ "layer": N, "adapters": [...] }, ...]`, `warnings` (empty or fixed depth warning string).
- **Depth warning (non-fatal):** stderr + `warnings` when any adapter is beyond the third stage (layer index > 2). Message is a **fixed constant** for stable tooling.

### Operational guidance

- **Keep dependency chains short** (ideally ≤ 3 stages: e.g. primary write → read models).
- **Prefer parallel fan-out** from one base adapter to several dependents instead of long serial chains.

**Prefer (fan-out):**

```
sql_write
 ├─ search_index
 └─ analytics
```

**Avoid (deep serial chain):**

```
sql → search → analytics → cache → reporting
```

### Core API

- `dependency_layers_parallel`, `dependency_layers_grouped`, `dependency_depth_exceeds_recommended` — display only; dispatch unchanged.

---

## Phase 10 — Projection Versioning & Schema Evolution ✅ (Completed)

**Goal:** Support explicit event/mapping versioning and deterministic upcasting so schema evolution never breaks historical replay.

---

### Phase 10.1 — Versioned Event Envelopes & Mapping Specs ✅ (Completed)

**Goal:** Establish explicit version tracking on event envelopes and YAML mapping specs without breaking existing $V_1$ workflows.

**Scope**
- **Event Model:** Add `version: u32` to standard event envelopes (`crates/conduit-core/src/event.rs`). Default `version = 1` when unmapped/omitted for full backward compatibility.
- **Mapping Specs:** Add required `version: u32` field to SQL and Document YAML schema definitions (`adapter/sql/mapping.rs`, `adapter/document/mapping.rs`).
- **Validation Rules:** Update `runtime/validation.rs` to enforce that mapping versions are positive non-zero integers ($V \ge 1$).

**Guarantees**
- Unversioned legacy JSON events automatically parse as `version = 1`.
- No implicit mapping versioning — every YAML spec must state its target schema version explicitly.

---

### Phase 10.2 — Pure-Function Upcaster Registry ✅ (Completed)

**Goal:** Implement in-memory payload upcasting to transform historical event payloads ($V_k \to V_{k+1}$) before mapping projection.

**Scope**
- **Upcaster Trait:** Define `Upcaster` trait (`crates/conduit-core/src/upcast.rs`) for side-effect-free `serde_json::Value` payload transformations.
- **UpcasterRegistry:** Registry storing upcasters indexed by `(event_type, source_version)`.
- **Chain Resolution:** Implement contiguous path resolution ($V_1 \to V_2 \to \dots \to V_{\text{target}}$). Multi-version jumps without a complete, unbroken chain fail fast during graph building.
- **Metadata Immutability:** Enforce that upcasting mutates payload JSON only; `event_id`, `timestamp`, and envelope headers remain immutable.

**Guarantees**
- No side effects allowed inside upcasters (no I/O, DB reads, or clock access).
- Upcast failures yield explicit `ConduitError::UpcastFailed`.
- No downcasting ($V_2 \to V_1$ is strictly forbidden).

---

### Phase 10.3 — Dispatch & Migration Policy Integration ⚡ (Completed)

**Goal:** Integrate version checking and upcasting into `dispatch.rs` and `ReplayContext` with configurable migration policies.

**Scope**
- **MigrationPolicy Enum:** Add `MigrationPolicy::Strict` (default) and `MigrationPolicy::IgnoreUnmatched` to `ConduitConfig`.
- **Pipeline Integration:**

```
event (V_src) ──► UpcasterRegistry ──► event (V_target) ──► Mapping Projection ──► Adapter Dispatch
```

- **Execution Reporting:** Expose `source_version` and `projected_version` inside `ExecutionReport` and `AdapterExecutionReport`.

**Guarantees**
- Strict Policy: If $V_{\text{event}} < V_{\text{mapping}}$ and no upcaster path exists, dispatch fails immediately with `ConduitError::UnsupportedVersion`.
- Ignore Policy: Skips projection for un-upcastable events, logging a skipped execution entry without halting the batch.

---

### Phase 10.4 — Replay Determinism & Verification Suite 🧪 (Completed)

**Goal:** Verify that historical event streams containing mixed payload versions ($V_1, V_2, \dots, V_n$) project deterministically.

**Scope**
- **Replay Verification:** Verify `conduit replay` produces identical target storage state across mixed-version streams as real-time dispatch.
- **Integration Tests** (`crates/conduit-core/tests/`):
  - `upcast_chain_resolution.rs` — verifies multi-step upcasting ($V_1 \to V_2 \to V_3$).
  - `upcast_missing_link.rs` — verifies fail-fast error reporting on broken upcaster chains.
  - `replay_versioned_stream.rs` — replays mixed $V_1/V_2/V_3$ fixtures and validates deterministic SQL/Document outputs.

**Guarantees**
- Replaying a stream of mixed-version historical events using a frozen upcaster registry is 100% deterministic and reproducible.

---

## Core Design Principles

- Event-first
- Explicit over implicit
- Deterministic execution
- Schema-driven mappings
- No hidden defaults
- Fail fast

---

## What Conduit Is NOT

- Not a database
- Not a Kafka replacement
- Not an ETL tool
- Not a schema inference system
