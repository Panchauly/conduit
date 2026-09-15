# Conduit — Architecture

## 1. Overview

**Conduit** is an **event-first projection engine** that deterministically applies events to one or more storage models (SQL, document, key-value, graph) using **explicit mappings** and **strict runtime semantics**.

Conduit is designed to be:

* deterministic
* explicit
* replay-safe
* side-effect aware

It is intentionally **not** a streaming platform, workflow engine, or database.

---

## 1.1 Scope boundary

Conduit's job is **projection only**. It takes an event payload, resolves where it goes, verifies it is safe to write, orders execution deterministically, and projects it into the target storage models without corrupting state. It does **not** produce, ingest, buffer, or store events — producing the event log is out of scope. (An event-store substrate that would own the log was considered and rejected; Conduit consumes a log it does not own.)

| Owner | Responsibilities |
|-------|------------------|
| **Application / command side** | business logic, command validation, state transitions, and emitting events — whether by synchronously invoking Conduit as a library or by pushing to a socket / broker |
| **Transport / external world** | durability, wire protocols, network transport, and consumer offsets — everything behind the `EventSource` trait |
| **Conduit** | resolve where an event goes (routing), verify it is safe to write (validation), order execution deterministically (dependency graph), project it into target storage models without corrupting state (`decide()` + per-lane guards) |

Everything Conduit relies on from the application and transport — a monotonic per-entity `sequence`, a stable canonical entity id, per-entity delivery order — it assumes and cannot verify. Those assumptions are catalogued in [`phases/producer-contract.md`](phases/producer-contract.md).

---

## 1.2 The three parts

Conduit is three parts, and each has a different dependency rule:

| Part | What it is | Dependency rule |
|------|-----------|-----------------|
| **1. Storage adapters** | speak each target store's own wire protocol (SQLite, Postgres, Redis, MongoDB, Neo4j) | **Dependencies are forced** — the client already runs these databases; Conduit must speak them |
| **2. Mapping & verification** | declarative YAML — routing, mappings, capabilities, versions — validated before runtime | **Technology-agnostic, dependency-free** — `serde` over YAML, no engine lock-in |
| **3. Producer ingestion** | how events reach the engine | **No technology lock-in** — Conduit's own contract (a gRPC `.proto`); never "you must use Kafka / a database / a broker" |

Part 3's ingestion contract is Conduit-defined and neutral. Concrete transports — a gRPC stream, a file directory, later Kafka or Kinesis — are `EventSource` implementations behind that contract; none is privileged and none is a core dependency.

---

## 1.3 Deployment modes

| Mode | How events arrive | Use |
|------|-------------------|-----|
| **Embedded crate** — `conduit-core` linked into a larger service | direct function call (`execute_event`) — no protocol | the host service already owns the command path and wants projection in-process |
| **Sidecar binary** — one statically-linked `conduit` process (e.g. a Kubernetes pod sidecar) | the gRPC ingestion service (Unix domain socket in-pod, or TCP); the app streams events via generated stubs | language-agnostic; the app and Conduit are decoupled processes |

Both modes run the identical core engine. Ingestion is stateless — Conduit acks a position after each batch's guards commit; the producer owns replay from the last ack.

---

## 1.4 Distribution (open-core)

- **Open source (MIT / Apache-2.0):** the core engine and `conduit-core` crate; the gRPC ingestion service, its `.proto` contract, and a reference client; the `file` / `stdin` / `directory` sources; the SQL, document, key-value, and graph adapters — all four with a real backend (file-backed, plus SQLite and Postgres for SQL, Redis for key-value, MongoDB for document, and Neo4j for graph).
- **Pro / Enterprise:** managed cloud connectors (native Kafka / Kinesis `EventSource` implementations), a distributed multi-node coordinator, compliance/audit encryption adapters, and a visual UI for `conduit explain` topology and execution debugging.

Both tiers target the same `EventSource` trait and the same engine API. The `.proto` is the public ingestion contract regardless of tier.

---

## 2. Core Architectural Principles

### 2.1 Event-First

Events are the **only input** to Conduit.

* Routing decisions are based on `event_type`
* Storage adapters never inspect transport metadata
* No implicit behavior based on payload shape

```text
event → route → adapters → side effects
```

---

### 2.2 Explicit Mappings

All projections are declared explicitly:

* SQL mappings define tables, columns, and keys
* Document mappings define collection-level structure
* Key-value mappings define a namespace, key path, and value template
* Graph mappings define a node (`label` + `key`) or an edge (`edge_type` + `from`/`to`)

There is:

* no schema inference
* no auto-migration
* no dynamic interpretation

Configuration errors are surfaced **before runtime**.

---

### 2.3 Deterministic Execution

Given:

* the same event
* the same mappings
* the same routing rules

Conduit will:

* select the same adapters
* execute them in the same order
* produce the same side effects

Determinism is a **hard requirement**, not a best effort.

---

## 3. High-Level Components

### 3.0 Runtime Factory

Adapter construction is centralized in a **runtime factory** (`runtime::factory`).

Responsibilities:

* assemble concrete adapters from validated mappings
* own adapter defaults (paths, priorities)
* ensure single ownership of runtime builders

Non-responsibilities:

* routing decisions
* runtime execution
* dynamic configuration

The factory is a **composition boundary only**; it introduces no new behavior and does not affect runtime semantics.

---

### 3.1 CLI Layer

The CLI is a **control surface**, not a runtime container.

Commands:

* `run` — real execution: one-shot with `--event <file>`, or the continuous source loop (`--once` to drain and exit) when the config declares `sources:` (Phase 17)
* `sources` — list configured sources and their committed positions
* `explain` — routing + dependency order for one event, no execution
* `dry-run` — full simulated execution, no writes
* `replay` — drain a directory / NDJSON file once (superseded by `run --once`; kept for compatibility)

The CLI performs:

* path resolution
* config loading
* error classification

It does **not** perform business logic (thin-CLI pattern — all engine logic lives in `conduit-core`).

---

### 3.2 Routing

Routing is:

* pure
* deterministic
* event-type driven

Events route by `event_type` through a config-backed routing table to a list of **adapter ids**, not storage kinds — one event type can target several adapters of the same kind (e.g. a master and a replica):

```rust
fn route_with_rules(event: &Event, rules: &HashMap<String, Vec<AdapterId>>) -> Vec<AdapterId>
```

Routing does **not**:

* read mappings
* inspect payloads
* depend on adapter availability

---

### 3.3 Dispatch

Dispatch is responsible for:

* ordering adapters by priority
* invoking adapters sequentially
* collecting results

Dispatch semantics:

* adapters run in dependency order (Kahn's algorithm over `depends_on`, then priority — Phase 9), not just priority
* on failure, `FailurePolicy` decides: `fail_fast` (default) stops the batch; `continue_on_error` runs every routed adapter
* downstream adapters of a failed dependency are skipped
* results are always reported

There is **no rollback** and **no cross-adapter transaction**.

---

## 4. Adapters

### 4.1 Adapter Contract

All adapters implement the same interface:

```rust
trait StorageAdapter {
    fn kind(&self) -> StorageKind;
    fn id(&self) -> &str;
    fn priority(&self) -> u32;
    fn handle(&self, event: &Event) -> AdapterResult;
}
```

Adapters are:

* synchronous
* blocking
* side-effecting

Every adapter's write is gated by one shared pure function, `decide()` (`adapter/mod.rs`): given the `Operation` (`upsert` / `delete`), the mapping's `on_existing` mode, the persisted guard state for that entity's lane (`last_sequence`, `deleted`, `permanent`), and the event's `sequence`, it returns a `WriteDecision` (`Insert` / `Update` / `Delete` / one of the skip variants). The four adapters call it unchanged — sequence gating, redelivery, tombstones, and resurrection are decided once, not four times.

The result is an `AdapterResult` carrying an `AdapterOutcome` — `Created` / `Updated` / `Deleted` / `Skipped(SkipReason)` / `Failed(AdapterError)` (Phase 12.1). A skip is not a failure; `SkipReason` (`AlreadyProjected`, `StaleSequence`, `AlreadyDeleted`, `Tombstoned`, `EntityAbsent`, `UnsupportedVersion`) is machine-readable and round-trips into the JSON execution report.

---

### 4.2 SQL Adapter

Responsibilities:

* build SQL from mappings
* execute exactly one statement per event
* manage its own database connection

Non-responsibilities:

* migrations
* retries
* cross-adapter transactions

Idempotency:

* enforced via the `conduit_projection_state` guard table, PK `(target_table, entity_key, facet)` (Phases 11 / 15)
* stored in the same database, atomic with projection execution
* `on_existing: replace` builds `INSERT … ON CONFLICT (<pk>) DO UPDATE SET …` (Phase 12); a named facet restricts the `SET` list to its own columns (Phase 15)

---

### 4.3 Document Adapter (File-Based)

Responsibilities:

* build document JSON via mappings
* determine filesystem layout — output keyed by `(collection, entity_id)` (Phase 13.3), not `event_type`
* write files deterministically via write-to-temp-then-rename

Idempotency:

* enforced via a per-entity JSON guard sidecar (`last_sequence` / `deleted` / `permanent`, plus a `facets` map — Phases 11 / 13 / 15)
* adapter-local state only; single-writer assumption

---

### 4.4 Key-Value Adapter (File-Based) — Phase 14

A flat namespace → key → value store: `{root}/{namespace}/{key}.json` plus a guard sidecar. The simplest adapter — no query language, no schema — added as a generalization test for the abstraction. Same `decide()`, same guard shape, `facets` on the value.

---

### 4.5 Graph Adapter (File-Based) — Phase 16

A directed property graph. Nodes and edges are *separately keyed records*, each an independent `decide()` lane: a node keyed by `node_id`, an edge by the canonical `[edge_type, from, to(, discriminator)]`. `operation: delete` on a node is a `DETACH DELETE` — it removes the node and every incident edge (tracked in a per-node incident index) in one adapter operation. Nodes are entities (facets apply); edges are not.

---

## 5. Builders

### 5.1 Runtime Builders

Builders are **pure projection engines**.

Example:

```rust
DocumentRuntimeBuilder::build(event) -> serde_json::Value
```

Builders:

* do not perform I/O
* do not resolve paths
* do not manage idempotency

They are intentionally opaque.

---

## 6. Idempotency Model (Phases 11–17)

### 6.1 Definition

Idempotency is **entity-keyed and sequence-gated**, not `event_id`-keyed (Phase 11 superseded the Phase 4 `conduit_events` / event-id model).

A guard lane is `(target, entity_key, facet)`. Its state is `{ last_sequence, deleted, permanent }`. For each event, `decide()` compares the event's `sequence` to `last_sequence`:

* a **redelivery or superseded** event (`sequence <= last_sequence`) is `Skipped(StaleSequence)` — no write
* a **newer** `upsert` under `on_existing: replace` overwrites; under `ignore` an already-projected entity is `Skipped(AlreadyProjected)`
* a **newer** `delete` writes a tombstone (`deleted = true`); `permanent` tombstones reject every later event forever (Phase 13)
* a newer `upsert` on a non-permanent tombstone **resurrects** the entity (Phase 13.4)

---

### 6.2 Scope

* idempotency is adapter-local — no global coordination, no exactly-once *delivery*
* combined with an at-least-once source and a committed checkpoint, it yields **effectively-once projection** (Phase 17): the state after any sequence of crashes / restarts / redeliveries equals a single clean pass
* each adapter enforces the guard using storage-native mechanisms (a SQL table row, a JSON sidecar)

---

### 6.3 Failure Semantics

If an adapter:

* fails before recording the guard → retry is possible (the source re-polls the uncommitted batch)
* records the guard → replay of that event becomes a `Skipped(*)` no-op

There is no attempt to reconcile partial success across adapters within one event; the source-loop retry / DLQ / halt decision is across *batches* (Phase 17.5).

---

## 7. Error Model

### 7.1 Adapter Results

Each adapter returns:

```rust
struct AdapterResult {
    adapter_id: String,
    kind: StorageKind,
    outcome: AdapterOutcome, // Created | Updated | Deleted | Skipped(SkipReason) | Failed(AdapterError)
    source_version: Option<u32>,
    projected_version: Option<u32>,
}
```

(Phase 12.1 replaced the `success: bool` + skip-message encoding with the `AdapterOutcome` enum.)

Errors are **observable and explicit**.

---

### 7.2 Outcome & Error Types

* `Created` / `Updated` / `Deleted` — a write happened
* `Skipped(SkipReason)` — no write; `SkipReason` says why (stale, already projected, tombstoned, entity absent, unsupported version)
* `Failed(AdapterError)` — `WriteFailed` or `UnsupportedVersion`

Errors are never swallowed. Skips are not errors.

---

## 8. Phase Boundaries (Intentional Limitations)

Conduit explicitly does **not** provide:

* distributed transactions or cross-adapter rollback
* exactly-once *delivery* (it provides effectively-once *projection* via idempotent sinks)
* async execution
* an event store / broker — it consumes a log it does not own (§1.1)
* schema inference or auto-migration
* backpressure, rate limiting, or flow control
* distributed / multi-instance coordination — one projector owns its sources' positions
* implicit behavior

The Phase 17 `run` loop does across-*batch* retry (with a bounded budget) and a DLQ for poison events — this is the one deliberate step past "no retries", scoped to the source-consumption boundary. These are architectural constraints, not missing features.

---

## 9. Design Philosophy

Conduit prioritizes:

* correctness over convenience
* explicitness over magic
* local reasoning over global coordination

Every design decision is evaluated against one question:

> **Can the behavior be explained deterministically by reading the code and configuration?**

If the answer is no, the feature does not belong in Conduit.

---

## 10. Status

* Phases 1–18 complete (see [`phases.md`](phases.md))
* Four storage adapters (SQL, Document, Key-Value, Graph) + two event sources (directory, stdin), all file-backed
* Core architecture locked; future phases must preserve existing semantics and the `decide()` contract

This document defines the **architectural contract** of Conduit.
