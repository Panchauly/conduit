# Phase 16 — Graph Adapter (Nodes & Edges) 🎯 (Completed)

**Goal:** The fourth and last storage kind named in the project pitch — a directed property graph — projected through the *same* `StorageAdapter` trait, `decide()` core, sequence-gated guard, facet model, and capability checks as SQL, Document, and Key-Value. This is the hardest generalisation test: a graph's natural unit is not a single keyed row.

**Why:**
- `StorageKind::Graph` already exists (`routing.rs:18`, `Display` → `"graph"`) with nothing behind it — the last half-wired kind.
- Cache (Phase 14) proved the abstraction survives a second *simple* adapter. Graph is the adversarial case: it has two record shapes (nodes, edges), edges carry a composite identity and reference other records, and deleting a node has consequences for records it doesn't key.
- If the model holds here with no change to `dispatch.rs` / `replay.rs` / `dependency_graph.rs` / `decide()` / `AdapterOutcome`, the adapter abstraction is done. If it doesn't, this phase is where the leak is found and named.

### The framing decision: records, not subgraphs

Everything Conduit projects is **one keyed entity per mapping**. A graph fits this if — and only if — nodes and edges are treated as *separately keyed records*:

| Record | Identity (`entity_key`) | Precedent |
|---|---|---|
| Node | `node_id` (scalar) | exactly like a SQL row / KV value |
| Edge | canonical `[edge_type, from, to]` (+ optional `discriminator`) | Phase 11.1 composite key |

One `GraphMapping` emits **exactly one** record — a node or an edge — keyed, sequence-gated, and `decide()`-driven identically to every prior adapter. A single event that must touch several graph records (`OrderPlaced` → an `Order` node + a `PLACED_BY` edge + N `CONTAINS` edges) is **out of scope**: decompose it into per-record events (`OrderCreated`, `OrderLineAdded`) upstream, or wait for a future "subgraph per event" phase. Most social / recommendation / org-chart graphs are already single-record per event; the order-graph shape is the exception.

---

## Phase 16.1 — Config & mapping 🕸️ (Completed)

**Scope**
- `AdapterConfig` gains `#[serde(rename = "graph")]` → `GraphAdapterConfig` (`id`, `priority`, `config: { root: String }`, `capabilities`, `depends_on`) — file-backed, mirroring `File` / `KeyValue`. A Cypher / Neo4j backend is a later phase, the way Postgres is for SQL.
- New `crates/conduit-core/src/adapter/graph/` module: `mod.rs`, `adapter.rs`, `mapping.rs`, `loader.rs`, `runtime.rs`, `preview.rs`, `store.rs`.
- `GraphMapping` — a tagged enum on `kind`:

  ```yaml
  # node mapping
  kind: node
  label: User
  version: 1
  key: payload.user_id                 # → node_id, must resolve to a scalar
  properties: { name: payload.name, tier: payload.tier }
  operation: upsert                    # upsert (default) | delete   — Phase 13.1
  on_existing: replace                 # ignore (default) | replace  — Phase 12.2
  facet: profile                       # optional — Phase 15, nodes are entities
  permanent: false
  ```

  ```yaml
  # edge mapping
  kind: edge
  edge_type: FOLLOWS
  version: 1
  from: payload.follower_id            # → endpoint node_id, scalar
  to:   payload.followee_id            # → endpoint node_id, scalar
  discriminator: payload.rating_id     # optional 4th key component for parallel edges
  properties: { since: payload.ts }
  operation: upsert
  on_existing: replace
  ```

- `load_graph_mappings` — YAML dir loader, one mapping per event type, same shape as the others.

**Guarantees**
- Node and edge mappings reuse `operation` / `on_existing` / `permanent` / `version` / `facet` with their Phase 11–15 meanings — no graph-specific mapping verbs beyond `kind`, `label`/`edge_type`, and `from`/`to`/`discriminator`.
- Edge identity defaults to `[edge_type, from, to]`; a multigraph (parallel edges between one ordered pair) adds `discriminator` → `[edge_type, from, to, discriminator]`.

**As built**
- `GraphMapping` is `#[serde(tag = "kind", rename_all = "snake_case")]` over `GraphNodeMapping` / `GraphEdgeMapping`, each carrying `event` + the shared vocabulary. `GraphMapping::event()/version()/operation()/on_existing()/permanent()/facet_key()` accessors; `resolve(event) -> ResolvedRecord`.
- `canonical_edge_key` = `serde_json::to_string(&[edge_type, from, to(, disc)])` (Phase 11.1 encoding). `encode_edge_key` = lowercase hex of its bytes — filesystem-safe, since the canonical key contains `"` `[` `,` which are illegal in filenames on Windows.
- An edge with no `properties` is a valid bare relationship (only a node upsert requires non-empty properties; a `delete` mapping of either kind must carry none).
- Internal API signature change: `execute_event*`, `ReplayContext::new*`, `replay_stream*`, `validate_projection_config`, `build_adapters_from_config` all gain a `graph_mappings` parameter. `conduit-cli` needed no change (it only goes through `pipeline.rs`). `<mappings_dir>/graph` is **optional** like `keyvalue/`.

---

## Phase 16.2 — File-backed store & guard 💾 (Completed)

**Scope**
- Node value: `{root}/nodes/{label}/{node_id}.json`. Edge value: `{root}/edges/{edge_type}/{encoded key}.json` (the canonical composite key, filesystem-safe-encoded).
- Guard: `{root}/.conduit/guard/nodes/{label}/{node_id}.json` and `{root}/.conduit/guard/edges/{edge_type}/{encoded key}.json` — `{ last_sequence, deleted, permanent }` (+ the `facets` map on nodes, Phase 15), structurally identical to the Document / KV sidecar.
- **Incident-edge index:** `{root}/.conduit/incident/{node_id}.json` → the set of edge keys touching that node, maintained on every edge write / delete. Used only by the node-delete detach cascade (16.4).
- All writes (value, guard, index) via write-temp-then-rename. Single-writer assumption, unchanged from Phase 12.4 / 14.2.
- `decide()` (`adapter/mod.rs`) is called **unchanged** for both record kinds — same `GuardState`, same `WriteDecision`.

**Guarantees**
- Each node and each edge is an independent Phase 12/13 state machine: highest sequence in that record's lane wins, redelivery is a no-op, resurrection and permanent tombstones inherited.
- Disjoint records never interfere — an edge write and a node write for the same `node_id` are separate lanes.

**As built**
- One `GraphGuard` struct (default facet in flat fields + `facets` map, Phase 15 shape) serves both record kinds; edges never populate `facets`.
- `decide()` is called unchanged from `adapter/mod.rs` for both kinds — nodes pass `Replace` for a named-facet lane (Phase 15's rule), edges pass the mapping's declared `on_existing`.

---

## Phase 16.3 — Node & edge upsert 🔗 (Completed)

**Scope**
- Node upsert: identical to the KV / document path — resolve `key` + `properties`, `decide()` on the node's lane, write the value + guard. Facets on nodes are Phase 15 verbatim (a node *is* an entity).
- Edge upsert: resolve `from` / `to` / optional `discriminator`, build the canonical key (Phase 11.1 encoding), `decide()` on the edge's lane, write value + guard, update the incident index for both endpoints.
- **Endpoint existence is not enforced.** An edge may reference a `node_id` that has never been projected — the file-backed store treats node ids as opaque. Whether dangling edges are acceptable is a producer/backend concern: a future Cypher backend that requires `MATCH` on both endpoints would enforce it and emit `Skipped(EndpointAbsent)`; the file adapter is permissive and documents the contract ("emit the node before edges that reference it if your backend enforces endpoints").

**Guarantees**
- An edge's projected properties are the highest-sequence value for `[edge_type, from, to(, discriminator)]`.
- Re-emitting `UserFollowed { a, b }` with a new `event_id` → `Skipped(*)` on the edge's lane, byte-identical.

**As built**
- A node id that was **never projected** → the edge is written (permissive, per this section). A node id that is **explicitly tombstoned** → the edge upsert is `Skipped(EntityAbsent)` (see 16.4 "As built"). These are not contradictory: 16.3's permissiveness is about absent ids, 16.4's DETACH DELETE is about deleted nodes.

---

## Phase 16.4 — Delete & detach cascade ✂️ (Completed)

**Scope**
- `operation: delete` on an **edge** mapping → remove that one edge, tombstone its lane, drop it from both endpoints' incident indexes. Nothing else.
- `operation: delete` on a **node** mapping → **`DETACH DELETE` semantics**: remove the node, then for every edge key in `{root}/.conduit/incident/{node_id}.json`, remove the edge and tombstone its lane **at the node-delete event's sequence**. All in one adapter operation (the store's unit of atomicity).
- A later `UserFollowed { u1, x }` at a sequence above the node delete → the edge's lane was tombstoned by the cascade → `Skipped(StaleSequence)` / `Skipped(AlreadyDeleted)`, consistent with Phase 13.
- A default-facet `upsert` resurrection of the node (Phase 13.4) clears the node's lanes; incident edges are **not** auto-resurrected — they return only if their own events are re-delivered above their tombstone sequence.

**Guarantees**
- No dangling edges after a node delete.
- The cascade is deterministic: it tombstones exactly the incident set recorded at delete time, at one sequence.

**As built**
- Incident index (`{root}/.conduit/incident/{node_id}.json`) stores `IncidentRef { edge_type, key, from, to }` so the cascade can drop each edge from the *other* endpoint's index too; the index file is deleted when it becomes empty.
- **Deviation from the doc's literal wording.** 16.4 said a later `UserFollowed { u1, x }` "at a sequence above the node delete" is `Skipped(StaleSequence) / Skipped(AlreadyDeleted)`. That only holds for a *previously-connected* edge whose lane the cascade tombstoned, and only below the tombstone sequence — a fresh edge to a deleted node, or a re-follow at a higher sequence, would otherwise resurrect. As built: an edge upsert first checks both endpoint node guards; if either endpoint is tombstoned, the edge is `Skipped(EntityAbsent)` (the shared Phase 15 reason — not the future-backend `EndpointAbsent`). This delivers "no attaching to a deleted node" for every case, and clears once the node is resurrected.
- Node detach cascade + node-facet cascade (Phase 15) are both applied: a default-facet node delete tombstones the node's facet lanes *and* every incident edge, in one adapter operation.

---

## Phase 16.5 — Capability-safe validation ✅ (Completed)

**Scope**
- `graph/` structural checks folded into the live `runtime/validation.rs` (per the Phase 14 finding that `sql/validate.rs` / `document/validate.rs` are dead code): `key` / `from` / `to` / `discriminator` are valid `payload.` / `metadata.` path expressions; `label` / `edge_type` non-empty; node `properties` present iff `operation: upsert`; `operation: delete` only on the default facet (Phase 15.1); a routed `graph-*` adapter has a `GraphMapping` for every event type on its route.
- New `ValidationIssue` variants: `InvalidGraphMapping`, `MissingGraphMapping`, `UnroutedGraphMapping`, `GraphMappingNoGraphTarget`. Graph *node* mappings also feed `validate_facets` (Phase 15.1) — entity = `label`, identity = `[key]`, fields = property keys; edges are not faceted so contribute no view.
- **Scalar-path note:** validation checks the path *shape* (`payload.`/`metadata.` prefix), not that it resolves to a scalar at runtime — the latter is impossible without an event and is caught by `resolve()` returning `BuildFailed`, same as every other adapter.
- The graph adapter declares `AdapterCapability::{Write, Idempotent, Upsert, Delete}` — **not** `Transactions` (single-record writes; the detach cascade is one adapter operation, not a user-visible transaction). No new `Graph` *capability* — `StorageKind` is the kind, capabilities are behaviours.
- Closes the "routes to `graph-*` but cannot construct" gap.

**Guarantees**
- No new validation mechanism — graph rules are instances of the existing scalar-path / capability / mapping-coverage checks.

---

## Phase 16.6 — Determinism & verification suite 🧪 (Completed)

| Test | Asserts |
|---|---|
| `graph_node_lifecycle.rs` | `[register s1, rename s2, delete s3]` → node absent; guard tombstoned at 3. |
| `graph_edge_lifecycle.rs` | `[follow s1, unfollow s2]` → edge absent. |
| `graph_out_of_order.rs` | Edge upsert sequences `3, 1, 2` → edge at the seq-3 property state, any arrival order. |
| `graph_edge_before_endpoints.rs` | `follow` before either `register` → edge written (permissive, documented); endpoints referenced by id. |
| `graph_detach_delete.rs` | Node `u1` with 3 incident edges, `delete u1 s5` → node and all 3 edges absent; a `follow(u1, x) s7` afterwards → `Skipped`. |
| `graph_multigraph_discriminator.rs` | Two `RATED` edges same `(from, to)` with different `discriminator` → two distinct edges; same discriminator → upsert. |
| `graph_node_facet.rs` | A node with `facet: profile` + `facet: stats` updated out of order → both land (Phase 15 on a node). |
| `graph_capability_rejected.rs` | Config routing a `delete` graph mapping through a non-delete adapter → validation error. |
| `graph_cross_adapter.rs` | `UserRegistered` projected to a SQL row **and** a graph node → key/properties agree. |
| `graph_dispatch_unchanged.rs` | A graph adapter with `depends_on` a SQL adapter runs in the right frontier — no `StorageKind::Graph` branch in dispatch. |

**Guarantee under test:** every node and every edge is the Phase 12/13 pure function of its own delivered events; records do not interfere; a node delete leaves no incident edge; the whole adapter adds no `StorageKind::Graph` match arm to the engine core.

**As built** — all ten test files exist under `crates/conduit-core/tests/` with those names and pass. `dispatch.rs` and `runtime/dependency_graph.rs` are unmodified (no `StorageKind::Graph` arm anywhere). Full workspace: `cargo build`, `cargo test` (187 tests), `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check` all clean.

---

## Non-Goals (explicit)

- **No traversal, query, or pattern matching.** Conduit *writes* the graph. `MATCH (a)-[:FOLLOWS*2]->(c)`, shortest-path, PageRank, neighbourhood reads — none are expressible, same as "no scan/query" for KV.
- **No subgraph-per-event.** One `GraphMapping` emits one node or one edge. A `OrderPlaced` that needs a node plus several edges is decomposed into per-record events upstream, or waits for a dedicated phase.
- **No endpoint-existence enforcement** in the file-backed adapter (documented producer contract; a future enforcing backend may add `Skipped(EndpointAbsent)`).
- **No Neo4j / Cypher / external backend.** File-backed adjacency only — the SQLite / file / file-KV precedent. A Cypher backend is a later phase.
- **No undirected edges.** `[edge_type, from, to]` is ordered; emit both directions or normalise upstream.
- **No auto-resurrection of detached edges.** Resurrecting a node does not bring back edges the detach cascade removed; their own events must re-fire.
- **No graph-wide transactionality** beyond the single detach cascade (itself confined to one adapter operation, like Phase 15's facet-delete cascade).
- **No schema / label constraints, no indexes.** Conduit does not enforce "a `User` node must have `name`" or build property indexes.
