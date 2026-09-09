# Phase 6 — Observability & Execution Semantics ✅ (Completed)

## Phase 6.1 — Execution Reporting ✅ (Completed)

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

## Phase 6.2 — Failure Policies ✅ (Completed)

**Scope**
- `failure_policy` in `ConduitConfig`: `fail_fast` (default) or `continue_on_error`
- `dispatch(event, adapters, failure_policy)`; `execute_event` passes config policy
- Fail-fast: stop on first adapter failure; continue-on-error: run all routed adapters, report all outcomes

**Guarantees**
- Default remains fail-fast; config is optional (`#[serde(default)]`)
- Adapter-level failure policy left for future

---

## Phase 6.3 — Explain & Dry-Run ✅ (Completed)

**Scope**
- `conduit explain --config <path> --event <path>`: show event type and routed adapter IDs (no execution)
- `conduit dry-run --config <path> --event <path>`: deterministic preview, same as explain (no writes)
- `route_with_rules(event, rules)` in core for explicit routing (used by explain)

**Guarantees**
- No side effects; routing resolved from config's routing file path

---

## Phase 6.4 — Adapter Capability Contracts ✅ (Declaration shape)

**Scope**
- Optional `capabilities: Vec<String>` per adapter config (e.g. `["write", "idempotent"]`)
- `AdapterConfig::capabilities()`; type alias `AdapterCapabilities`
- Mapping ↔ adapter capability checks: Phase 8 (`requires_capabilities` on mappings)
