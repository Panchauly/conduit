# Conduit

**Conduit** is an event-first projection engine that deterministically projects events into multiple storage models using explicit, schema-driven mappings.

It is designed for backend systems where the **same event must be safely and consistently projected into different stores** — SQL, document, key-value, and graph adapters all exist (file-backed) — without duplication of logic or hidden behavior. Conduit is **projection-only**: it consumes an event log it does not own and does not produce, buffer, or store events (see [`architecture.md` §1.1](architecture.md)).

---

## Why Conduit Exists

In many systems, a single event needs to be written to:
- a SQL database (transactions)
- a document store (read models)
- a key-value store / cache (fast access)
- a graph (relationships)

This logic is often:
- duplicated across services
- inconsistently implemented
- difficult to validate
- hard to reason about when failures occur

Conduit centralizes this responsibility with **deterministic routing**, **explicit mappings**, and **strict validation**.

---

## Core Principles

- **Event-first** — routing is based on event type, not metadata heuristics
- **Deterministic** — no implicit behavior, no magic
- **Schema-explicit** — mappings are declared, not inferred
- **Fail fast** — configuration errors are caught at startup
- **Storage-agnostic** — adapters define *how*, not *what*

---

## What Conduit Is

- A **projection engine** for event-driven systems
- A **library + CLI** (service optional later)
- A tool for **backend / infrastructure engineers**
- A foundation for multi-model data architectures (CQRS, event sourcing)

---

## What Conduit Is NOT

- ❌ Not a streaming platform (Kafka alternative)
- ❌ Not an ETL / ELT tool
- ❌ Not a workflow engine
- ❌ Not a database
- ❌ Not a schema inference system

Conduit prefers **explicitness over convenience**.

---

## High-Level Architecture

See [`architecture.md`](architecture.md) for the full contract, [`phases.md`](phases.md) for the phase-by-phase evolution, and [`phases/producer-contract.md`](phases/producer-contract.md) for the assumptions Conduit depends on but cannot verify.

`event → route (by event_type) → adapters (in dependency order) → decide() gate → storage`

---

## Version Status

Latest release notes: **[`RELEASES/v0.6.0.md`](RELEASES/v0.6.0.md)** (Phases 11–17).

Conduit is in an **alpha** phase. Core architecture and execution semantics are stable, but public APIs may evolve.

This release is intended for early feedback, not production deployment.
