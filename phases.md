# Conduit — Project Phases

This document tracks the **evolution, guarantees, and scope** of the Conduit projection engine.

Conduit is an **event-first, deterministic projection engine** that routes events to explicit adapters using schema-driven mappings.

Full detail for each phase lives in [`phases/`](phases/) — this file is an index only.

---

## Phase Index

| Phase | Title | Status | Doc |
|-------|-------|--------|-----|
| 1 | Core Event Ingestion | ✅ Completed | [phases/phase-01-core-event-ingestion.md](phases/phase-01-core-event-ingestion.md) |
| 2 | Adapter Abstractions & Runtime | ✅ Completed | [phases/phase-02-adapter-abstractions-runtime.md](phases/phase-02-adapter-abstractions-runtime.md) |
| 3 | Explicit Mapping System | ✅ Completed | [phases/phase-03-explicit-mapping-system.md](phases/phase-03-explicit-mapping-system.md) |
| 4 | Routing System (Adapter ID Based) | ✅ Completed | [phases/phase-04-routing-system.md](phases/phase-04-routing-system.md) |
| 5 | Configuration & Validation | ✅ Completed | [phases/phase-05-configuration-validation.md](phases/phase-05-configuration-validation.md) |
| 6 | Observability & Execution Semantics (6.1–6.4) | ✅ Completed | [phases/phase-06-observability-execution-semantics.md](phases/phase-06-observability-execution-semantics.md) |
| 7 | Replay & Deterministic Rehydration | ✅ Completed | [phases/phase-07-replay-deterministic-rehydration.md](phases/phase-07-replay-deterministic-rehydration.md) |
| 8 | Capability-Safe Validation | ✅ Completed | [phases/phase-08-capability-safe-validation.md](phases/phase-08-capability-safe-validation.md) |
| 9 | Projection Dependency Graphs (incl. 9.5) | ✅ Completed | [phases/phase-09-projection-dependency-graphs.md](phases/phase-09-projection-dependency-graphs.md) |
| 10 | Projection Versioning & Schema Evolution (10.1–10.4) | ✅ Completed | [phases/phase-10-versioning-schema-evolution.md](phases/phase-10-versioning-schema-evolution.md) |
| 11 | Entity-Aware Idempotent Insert | ✅ Completed | [phases/phase-11-entity-aware-idempotent-insert.md](phases/phase-11-entity-aware-idempotent-insert.md) |

Cross-cutting principles: [phases/design-principles.md](phases/design-principles.md)

---

## Currently Only Two of Four Advertised Storage Kinds Exist

SQL (SQLite only) and Document adapters are implemented. Cache and Graph adapters — both named in the project pitch — do not exist yet. Candidate future phases, not yet planned in detail:

- **Phase 12** — Cache adapter (simplest semantics, no query language — good test of whether the adapter abstraction generalizes beyond SQL/document)
- **Phase 13** — Graph adapter
