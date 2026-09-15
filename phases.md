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
| 19 | Postgres SQL Backend (19.1–19.6) | ✅ Completed | [phases/phase-19-postgres-sql-backend.md](phases/phase-19-postgres-sql-backend.md) |
| 20 | gRPC Ingestion Service (20.1–20.5) | ✅ Completed | [phases/phase-20-grpc-ingestion.md](phases/phase-20-grpc-ingestion.md) |
| 21 | OSS Release Readiness (21.1–21.6) | ✅ Completed | [phases/phase-21-oss-release-readiness.md](phases/phase-21-oss-release-readiness.md) |
| 22 | Redis Key-Value Backend (22.1–22.5) | ✅ Completed | [phases/phase-22-redis-kv-backend.md](phases/phase-22-redis-kv-backend.md) |
| 23 | MongoDB Document Backend (23.1–23.5) | ✅ Completed | [phases/phase-23-mongodb-document-backend.md](phases/phase-23-mongodb-document-backend.md) |
| 24 | Neo4j Graph Backend (24.1–24.6) | ✅ Completed | [phases/phase-24-neo4j-graph-backend.md](phases/phase-24-neo4j-graph-backend.md) |
| 25 | Backend Extension API & Community Connectors (25.1–25.5) | ✅ Completed | [phases/phase-25-backend-extension-api.md](phases/phase-25-backend-extension-api.md) |

Cross-cutting principles: [phases/design-principles.md](phases/design-principles.md) · Engine/producer boundary: [phases/producer-contract.md](phases/producer-contract.md)

---

## Beyond Phase 25

All four storage kinds now have a real database backend, not just a file-backed default — SQL on SQLite, Postgres, and MySQL (Phases 19, 25), key-value on Redis (Phase 22), document on MongoDB (Phase 23), and graph on Neo4j (Phase 24), every one of them behind the exact same `decide()`/guard model as the file adapters. Combined with the symmetric source side (Phase 17), the technology-agnostic producer contract (Phase 20), OSS release readiness (Phase 21), and now a public, documented extension point (`SqlTxn`/`KvBackend`/`DocumentBackend`/`GraphBackend` plus `register_adapter_factory`, Phase 25) so a community connector for a store Conduit doesn't ship can plug in without forking the crate, this is the complete `architecture.md` §1.4 pitch: **the open-source engine holds every real backend, not a limited edition of one, and growing that set doesn't require the core team.** Conduit is **projection-only** and structured in three parts with distinct dependency rules ([`architecture.md` §1.1–1.4](architecture.md), [`phases/producer-contract.md`](phases/producer-contract.md)).

Candidate future work, not yet planned in detail:

- **Pro modules** — native Kafka / Kinesis `EventSource` implementations, a distributed multi-node coordinator, compliance/audit encryption adapters, a visual `conduit explain` topology UI ([`architecture.md` §1.4](architecture.md)).
- **`conduit-core` dependency weight** — Phase 19 (`postgres`/`tokio`), Phase 20 (nothing, `tonic` is isolated in `conduit-ingest`), Phase 22 (`redis`), Phase 23 (`mongodb`, its own internally-pooled client, no r2d2), Phase 24 (`neo4rs` plus an internal `tokio::runtime::Runtime`, the same async-wrapped-in-sync pattern as Postgres), and Phase 25 (`mysql`, a fifth driver, first-party by explicit product decision — see phase 25's "As built" notes) mean the embeddable crate now pulls in every one of these drivers transitively; a feature gate to drop unused backends for lean embedded use is open.
- **Subgraph-per-event** — one mapping emitting several graph records atomically (deferred out of Phase 16).
- **TTL for the KV adapters** — deferred out of Phase 14/22 for replay-determinism reasons (time-based eviction isn't reproducible from a replayed log).
- **A real out-of-tree community connector** — Phase 25 built and proved the extension point (traits, registry, guide) using an in-tree MySQL backend as the worked example; an actual independent crate (`conduit-dynamodb`, `conduit-elasticsearch`, …) exercising the registry from outside this repo is still open.
