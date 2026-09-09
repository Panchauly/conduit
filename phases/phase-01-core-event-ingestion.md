# Phase 1 — Core Event Ingestion ✅ (Completed)

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
