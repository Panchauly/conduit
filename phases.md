# Conduit — Project Phases

This document tracks **completed progress (Phase 1–Phase 4)** for the Conduit project.

---

## Phase 1 — CLI & Validation (COMPLETED)

### Objective

Provide a safe, deterministic command-line interface to validate configuration and inspect behavior **without executing writes**.

### Status

**Phase 1: ✅ COMPLETE**

### Deliverables

* `conduit` CLI using `clap`
* Explicit path-based configuration
* Commands:

  * `conduit validate`
  * `conduit test-event`
* Deterministic routing
* Strict schema validation
* Zero side effects

---

## Phase 2 — Runtime Engine (COMPLETED)

### Objective

Execute events through adapters in a **controlled, deterministic runtime**, with explicit failure semantics.

### Status

**Phase 2: ✅ COMPLETE**

### Deliverables

* `conduit run` command
* Sequential adapter execution
* Aggregated execution results
* Deterministic exit codes
* Validation reused from Phase 1

### Exit Code Contract

| Scenario                | Exit Code |
| ----------------------- | --------- |
| All adapters succeeded  | `0`       |
| Partial adapter failure | `2`       |
| Configuration invalid   | `1`       |
| Event invalid           | `3`       |

---

## Phase 3 — Real Adapters & Failure Semantics (COMPLETED)

### Objective

Introduce **real side effects** while preserving Conduit’s core guarantees.

### Status

**Phase 3: ✅ COMPLETE**

### Delivered

* Real SQL adapter (SQLite, blocking)
* Real document adapter (file-based)
* Strict `StorageAdapter` contract
* Explicit execution report
* No retries, no rollback, no transactions

### Failure Semantics

| Case                  | Behavior                   |
| --------------------- | -------------------------- |
| Adapter fails         | Execution continues        |
| Panic                 | Treated as adapter failure |
| Side effect committed | Never rolled back          |

### Exit Criteria (Met)

* Real database writes occur
* Adapter contract enforced
* Failures explicit and observable

---

## Phase 4 — Operational Hardening (COMPLETED)

### Objective

Make Conduit **safe for replays and repeated execution** without changing Phase 3 runtime semantics.

### Status

**Phase 4: ✅ COMPLETE**

---

### 1. Idempotency (Adapter-Local)

#### Goal

Ensure reprocessing the same event does **not** create duplicate side effects.

#### Design

* Idempotency is **adapter-local**
* Uses `event_id` as the guard key
* No global coordination
* No distributed state

##### SQL Adapter

* Guard table stored in the **same database** as user tables

```sql
CREATE TABLE IF NOT EXISTS conduit_events (
  event_id TEXT PRIMARY KEY,
  processed_at TEXT NOT NULL
);
```

Execution rules:

1. Check `conduit_events` for `event_id`
2. If present → skip projection (success)
3. Else → execute projection + insert guard
4. Guard + write occur in a **single transaction**

##### Document Adapter

* Guard stored in adapter-owned directory (e.g. `.conduit/events/`)
* Presence of guard file → skip write

---

### 2. Adapter Configuration Discipline

* Adapters remain explicitly constructed
* No implicit defaults
* No runtime inference
* Validation remains fail-fast

(Structured config schemas reserved for future phases)

---

### 3. Execution Observability

* Adapter results remain explicit
* Idempotent skips are observable via `AdapterError::Skipped`
* No behavior change to exit codes

---

### Phase 4 Exit Criteria (Met)

* Adapter-local idempotency implemented
* SQL idempotency atomic and replay-safe
* Phase 3 semantics preserved
* No retries, no async, no coordination added

---

## Phase Boundaries (Locked)

Conduit **intentionally does NOT provide**:

* Distributed transactions
* Exactly-once delivery
* Retries or backoff
* Async execution
* Service / daemon mode
* Global coordination database

---

*Last updated: Phase 4 completed (Idempotency & operational hardening)*
