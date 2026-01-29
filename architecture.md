# Conduit — Architecture

## 1. Overview

**Conduit** is an **event-first projection engine** that deterministically applies events to one or more storage models (SQL, document, etc.) using **explicit mappings** and **strict runtime semantics**.

Conduit is designed to be:

* deterministic
* explicit
* replay-safe
* side-effect aware

It is intentionally **not** a streaming platform, workflow engine, or database.

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

## 3. High-Level Components

### 3.1 CLI Layer

The CLI is a **control surface**, not a runtime container.

Commands:

* `validate` — configuration validation only
* `test-event` — deterministic preview, no writes
* `run` — real execution with side effects

The CLI performs:

* path resolution
* config loading
* error classification

It does **not** perform business logic.

---

### 3.2 Routing

Routing is:

* pure
* deterministic
* event-type driven

```rust
fn route(event: &Event) -> Vec<StorageKind>
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

* adapters are isolated
* failures do not stop execution
* results are always reported

There is **no rollback** and **no coordination** between adapters.

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

Idempotency (Phase 4):

* enforced via a local guard table
* stored in the same database
* atomic with projection execution

---

### 4.3 Document Adapter (File-Based)

Responsibilities:

* build document JSON via mappings
* determine filesystem layout
* write files deterministically

Idempotency (Phase 4):

* enforced via filesystem guard files
* adapter-local state only
* no shared coordination

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

## 6. Idempotency Model (Phase 4)

### 6.1 Definition

Idempotency is **event-level**, not row-level.

Processing the same `event_id` multiple times:

* must not duplicate side effects
* must return success

---

### 6.2 Scope

* idempotency is adapter-local
* no global coordination
* no exactly-once guarantees

Each adapter enforces idempotency using **storage-native mechanisms**.

---

### 6.3 Failure Semantics

If an adapter:

* fails before recording the guard → retry is possible
* records the guard → replay becomes a no-op

There is no attempt to reconcile partial success across adapters.

---

## 7. Error Model

### 7.1 Adapter Results

Each adapter returns:

```rust
struct AdapterResult {
    adapter_id: String,
    kind: StorageKind,
    success: bool,
    error: Option<AdapterError>,
}
```

Errors are **observable and explicit**.

---

### 7.2 Error Types

* `WriteFailed` — side effect failed
* `Skipped` — idempotent replay

Errors are never swallowed.

---

## 8. Phase Boundaries (Intentional Limitations)

Conduit explicitly does **not** provide:

* distributed transactions
* exactly-once delivery
* retries or backoff
* async execution
* service / daemon mode
* schema inference
* implicit behavior

These are architectural constraints, not missing features.

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

* Phase 1–4 complete
* Core architecture locked
* Future phases must preserve existing semantics

This document defines the **architectural contract** of Conduit.
