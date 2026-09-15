# Phase 24 — Neo4j Graph Backend 🎯 (Completed)

**Goal:** A real database backend for the Graph kind — the last of the four storage kinds to get one. Completes `architecture.md` §1.4's "every storage kind, with a real backend, in core." Same seam pattern as Phases 19/22/23; the hardest of the four because writes are Cypher, not a key/row/document put.

**Why:** Neo4j is the obvious real backend for a graph projection — same "highest expectation" logic used for Redis and MongoDB, just for the smaller graph-database audience. It's sequenced last because it's the biggest lift: generating Cypher and mapping Conduit's sequence-gating onto a query language, rather than a simple put/upsert call.

---

## Phase 24.1 — Graph backend seam 🧩 (Completed)

**Scope**
- Extract the orchestration in `adapter/graph/store.rs` (read node/edge guard → `decide()` → apply MERGE/DETACH DELETE → write guard → maintain the incident index) into a `GraphBackend` trait:

  ```rust
  trait GraphBackend {
      fn read_node_guard(&mut self, label: &str, node_id: &str, facet: &str) -> Result<Option<GraphGuard>, GraphError>;
      fn read_edge_guard(&mut self, edge_type: &str, key: &str) -> Result<Option<GraphGuard>, GraphError>;
      fn write_node(&mut self, label: &str, node_id: &str, properties: Option<&Value>, facet: &str, guard: &GraphGuard) -> Result<(), GraphError>;
      fn write_edge(&mut self, edge_type: &str, from: &str, to: &str, key: &str, properties: Option<&Value>, guard: &GraphGuard) -> Result<(), GraphError>;
      fn delete_node_cascade(&mut self, label: &str, node_id: &str, guard: &GraphGuard) -> Result<(), GraphError>;
      fn delete_edge(&mut self, edge_type: &str, key: &str, guard: &GraphGuard) -> Result<(), GraphError>;
  }
  ```

- `GraphGuard` (Phase 13/15/16.2) is reused unchanged as the guard schema.
- `FileGraphStore` implements `GraphBackend` over the existing node/edge files + incident index. Behaviour byte-identical to every Phase 16 test.

**Guarantees**
- `decide()` unchanged, called identically for both record kinds, on both backends.

**As built**
- `adapter/graph/exec.rs`: `GraphGuard`/`GraphFacetGuard` (moved from `store.rs`, fields made `pub`), the `GraphBackend` trait, `NodePlan`/`EdgePlan`, `GraphOutcome`, and **two** orchestration entry points — `project_node()` and `project_edge()` — rather than one `project()`, matching the file store's own pre-existing `handle_node`/`handle_edge` split (nodes and nodes alone carry facets; edges have a structurally different identity and a Phase 16.4 endpoint-tombstone pre-check node-writes don't need).
- **Trait signature adjusted from the sketch:** `delete_edge` takes `(edge_type, from, to, key, guard)`, not just `(edge_type, key, guard)` — the file backend's `drop_incident` genuinely needs both endpoint ids to update *their* incident indexes; Neo4j's implementation ignores `from`/`to` (the relationship carries its own endpoints in the graph structure). Also added `node_is_tombstoned(node_id)` to the trait (not in the doc's sketch) to cover the Phase 16.4 edge-attach pre-check generically — the file backend scans its label directories exactly as before; Neo4j does it in one query, without needing to know which label the endpoint has.
- `FileGraphStore` from the sketch is, as built, `FileGraphBackend` — matching the `FileKvBackend`/`FileDocumentBackend` naming from Phases 22–23; `GraphStore` (the `StorageAdapter`) keeps its existing name, only its internals refactored. Every pre-24.1 graph test passes unchanged.

---

## Phase 24.2 — Neo4j backend & guard placement 🕸️ (Completed)

**Scope**
- New dependency: `neo4rs` (Bolt protocol driver; async by nature — wrapped in a small internal runtime to keep the adapter path synchronous, the same pattern Phase 19.2 used for the sync `postgres` crate over `tokio-postgres`).
- `AdapterConfig::Neo4j` → `Neo4jAdapterConfig { id, priority, config: { uri: String, user: String, password: String, database: Option<String> }, capabilities, depends_on }`. `uri`/`password` support `${ENV_VAR}` expansion.
- **Guard placement:** a dedicated `__ConduitGuard` node label, one guard node per `(target, entity_key, facet)`, with a uniqueness constraint on that triple — kept **separate** from the user's projected nodes/edges, same principle as every other adapter's guard. (Neo4j *could* store guard fields as extra properties directly on the projected node/relationship, avoiding a second element entirely — rejected in favour of consistency: "the guard is never mixed into your projected shape" holds identically across all four kinds, rather than being special-cased here because the storage happens to allow it.)

**Guarantees**
- Same entity-key / edge-key (Phase 11.1/16.1 canonical encoding) scheme as the file adapter.

**As built**
- Dependency: `neo4rs = "0.8"` (async-only, Bolt protocol) plus a direct `tokio = { features = ["rt-multi-thread"] }` — `Neo4jAdapter` builds one `tokio::runtime::Runtime` at construction and bridges every async driver call with `rt.block_on(...)`, the same "sync wrapper over an async driver" shape as Phase 19.2's `postgres` crate (which does the identical trick over `tokio-postgres` internally).
- `AdapterConfig::Neo4j` / `Neo4jConfig { uri, user, password, database: Option<String> }` — four separate fields exactly as scoped (every other backend uses one combined `url`), since `uri`/`user`/`password` are genuinely separate concerns in `neo4rs::ConfigBuilder`. `uri`/`password` support `${ENV_VAR}` expansion.
- **Deviation, forced by a real internal contradiction:** this sub-phase's guard placement — "one guard node per `(target, entity_key, facet)`" — directly contradicts 24.1's "reuse `GraphGuard` unchanged" (a struct with facets *nested*, one record per entity). Unlike Phase 23's version of this same contradiction (resolved toward "reuse unchanged," since MongoDB could support either shape), **this one resolves toward 24.2's flat, one-node-per-lane model** — not a stylistic preference this time but a hard requirement: Phase 24.3's CAS needs `last_sequence` to be a plain, independently queryable property *per facet lane*, and Neo4j property values cannot hold a nested map at all (not even a JSON-string blob would help, since a Cypher `WHERE` can't parse one). `read_node_guard`/`read_edge_guard` reconstruct the `GraphGuard` trait return type by querying every `__ConduitGuard` row for that `(kind, target, entity_key)` and folding facet rows into the nested shape the seam expects — so the *seam*'s contract stays uniform across backends even though Neo4j's own storage is flatter than every other backend's.
- Connects **eagerly** at construction (`rt.block_on(Graph::connect(...))` inside `new()`) rather than lazily like Postgres/Redis/Mongo — `neo4rs` has no documented lazy/unchecked pool-style constructor to defer the handshake to first use.

---

## Phase 24.3 — Cypher generation & atomic CAS 🔀 (Completed)

**Scope**
- One Cypher transaction per event (`neo4rs` transaction function — Neo4j transactions are ACID):
  1. CAS-check + upsert the guard node: `MERGE (g:__ConduitGuard {target: $t, entity_key: $k, facet: $f}) WITH g WHERE g.last_sequence IS NULL OR g.last_sequence < $seq SET g.last_sequence = $seq, ...` — if the `WHERE` filters it out, the transaction's write count is zero and the adapter treats it as `SkipStale`, mirroring the Postgres/Mongo CAS pattern.
  2. Node: `MERGE (n:{Label} {id: $id}) SET n += $properties` (full replace of the mapped properties; a named facet sets only its own property keys via `SET n += $facetProperties`, additive — same partial-update shape as Phase 15).
  3. Edge: `MATCH (a {id: $from}), (b {id: $to}) MERGE (a)-[r:{TYPE} {key: $edgeKey}]->(b) SET r += $properties`.
- `decide()` in Rust still drives *which* of insert/update/delete/skip happens — the Cypher `MERGE` is the write primitive, not the gate.

**Guarantees**
- Two writers racing the same node/edge serialize through Neo4j's own transaction conflict detection, same shape as Phase 23.3's Mongo guarantee.

**As built**
- **Important correctness deviation from Phase 23's approach:** MongoDB's multi-document transactions are snapshot-isolated, so Phase 23 could rely on the transaction's own conflict detection with no manual CAS filter. Neo4j's per-node write locks do **not** give the same guarantee — a transaction can still commit stale data it read before a concurrent transaction committed, since locks alone don't stop a *write* computed from an earlier *read*. So here, unlike Mongo, the `WHERE g.last_sequence IS NULL OR g.last_sequence < $seq` CAS in the `SET` clause is the actual correctness mechanism, not a redundant belt-and-suspenders — confirmed by reasoning through Neo4j's concurrency model, not just following the doc's sketch by default.
- Each CAS-checked write is one Cypher statement combining the guard `MERGE`/`WHERE`/`SET` with the node/edge `MERGE`/`SET`, executed via `Txn::execute` and checked for zero returned rows (`RETURN g.last_sequence`) — mirroring exactly how the doc's sketch reads, just implemented as a helper (`Neo4jBackend::cas_write`) shared by both node and edge writes.
- **Refinement over the doc's literal Cypher:** the default facet's write is `SET n = $properties, n.id = $id` (a true full replace, re-setting `id` afterward since `=` wipes every property not in the map) — not `SET n += $properties` as the doc's sketch shows. `+=` is additive and would leave stale properties behind after a mapping's fields change, which doesn't match "on_existing: replace" anywhere else in the codebase (SQL's full-column `SET`, and the file/KV/Document adapters' whole-file overwrite). A named facet's `SET n += $facetProperties` *is* additive, matching the doc and every other backend's shallow-merge facet semantics.
- Values convert `serde_json::Value` → `neo4rs::BoltType` via a small hand-written `json_to_bolt()` — `neo4rs` has no serde bridge for `BoltType` the way `bson` does for BSON, so this mirrors Phase 24's own JSON-scalar handling rather than Phase 19.3's Postgres column-typed binding (there's no schema to introspect here).
- Conflict detection: any `neo4rs::Error::Neo4j` carrying `Neo4jErrorKind::Transient` (deadlocks, lock timeouts) becomes `GraphError::WriteConflict` too, alongside the CAS's own zero-rows signal — the same "check the driver's own retryable-error classification" pattern Phase 23.3 used for MongoDB's `TransientTransactionError` label.

---

## Phase 24.4 — Native `DETACH DELETE` 🗑️ (Completed)

**Scope**
- A node delete uses Cypher's native `MATCH (n:{Label} {id: $id}) DETACH DELETE n` — Neo4j's own primitive removes the node **and every incident relationship** in one statement. Conduit no longer needs the file adapter's separate incident-index bookkeeping (Phase 16.2) — the graph structure *is* the incident index.
- Before the `DETACH DELETE`, the transaction reads the incident edges' `__ConduitGuard` nodes (`MATCH (n)-[r]-() MATCH (g:__ConduitGuard {...}) WHERE ...`) and tombstones each at the node-delete's sequence, matching Phase 16.4's cascade semantics exactly — the mechanism is native, the guarantee is identical.

**Guarantees**
- No dangling edges after a node delete, verified the same way as Phase 16.6's `graph_detach_delete` — this phase's version of that test targets Neo4j instead of files.

**As built**
- **Deviation from the doc's single-statement sketch, for reliability:** `delete_node_cascade` runs **four sequential statements** in the same transaction rather than one combined query — (A) a CAS-checked tombstone of the node's default-facet guard (the cascade's conflict-detection point), (B) an `UNWIND`-batched tombstone of every named-facet guard (if any), (C) a read collecting every incident edge's `(type, key)` pair via `MATCH (n)-[r]-()`, followed by an `UNWIND`-batched tombstone of those edges' guards, and (D) the `DETACH DELETE` itself. A single mega-query combining all of this needs `FOREACH`-based conditionals to avoid Cypher's `UNWIND`-on-an-empty-list-drops-the-row gotcha in several places at once; four small, individually-obvious statements were judged less likely to hide a subtle bug than one large one, at the cost of a few more round trips on a path (node delete) that isn't the hot path.
- Incident edges are found with a live `MATCH (n)-[r]-()` query — no incident-index bookkeeping at all, exactly as scoped ("the graph structure *is* the incident index").
- `neo4j_backend.rs`'s `neo4j_detach_delete` test (Phase 24.6) verifies this directly: 3 edges exist before delete, all 3 guards get tombstoned, all 3 relationships are gone, and the node itself is gone — not just "no dangling edges" asserted indirectly.

---

## Phase 24.5 — Capability & validation ✅ (Completed)

**Scope**
- `Neo4jAdapter` declares `AdapterCapability::{Write, Idempotent, Upsert, Delete, Transactions}`.
- `runtime/validation.rs`: a routed `neo4j` adapter needs a `GraphMapping` per routed event type; `uri` non-empty; a startup connectivity check (warn, not fail, on failure — consistent with Phase 19/23's pattern).

**Guarantees**
- No new validation mechanism.

**As built**
- `Neo4jAdapter` declares `Write, Idempotent, Upsert, Delete, Transactions` exactly as scoped.
- `runtime/validation.rs`: `is_graph()` extended to `matches!(a, AdapterConfig::Graph(_) | AdapterConfig::Neo4j(_))`, mirroring `is_sql`/`is_keyvalue`/`is_document`. A new `InvalidNeo4jConfig` mirrors the other three connection-config checks: `uri` non-empty, and a `ConfigBuilder::build()` parse check (the same construction primitive the adapter itself uses, without connecting).
- **Same deliberate omission as Phases 23.4/24.5's own scope text asks for and Phase 23 already declined:** no live connectivity/replica-set-style warning at validation time — this module has no non-fatal issue concept (every pushed `ValidationIssue` is fatal), so adding one here would contradict this same sub-phase's "no new validation mechanism" guarantee, exactly as documented in Phase 23.4's "As built" notes.

---

## Phase 24.6 — Verification 🧪 (Completed)

`testcontainers`-gated suite on a Neo4j image (skipped without Docker):

| Test | Asserts |
|---|---|
| `neo4j_node_lifecycle.rs` | Phase 16 node create/update/delete → node absent, guard tombstoned. |
| `neo4j_edge_out_of_order.rs` | Edge upsert sequences `3, 1, 2` → edge at seq-3 property state. |
| `neo4j_detach_delete.rs` | Node with 3 incident edges deleted → node and all 3 edges gone via native `DETACH DELETE`; each edge's guard tombstoned. |
| `neo4j_concurrent_writers.rs` | Two transactions race the same node → one commits, one retries then skips-stale. |
| `graph_cross_backend.rs` | The same stream into the file adapter and Neo4j → identical logical nodes/edges. |

**Guarantee under test:** Neo4j is behaviourally interchangeable with the file-backed graph adapter, and the detach-delete cascade is both simpler to implement (native primitive) and identically correct.

**As built**
- **Deviation:** all five scenarios live in **one consolidated file, `crates/conduit-core/tests/neo4j_backend.rs`**, sharing one skip guard and a `fresh_label()` helper — mirroring `pg_backend.rs` (19.6), `redis_backend.rs` (22.5), and `mongo_backend.rs` (23.5).
- **Deviation:** gated on **`CONDUIT_NEO4J_URI`** (plus `CONDUIT_NEO4J_USER`/`CONDUIT_NEO4J_PASSWORD`, defaulting to `neo4j`/`conduit-test`), not `testcontainers` — the same now-four-times-documented deviation between this doc's aspirational `testcontainers` wording and every backend's actual env-var-gated implementation (Phases 19.6, 22.5, 23.5). CI's new `test-neo4j` job uses a plain `services:` container (`neo4j:5` with `NEO4J_AUTH=neo4j/conduit-test`) — unlike MongoDB's replica set, Neo4j's official image needs no command-line override for this phase's needs, so the standard `services:` block (with a `cypher-shell`-based health check) is enough, no `docker run` workaround required.
- Verification queries in the test file use `neo4rs` directly against the running server (a throwaway `tokio::runtime::Runtime`, separate from the adapter's own internal one) — the same "reach into the driver crate directly for assertions" pattern `pg_backend.rs`/`mongo_backend.rs` already use.
- `neo4j_concurrent_writers` races two real OS threads against a shared `Arc<Neo4jAdapter>`, the same shape as Phases 22.5/23.5's own concurrent-writer tests.
- **Not verified against a live Neo4j in the implementing environment** — no Docker daemon available (the same limitation already documented for Phase 19's Postgres example, Phase 21.6's UDS listener, and Phases 22–23's own backend suites). All five tests were confirmed to compile and skip cleanly (`SKIP: CONDUIT_NEO4J_URI not set`) with `cargo test -p conduit-core --test neo4j_backend`; the real Cypher generation, CAS logic, and detach-delete cascade run for real only under CI's `test-neo4j` job. Given how much of this phase's design (the guard-placement contradiction, the CAS-vs-transaction-isolation reasoning, the four-statement cascade) rests on API research rather than empirical testing, this phase carries more residual risk of a CI-caught bug than Phases 22–23 did — consistent with the doc's own framing of Neo4j as "the biggest lift" of the four backends.

---

## Non-Goals (explicit)

- **Not SQL, KV, or Document.** One backend, one kind — Graph only, and the last of the four.
- **No Cypher query/read API exposed.** Conduit writes the graph; traversal, pattern matching, and graph algorithms stay the application's job (Phase 16's non-goal, unchanged).
- **No APOC procedures** or Neo4j-plugin-specific features.
- **No multi-database Neo4j fan-out** beyond the one `database` a `Neo4jAdapter` instance targets — multiple databases means multiple adapter configs, same as multiple SQL adapters today.
- **No graph algorithms, indexes beyond the guard uniqueness constraint, or schema management** beyond what Conduit needs for its own guard.
