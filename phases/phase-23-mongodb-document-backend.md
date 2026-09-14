# Phase 23 — MongoDB Document Backend 🎯 (Completed)

**Goal:** A real database backend for the Document kind, completing the "all four kinds, real database, in core" story (`architecture.md` §1.4). Same pattern as Phases 19 (Postgres) and 22 (Redis): extract the file adapter's orchestration into a backend trait, keep `decide()` untouched, let MongoDB implement it.

**Why:** MongoDB is the single most-expected "document store" — this is the natural next credibility gap after KV (Redis, Phase 22). It's also, structurally, the *easiest* fit of the three real backends still open: Conduit's document projections are already JSON objects; MongoDB stores BSON documents natively, so there's no relational flattening (SQL) and no query-language translation (Cypher, Phase 24) — the projected `document` *is* the Mongo document.

---

## Phase 23.1 — Document backend seam 🧩 (Completed)

**Scope**
- Extract the orchestration in `adapter/document/file.rs` (read guard sidecar → facet pre-check → `decide()` → write/merge the document → write guard) into a `DocumentBackend` trait, mirroring Phase 22.1's `KvBackend`:

  ```rust
  trait DocumentBackend {
      fn read_guard(&mut self, collection: &str, entity_id: &str) -> Result<Option<ProjectionGuard>, DocumentError>;
      fn write(&mut self, collection: &str, entity_id: &str, document: Option<&Value>, facet: &str, guard: &ProjectionGuard) -> Result<(), DocumentError>;
      fn delete(&mut self, collection: &str, entity_id: &str, guard: &ProjectionGuard) -> Result<(), DocumentError>;
  }
  ```

- `ProjectionGuard` / `FacetGuard` (Phase 11.3/13.3/15.2) are reused as the guard schema, unchanged — same struct, a different wire format (BSON document instead of a JSON sidecar file).
- `FileDocumentStore` implements `DocumentBackend` over the existing sidecar files. Behaviour byte-identical to every Phase 11–15 document test.

**Guarantees**
- `decide()` unchanged; a backend supplies only how the guard+document pair is read and atomically written.

**As built**
- `adapter/document/exec.rs`: `ProjectionGuard`/`FacetGuard` (moved from `file.rs`, fields made `pub`), the `DocumentBackend` trait (`read_guard`/`write`/`delete`, exactly the doc's sketch), `DocumentPlan`, `DocumentOutcome`, and `project()` — the single Phase 11–15 orchestration.
- **Deviation from the KV seam (Phase 22.1):** `project()` does **not** read-then-merge a named facet's value itself — unlike `KvBackend`, `DocumentBackend` has no `read_value` method. `write()` receives the facet's own unmerged fields plus the facet name, and each backend decides how to apply it. This is deliberate, not an oversight: it lets MongoDB do the merge as a single server-side `$set` (Phase 23.3) instead of a round trip this layer would otherwise force. The file backend's `write()` still does its own read-merge-write internally when `facet` is non-empty — behaviour is byte-identical to the pre-23.1 inlined version (confirmed: the full pre-existing document test suite passes unchanged).
- `FileDocumentAdapter`'s name was **not** changed (unlike Phase 22's `KeyValueStore` → `FileKvStore` rename) — it was already adapter-suffixed and consistent with `SqliteAdapter`/`PostgresAdapter`, so only its internals were refactored to delegate to a new `FileDocumentBackend`.

---

## Phase 23.2 — MongoDB backend 🍃 (Completed)

**Scope**
- New dependency: the official `mongodb` driver, sync API (`features = ["sync"]`) to keep the adapter path synchronous, matching `rusqlite` / `postgres` / `redis`.
- `AdapterConfig::MongoDb` → `MongoDbAdapterConfig { id, priority, config: { url: String, database: String, pool_size: Option<u32> }, capabilities, depends_on }`. `url` supports `${ENV_VAR}` expansion.
- Target documents live in `{database}.{collection}` (`collection` = the mapping's `collection` field, same name used by the file adapter today).
- Guard documents live in a dedicated `conduit_projection_state` collection — kept separate from the user's projected documents (same principle as the SQL guard table), one document per `(target_collection, entity_key, facet)`, with a unique compound index on that triple.

**Guarantees**
- Same `entity_key` / `collection` scheme as the file adapter — a stream projected into MongoDB converges to the same logical state as the same stream into files.
- No column-typing problem (Phase 19.3's Postgres pain): BSON is dynamically typed like JSON, so JSON scalars map directly.

**As built**
- Dependency: `mongodb = { version = "3", features = ["sync"] }`. **No r2d2 pool** — unlike Postgres/Redis, `mongodb::sync::Client` is already an internally-pooled, `Arc`-backed, cheaply-cloneable handle; `pool_size` maps onto the connection string's `maxPoolSize` parameter (appended to the URL) rather than a separate pool wrapper, since `ClientOptions::parse` is async and the adapter path stays synchronous end to end.
- **Deviation from 23.1's own instruction:** the guard collection stores **one document per `(target, entity_key)` — not per `(target, entity_key, facet)`** as this sub-phase's scope text says. 23.1 explicitly says to reuse `ProjectionGuard` "unchanged" — the same struct with a nested `facets: BTreeMap`, one record per *entity* — which is what Phase 22 (Redis) already did for its guard key, and what the file adapter has always done. One-document-per-facet-lane would mean NOT nesting facets, contradicting 23.1's explicit instruction and diverging from the established precedent for this struct. Resolved in favor of 23.1's more specific, precedent-consistent instruction; the "unique compound index" is therefore on `(target, entity_key)` (two fields), not the stated triple.
- `AdapterConfig::MongoDb` / `MongoDbConfig { url, database, pool_size }`, `url` supporting `${ENV_VAR}` expansion (Phase 19.2 pattern, reused as-is).
- Target documents are keyed by `_id: <entity_id>` in `{database}.{collection}` — literally the projected JSON object becomes the Mongo document, per the phase's own framing.

---

## Phase 23.3 — Atomic write: native CAS + a session transaction 🔒 (Completed)

**Scope**
- The guard update is a **native atomic compare-and-swap in one round trip**: `find_one_and_update` on the guard document with filter `{ target, entity_key, facet, last_sequence: { $lt: event.sequence } }` (or "does not exist yet" for an insert) — MongoDB guarantees single-document operations are atomic, so this *is* the CAS, no retry loop needed (unlike Redis's optimistic WATCH/MULTI/EXEC).
- The guard write and the target-document write are two different documents (different collections), so they're wrapped in one **client-session transaction** (`start_transaction` / `commit_transaction`) for atomicity across the pair. This requires MongoDB running as a replica set — including a single-node "replica set of one," which is how most modern deployments (including Atlas) run by default; a standalone `mongod` cannot do multi-document transactions.
- A named facet's update is a targeted `$set` on the facet's top-level fields within the target document — an atomic partial update Mongo does server-side, not a read-merge-write like the file adapter or Redis need.

**Guarantees**
- Genuine ACID: two concurrent writers to the same entity serialize through MongoDB's own transaction conflict detection — one commits, the other's transaction aborts and the Rust retry loop (bounded, same shape as Phase 22.3) re-reads and gets `Skipped(StaleSequence)` if it lost the race.

**As built**
- **Deviation from the sketch:** no manual `find_one_and_update` filter-based CAS on the guard document. The whole read-decide-write cycle (`read_guard` → `decide()` in Rust → `write`/`delete`) runs inside one MongoDB client-session transaction (`session.start_transaction()` … `commit_transaction()`); MongoDB's own transaction conflict detection is the CAS — a concurrent writer's commit makes ours fail with a `TransientTransactionError`-labeled error (checked via `Error::contains_label`), detected in `MongoDbAdapter::handle()` after `commit_transaction()` (or immediately, if a write inside the transaction already failed with that label). This is simpler than hand-encoding `decide()`'s branch logic into a Mongo filter and is exactly the officially-documented MongoDB transaction-retry pattern.
- The retry loop (`MAX_ATTEMPTS = 5`, matching Phase 22.3) lives in the adapter, wrapping a fresh `start_session()` + `start_transaction()` + `exec::project()` + `commit_transaction()` each attempt — never a bare retry of just the commit. `UnknownTransactionCommitResult` (MongoDB's official guidance: retry only the commit) is folded into the same "retry the whole transaction" path for simplicity, which is safe here because `decide()` is itself sequence-gated and idempotent — a redundant retried write just lands on `Skipped(StaleSequence)` or a no-op match.
- `createIndex` cannot run inside a transaction, so the guard collection's unique index on `(target, entity_key)` is created once per adapter instance (`Mutex<bool>` guard, checked outside any transaction) before the retry loop starts — mirroring Postgres's lazy, cached column-type introspection (Phase 19.3) in spirit.
- A named facet's `write()` is a targeted `update_one` with `$set` on the facet's own top-level fields — no read, exactly as scoped.
- **Real bug, caught by CI, not by local testing:** the first push had the guard's `replace_one` replacement built as bare `to_document(guard)` — `ProjectionGuard` itself carries no `target`/`entity_key` fields (they're the caller's identity, not guard state), and `replace_one` overwrites the *entire* document with exactly what it's given. Every guard write was therefore silently dropping both fields, so the unique index saw every entity as the same `{target: null, entity_key: null}` row: the second guard write anywhere in the process hit `E11000 duplicate key`, and — because that failure happened inside the transaction — the whole transaction (including the already-applied target-document write) rolled back with it. Fixed by a `MongoBackend::guard_doc()` helper that inserts `target`/`entity_key` into the serialized guard before every `replace_one`, the same way `write()` already did for the target document's `_id`. This is exactly why this environment's lack of Docker matters: the bug was invisible to `cargo build`/`cargo clippy`/a compile-check of the test file, and only surfaced once CI's `test-mongo` job ran the suite against a real server.

---

## Phase 23.4 — Capability & validation ✅ (Completed)

**Scope**
- `MongoDbAdapter` declares `AdapterCapability::{Write, Idempotent, Upsert, Delete, Transactions}` — genuinely, real multi-document ACID transactions (like Postgres, unlike Redis's optimistic retry).
- `runtime/validation.rs`: a routed `mongodb` adapter needs a `DocumentMapping` per routed event type (existing rule); `url` / `database` non-empty. A connectivity check at validation time warns (does not fail) if the deployment isn't a replica set, since transactions would then fail at runtime.

**Guarantees**
- No new validation mechanism.

**As built**
- `MongoDbAdapter` declares `Write, Idempotent, Upsert, Delete, Transactions` exactly as scoped.
- `runtime/validation.rs`: `is_file()` **renamed to `is_document()`** (mirroring `is_sql`/`is_keyvalue`) and extended to `matches!(a, AdapterConfig::File(_) | AdapterConfig::MongoDb(_))`. A new `InvalidMongoDbConfig` mirrors `InvalidPostgresConfig`/`InvalidRedisConfig`: `url` and `database` non-empty, `url` parses via `mongodb::sync::Client::with_uri_str` (no connection attempted).
- **Deviation, deliberately not implemented:** the live replica-set connectivity warning. This module has never had a non-fatal issue — `ValidationReport::into_result()` treats every pushed `ValidationIssue` as fatal by construction, so a "warns, does not fail" check would need a new warning channel, directly contradicting this same sub-phase's own "no new validation mechanism" guarantee. Skipped rather than silently mis-scoped; a non-replica-set deployment now simply fails at the first write instead of at validation time, the same tradeoff every other adapter here already makes for connectivity problems in general.

---

## Phase 23.5 — Verification 🧪 (Completed)

`testcontainers`-gated suite on a replica-set-enabled MongoDB image (skipped without Docker):

| Test | Asserts |
|---|---|
| `mongo_upsert_out_of_order.rs` | Phase 12 scenario — sequences `3, 1, 2` → document at seq-3 state. |
| `mongo_delete_lifecycle.rs` | Phase 13 — create/update/delete → document gone, guard tombstoned. |
| `mongo_facet_disjoint.rs` | Phase 15 — two facets updated out of order → both `$set` merges land. |
| `mongo_concurrent_writers.rs` | Two threads race the same entity via real transactions → one commits, one aborts-then-skips-stale; no lost write. |
| `doc_cross_backend.rs` | The same stream into the file adapter and MongoDB → identical logical document and guard state. |

**Guarantee under test:** MongoDB is behaviourally interchangeable with the file-backed document adapter, with real ACID guarantees under concurrent writers (no retry-and-lose window, unlike the optimistic Redis path).

**As built**
- **Deviation:** all five scenarios live in **one consolidated file, `crates/conduit-core/tests/mongo_backend.rs`**, sharing one skip guard and a `fresh_collection()` helper — mirroring Phase 19.6's `pg_backend.rs` and Phase 22.5's `redis_backend.rs` precedent.
- **Deviation:** gated on **`CONDUIT_MONGO_URL`** (plus optional `CONDUIT_MONGO_DB`, default `conduit_test`), not `testcontainers` — matching the established, already-twice-documented deviation (Phase 19.6, Phase 22.5) between this doc's aspirational `testcontainers` wording and every backend's actual env-var-gated implementation. CI's new `test-mongo` job starts `mongo:7 --replSet rs0` via a plain `docker run` (GitHub Actions' `services:` block has no way to override a container's command, only `docker create` flags like health checks), waits for the server to answer, calls `rs.initiate()`, waits for the single node to become primary, then points `CONDUIT_MONGO_URL` at it with `?directConnection=true`.
- `mongo_concurrent_writers` races two real OS threads (`std::thread::spawn`) against a shared `Arc<MongoDbAdapter>`, same shape as Phase 22.5's `redis_concurrent_writers`.
- **Not verified against a live MongoDB in the implementing environment** — no Docker daemon available (the same limitation already documented for Phase 19's Postgres example, Phase 21.6's UDS listener, and Phase 22.5's Redis suite). All five tests were confirmed to compile and skip cleanly (`SKIP: CONDUIT_MONGO_URL not set`) with `cargo test -p conduit-core --test mongo_backend`; the actual transactional read-decide-write logic, and the CI job's replica-set bring-up script, run for real only under CI's `test-mongo` job — that job's exact shell sequence is a best-effort implementation against MongoDB's documented behavior, not something this environment could rehearse.

---

## Non-Goals (explicit)

- **Not KV or Graph.** One backend, one kind — Document only.
- **No GridFS** — large-object storage is out of scope; documents are the mapping's resolved JSON, same size class as everywhere else.
- **No aggregation pipeline / read API exposed.** Conduit writes documents; querying them is the application's job, same as every other adapter.
- **No sharded-cluster-specific tuning** (shard key selection, zone sharding). A single replica set is the supported target; sharded clusters may work but aren't tuned for.
- **No schema validation rules management.** Conduit does not configure MongoDB's `$jsonSchema` validators on your collections.
