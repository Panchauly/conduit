# Phase 8 — Capability-safe validation ✅ (Completed)

**Scope**
- **`requires_capabilities`** on SQL and document mappings (`Vec<AdapterCapability>`; enum serde, no string caps).
- **`runtime/validation.rs`:** `validate_projection_config` — routing ↔ adapter IDs, per-target mapping presence, capability subset check, orphan mapping rules (SQL/doc mapped but never routed to that adapter kind).
- **`validate_routing_adapter_ids`:** lightweight check when mappings are not loaded.
- **`ValidationReport` / `ValidationIssue`:** aggregated multi-error `Display` for ops.
- **CLI:** `run`, `replay`, `dry-run` run full validation after load. **`explain`:** optional `--mappings <dir>` for full validation; without it, only adapter IDs **routed for that event** must exist in config (so routing files may list other adapters not yet configured).

**Recommended priority improvements (implemented)**

| # | Item | Behavior |
|---|------|----------|
| 1 | **Mapping key consistency** | SQL/document map keys must equal the mapping's `event` field (same identity as routing). |
| 2 | **Adapter lookup map** | O(1) adapter resolution via id → config (no linear scan per route target). |
| 3 | **Duplicate adapter detection** | Duplicate ids in `adapters[]` reported in validation; duplicate adapter id on the **same** route entry also rejected. |
| 4 | **Empty document templates** | Reject `document: null`, `{}`, or `[]` (must define at least one projected field). |

**Guarantees**

| Guarantee | Meaning |
|-----------|---------|
| Mappings validated | Structural rules (non-empty table/columns/document); no dead SQL/doc mappings relative to routing |
| Adapters validated | Declared capabilities satisfy each routed mapping's `requires_capabilities` |
| Routing validated | Every routed adapter id exists in config; every route target has the right mapping kind |
| Replay safety | Same bundle checks before replay (incompatible projections blocked at startup) |

**Not in scope:** new adapter types, extra mapping syntax beyond `requires_capabilities`, runtime capability negotiation, dynamic discovery.
