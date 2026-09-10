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
| 12 | Sequence-Gated Upsert (12.1–12.6) | ✅ Completed | [phases/phase-12-sequence-gated-upsert.md](phases/phase-12-sequence-gated-upsert.md) |
| 13 | Deletion & Tombstones (13.1–13.6) | ✅ Completed | [phases/phase-13-deletion-tombstones.md](phases/phase-13-deletion-tombstones.md) |
| 14 | Cache Adapter (Key-Value) (14.1–14.5) | ✅ Completed | [phases/phase-14-cache-adapter.md](phases/phase-14-cache-adapter.md) |
| 15 | Facet Partial Updates (15.1–15.6) | ✅ Completed | [phases/phase-15-facet-partial-updates.md](phases/phase-15-facet-partial-updates.md) |
| 16 | Graph Adapter (Nodes & Edges) (16.1–16.6) | ✅ Completed | [phases/phase-16-graph-adapter.md](phases/phase-16-graph-adapter.md) |
| 17 | Event Sources (17.1–17.6) | ✅ Completed | [phases/phase-17-event-sources.md](phases/phase-17-event-sources.md) |
| 18 | Consolidation & Release Hygiene (18.1–18.8) | ✅ Completed | [phases/phase-18-consolidation-release-hygiene.md](phases/phase-18-consolidation-release-hygiene.md) |

Cross-cutting principles: [phases/design-principles.md](phases/design-principles.md) · Engine/producer boundary: [phases/producer-contract.md](phases/producer-contract.md)

---

## Beyond Phase 18

All four storage kinds from the project pitch exist (Phases 11–16, file-backed, one `decide()` core), the symmetric source side exists (Phase 17), and the debt from that run is paid down (Phase 18). Conduit is **projection-only** — it consumes a log it does not own; producing / storing the log is permanently out of scope ([`architecture.md` §1.1](architecture.md), [`phases/producer-contract.md`](phases/producer-contract.md)).

Candidate future work, not yet planned in detail:

- **Later backends** — Postgres SQL backend, Redis key-value backend, Neo4j/Cypher graph backend (each currently single-implementation, like SQLite / file-backed).
- **Later sources** — Kafka, Postgres-outbox, and HTTP-ingest sources on the Phase 17 `EventSource` trait. A source whose backing system owns offsets (Kafka consumer group, log cursor) uses *that*, not Conduit's checkpoint file (Phase 18.8).
- **Subgraph-per-event** — one mapping emitting several graph records atomically (deferred out of Phase 16).
