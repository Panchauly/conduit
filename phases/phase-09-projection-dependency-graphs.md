# Phase 9 — Projection Dependency Graphs ✅ (Completed)

**Goal:** Order adapter execution by explicit `depends_on` edges among routed adapters, deterministically.

### Scope

| Area | Details |
|------|---------|
| **Config** | `depends_on: Vec<AdapterId>` on sqlite/file adapters (default empty). Each id must exist in config; no self-dependency (`ConfigError`). |
| **Metadata** | `adapter_metadata_map(config)` → `priority` + `depends_on` per id; topo uses this map only. |
| **Graph** | Nodes = unique ids on the route (`HashSet`). Edge `dep → adapter` when `dep` is in `depends_on` and both routed. |
| **Order** | Kahn topological sort; ready queue: ascending `priority`, then adapter id. |
| **Cycles** | `DependencyCycle` in validation; sorted adapter list for stable errors. Dispatch returns failed `ExecutionReport` (`_dispatch` row), never panics. |
| **Route closure** | Every `depends_on` for a routed adapter must appear on that event's route (`DependencyNotOnRoute`). |
| **Validation** | `validate_projection_config` includes per-event dependency checks. `validate_routing_and_dependencies_for_event_type` for explain / routing-only checks. |
| **Dispatch / replay** | Same order via `adapter_meta` on every event. |
| **CLI** | `conduit explain` prints execution order, **execution layers** (Phase 9.5), and stable JSON (`execution_order`, `execution_layers`, `warnings`). |

### Execution pipeline

```
event → routing → dependency graph (routed only) → topological sort → dispatch adapters
```

> **Note:** the dependency graph controls *execution order only*. No data is passed between adapters — every adapter independently resolves its own projection from the same `Event`. See Phase 11's design note on why this is intentional and not being changed.

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
