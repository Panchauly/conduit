# The Producer Contract

Conduit is projection-only ([`architecture.md` §1.1](../architecture.md)). It consumes an event log it does not own, and its determinism / idempotency / replay guarantees hold **only if the events it receives satisfy the assumptions below**. Conduit sees one event at a time and cannot verify any of them — they are the application's and transport's responsibility.

This file consolidates assumptions that were introduced piecemeal across Phases 11–17.

---

## 1. `Event.sequence` — monotonic per entity

**Assumption.** For successive events about the *same* entity, `sequence` strictly increases. A per-aggregate version or a global monotonic log offset (Kafka offset, commit position) both satisfy this. Different entities have independent sequences.

- Gaps are fine (`1, 2, 7, 50`) — the gate needs ordering, not contiguity.
- It is **not** the event timestamp — wall-clock time is not monotonic under clock skew and can collide.
- It is unrelated to `version` (payload *schema* version, for upcasting). An entity on schema `version 3` may be at `sequence 47`.

**If violated.** Two distinct events sharing a sequence → the later-delivered one is judged stale and dropped (silent loss). A decreasing sequence → the "newer" update is rejected as stale.

**Introduced:** Phase 11 (field), Phase 12 (gating).

---

## 2. Entity id — stable, canonical, scalar

**Assumption.** The `key` / `id` / `primary_key` path resolves to a JSON scalar, and the producer emits it in **one canonical textual form** for a given entity across its whole lifetime.

**If violated.** `550e8400-…` in one event and `{550E8400-…}` in another are two different `entity_key`s → two guard lanes → the "update" inserts a second entity instead of updating the first. Silent.

**Introduced:** Phase 11.1 (scalar requirement, canonical encoding), Phase 12 (guard lookup by raw string).

---

## 3. Per-entity delivery order

**Assumption.** Each entity's events reach Conduit in sequence order.

- The replay / batch sort tolerates *local* reordering within a batch — it sorts by `sequence` before dispatch (Phase 15.5).
- It does **not** tolerate: an event permanently dropped; or, under `on_existing: ignore`, a lower-sequence event arriving *after* that entity's lane has already advanced (the first-delivered event wins, so a late "create" is skipped and its data never lands).
- `on_existing: replace` and facet lanes are order-independent *within a lane* — highest sequence wins — but a facet update delivered before the entity's create is skipped (`EntityAbsent`) in a live feed.

**If violated.** Lost updates (ignore mode) or skipped facet writes (out-of-order create).

**Introduced:** Phase 12 (gate), Phase 15 (facets, sequence-ordered replay).

---

## 4. One entity, one source

**Assumption.** An entity's events all arrive through a single `EventSource`. There is no ordering guarantee *across* sources.

**If violated.** Two sources feeding the same entity can interleave arbitrarily; the sequence gate still prevents corruption but convergence is not guaranteed if the sources' positions advance independently.

**Introduced:** Phase 17.

---

## 5. Delete route ⊇ create route

**Assumption.** A `delete` event (e.g. `OrderCancelled`) routes to *at least* the same adapters its `create` event (`OrderCreated`) routed to.

**If violated.** The entity is removed from some stores and left in others. Conduit routes by `event_type` and cannot compare the two routes.

**Introduced:** Phase 13.5.

---

## 6. Fat events, not deltas

**Assumption.** Every mapped value is fully resolved from the event payload / metadata. An event that logically increments a value carries the *computed result* (`new_balance: 150`), never the delta (`amount: +50`).

**If violated.** Conduit has no read-modify-write path; a delta would be written literally as the new value.

**Introduced:** Phase 12 (design position), Phase 15 (restated for facets).

---

## 7. Graph endpoint order (file-backed adapter)

**Assumption.** If edges reference nodes, the producer emits the node before edges that point at it — *when the target graph backend enforces endpoint existence*. The file-backed adapter is permissive (opaque node ids); a future enforcing backend would emit `Skipped(EndpointAbsent)`.

**Introduced:** Phase 16.3.

---

## 8. Atomic file delivery (directory source)

**Assumption.** A producer writing event files into a watched directory writes to a temp name and `rename`s the finished file in — never appends to a file Conduit may already be reading.

**If violated.** The directory source reads a whole file at once; a half-written trailing NDJSON line is a parse error → that line is DLQ'd, the rest of the file continues.

**Introduced:** Phase 17 (NDJSON handling).
