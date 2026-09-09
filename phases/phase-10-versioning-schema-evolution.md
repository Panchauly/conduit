# Phase 10 — Projection Versioning & Schema Evolution ✅ (Completed)

**Goal:** Support explicit event/mapping versioning and deterministic upcasting so schema evolution never breaks historical replay.

---

## Phase 10.1 — Versioned Event Envelopes & Mapping Specs ✅ (Completed)

**Goal:** Establish explicit version tracking on event envelopes and YAML mapping specs without breaking existing $V_1$ workflows.

**Scope**
- **Event Model:** Add `version: u32` to standard event envelopes (`crates/conduit-core/src/event.rs`). Default `version = 1` when unmapped/omitted for full backward compatibility.
- **Mapping Specs:** Add required `version: u32` field to SQL and Document YAML schema definitions (`adapter/sql/mapping.rs`, `adapter/document/mapping.rs`).
- **Validation Rules:** Update `runtime/validation.rs` to enforce that mapping versions are positive non-zero integers ($V \ge 1$).

**Guarantees**
- Unversioned legacy JSON events automatically parse as `version = 1`.
- No implicit mapping versioning — every YAML spec must state its target schema version explicitly.

---

## Phase 10.2 — Pure-Function Upcaster Registry ✅ (Completed)

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

## Phase 10.3 — Dispatch & Migration Policy Integration ⚡ (Completed)

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

## Phase 10.4 — Replay Determinism & Verification Suite 🧪 (Completed)

**Goal:** Verify that historical event streams containing mixed payload versions ($V_1, V_2, \dots, V_n$) project deterministically.

**Scope**
- **Replay Verification:** Verify `conduit replay` produces identical target storage state across mixed-version streams as real-time dispatch.
- **Integration Tests** (`crates/conduit-core/tests/`):
  - `upcast_chain_resolution.rs` — verifies multi-step upcasting ($V_1 \to V_2 \to V_3$).
  - `upcast_missing_link.rs` — verifies fail-fast error reporting on broken upcaster chains.
  - `replay_versioned_stream.rs` — replays mixed $V_1/V_2/V_3$ fixtures and validates deterministic SQL/Document outputs.

**Guarantees**
- Replaying a stream of mixed-version historical events using a frozen upcaster registry is 100% deterministic and reproducible.
