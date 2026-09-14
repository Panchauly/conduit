# Phase 22 — Redis Key-Value Backend 🎯 (Completed)

**Goal:** One real database backend for the Key-Value kind. Of the three file-backed kinds (Document, Key-Value, Graph), KV is the credibility gap — "key-value store" means Redis to almost everyone, and shipping v1.0 with only a file-backed KV adapter undersells the pitch. Phase 22 adds Redis behind the *same* `KvMapping` / `decide()` / guard model the file adapter already uses.

**Why:**
- Phase 19 proved the pattern: extract the storage-agnostic orchestration once, let a real backend implement a small trait, keep `decide()` untouched. Phase 22 repeats it for KV.
- Redis over Neo4j or MongoDB because it's the highest-expectation, lowest-effort real backend among the three file-backed kinds — no query-language translation (SQL dialects, Cypher), just key reads/writes.
- **One backend, one kind, in `conduit-core`** — this phase does not touch Document or Graph.

---

## Phase 22.1 — KV backend seam 🧩 (Completed)

**Scope**
- Extract the orchestration currently inlined in `adapter/keyvalue/store.rs` (read guard sidecar → facet existence pre-check → `decide()` → apply value write/merge/delete → write guard) into a backend-agnostic driver over a small trait:

  ```rust
  trait KvBackend {
      /// Read the whole-entity guard (all facet lanes) — Phase 12/13/15 `KvGuard`, unchanged.
      fn read_guard(&mut self, namespace: &str, key: &str) -> Result<Option<KvGuard>, KvError>;
      /// Read the current value (for a facet's shallow-merge).
      fn read_value(&mut self, namespace: &str, key: &str) -> Result<Option<Value>, KvError>;
      /// Atomically apply the resolved value (or merged facet) and the updated guard together.
      fn write(&mut self, namespace: &str, key: &str, value: Option<&Value>, guard: &KvGuard) -> Result<(), KvError>;
      /// Atomically remove the value and write the tombstoned guard.
      fn delete(&mut self, namespace: &str, key: &str, guard: &KvGuard) -> Result<(), KvError>;
  }
  ```

- `KvGuard` (already `Serialize`/`Deserialize` for the file sidecar — Phase 12.1/13.3/15.2) is reused **as-is** as the wire format for both backends: one JSON blob per entity, default facet in the flat fields, named facets in the `facets` map. No new guard schema.
- `FileKvStore` implements `KvBackend` over the existing sidecar files. **Behaviour byte-identical** — every Phase 14/15 KV test passes unchanged.

**Guarantees**
- `decide()` and the gating/facet logic are written once; a backend supplies only how the guard+value pair is read and atomically written.

**As built**
- `adapter/keyvalue/exec.rs`: `KvGuard`/`KvFacetGuard` (moved from `store.rs`, fields made `pub` — reused as-is as the wire format for both backends, unchanged shape), the `KvBackend` trait (`read_guard`/`read_value`/`write`/`delete`, exactly the doc's sketch), `KvPlan`, `KvOutcome`, and `project()` — the single Phase 11–15 orchestration.
- `store.rs`'s `KeyValueStore` **renamed to `FileKvStore`** (matching this doc's own naming and the coming `RedisAdapter` symmetry) implementing `StorageAdapter`, delegating to a small `FileKvBackend` that implements `KvBackend` over the existing sidecar files. Every pre-22.1 KV test (`kv_upsert_out_of_order`, `kv_delete_lifecycle`, `kv_permanent_tombstone`, `kv_redelivery`, `kv_resurrection`, `kv_delete_before_create`, `kv_cross_adapter`) passes unchanged after a mechanical `KeyValueStore` → `FileKvStore` rename — behaviour is byte-identical, confirmed by the full existing suite staying green.
- `KvBackend::write` takes `value: Option<&Value>` exactly per the doc's sketch — `None` means "leave the value key untouched, just persist the guard" (the `SkipAlreadyDeleted` sequence-bump path), matching the file adapter's pre-existing behavior of never touching the value file on that skip.

---

## Phase 22.2 — Redis backend 🔴 (Completed)

**Scope**
- New dependency: the `redis` crate (sync API) + `r2d2` for pooling — same pooling story as Phase 19's Postgres backend.
- `AdapterConfig::Redis` → `RedisAdapterConfig { id, priority, config: { url: String, pool_size: Option<u32> }, capabilities, depends_on }`. `url` supports `${ENV_VAR}` expansion (Phase 19.2).
- Key layout, one Redis key per entity for each of value and guard:
  - Value: `{namespace}:{entity_key}` — the resolved JSON, `SET`/`GET`/`DEL`.
  - Guard: `__conduit:guard:{namespace}:{entity_key}` — the whole `KvGuard` struct, JSON-serialized in one key (not a Redis hash — keeps the exact same struct and `serde` path as the file sidecar).
- `RedisBackend` implements `KvBackend` over pooled connections.

**Guarantees**
- Same `entity_key` / namespace scheme as the file adapter — a stream projected into Redis converges to the same logical state as the same stream into files.

**As built**
- Dependency: `redis = { version = "1", features = ["r2d2"] }` — the `redis` crate's own `r2d2` feature implements `r2d2::ManageConnection` directly on `redis::Client`, so `r2d2::Pool<redis::Client>` needs no separate connection-manager crate (no `r2d2_redis`). Same lazy/`build_unchecked` pooling pattern as Phase 19.2's Postgres pool — connectivity problems surface on the first real write, not at construction.
- `AdapterConfig::Redis(RedisAdapterConfig)` / `RedisConfig { url, pool_size }`, `url` supporting `${ENV_VAR}` expansion via the Phase 19.2 `expand_env` helper, reused as-is.
- Key layout exactly per the doc: value at `{namespace}:{entity_key}`, guard at `__conduit:guard:{namespace}:{entity_key}` (the whole `KvGuard` JSON-serialized in one key, same struct and `serde` path as the file sidecar).

---

## Phase 22.3 — Atomic write via WATCH / MULTI / EXEC 🔒 (Completed)

**Scope**
- Redis has no server-side equivalent of Postgres's `FOR UPDATE` row lock reachable from a client transaction, so `write` / `delete` use Redis's **optimistic** transaction primitive:

  ```
  WATCH  guard_key
  GET    guard_key                 -- read, still outside the Rust decide() call
  ...    decide() runs in Rust, unchanged...
  MULTI
    SET  value_key   <new value>   -- or DEL if operation: delete
    SET  guard_key    <new guard>
  EXEC                              -- nil if guard_key changed since WATCH -> conflict
  ```

  On a nil `EXEC` (a concurrent writer touched the guard between `WATCH` and `EXEC`), retry the whole read-decide-write cycle up to a bounded number of attempts (default 5), then fail as `AdapterError::WriteFailed` (surfaces as a batch failure, retried by the run loop like any other transient error — Phase 17.5).
- `decide()` itself is **not reimplemented in Redis** (no Lua port of the state machine) — the gating logic stays the single Rust function every adapter calls. The cost of that choice is optimistic retry instead of a blocking lock; documented as the honest tradeoff.

**Guarantees**
- Two writers racing the same entity: one's `EXEC` succeeds, the other detects the conflict and retries — on retry it re-reads the now-updated guard and gets `Skipped(StaleSequence)` if it lost the race. No lost write, no corrupted guard, no raw Redis error surfacing to the caller.

**As built**
- **Deviation, by necessity:** `redis::transaction()` (the crate's own WATCH/MULTI/EXEC helper) retries *unboundedly* until it succeeds — no attempt cap, which doesn't match this phase's explicit "bounded, default 5, then fail" requirement. Hand-rolled the same WATCH → read → decide → MULTI/EXEC loop instead (`keyvalue/redis.rs`), modeled directly on `redis::transaction`'s own source (confirmed via the upstream implementation): `KvBackend::read_guard` issues `WATCH` then `GET`; `write`/`delete` issue `MULTI … EXEC` via an all-`.ignore()`d atomic `Pipeline` and interpret a `None` result (nil `EXEC`, meaning a concurrent writer changed the guard) as `KvError::WriteConflict`. The retry loop lives in `RedisAdapter::handle()`, re-running `exec::project()` from scratch (fresh `WATCH` + guard read each attempt) up to `MAX_ATTEMPTS = 5`, issuing a defensive `UNWATCH` after every attempt (matching `redis::transaction`'s own belt-and-suspenders cleanup) so a hard error before `EXEC` never leaves a stale `WATCH` on a connection returned to the pool.

---

## Phase 22.4 — Capability & validation ✅ (Completed)

**Scope**
- `RedisAdapter` declares `AdapterCapability::{Write, Idempotent, Upsert, Delete}`. **Not** `Transactions` in the Postgres sense — WATCH/MULTI/EXEC is optimistic concurrency (retry-on-conflict), not a pessimistic lock; document the distinction so a mapping author doesn't assume Redis and Postgres give identical guarantees under contention.
- `runtime/validation.rs`: a routed `redis` adapter needs a `KvMapping` for each event type on its route (same rule as `keyvalue`/`file`); `url` non-empty.

**Guarantees**
- No new validation mechanism — Redis reuses the KV mapping-coverage and capability checks.

**As built**
- `RedisAdapter` declares `Write, Idempotent, Upsert, Delete` — **not** `Transactions`, per the doc's explicit distinction.
- `runtime/validation.rs`: `is_keyvalue()` extended to `matches!(a, AdapterConfig::KeyValue(_) | AdapterConfig::Redis(_))` — the single choke point already shared by every KV mapping-coverage/capability check, mirroring exactly how `is_sql()` already covers both SQLite and Postgres. A new `InvalidRedisConfig` issue mirrors `InvalidPostgresConfig` (`url` non-empty, `redis::Client::open` parses it). Confirmed no other `runtime/validation.rs` code path needed touching.

---

## Phase 22.5 — Verification 🧪 (Completed)

`testcontainers`-gated integration suite using a Redis container (skipped, not failed, without Docker):

| Test | Asserts |
|---|---|
| `redis_upsert_out_of_order.rs` | Phase 12 scenario against Redis — sequences `3, 1, 2` → value at seq-3 state. |
| `redis_delete_lifecycle.rs` | Phase 13 — set/update/delete → key gone, guard tombstoned. |
| `redis_facet_disjoint.rs` | Phase 15 — two facets updated out of order → both merge correctly. |
| `redis_concurrent_writers.rs` | Two threads race the same entity → one applies, the other retries then (if it lost) gets `Skipped(StaleSequence)` — no lost write, no corruption. |
| `kv_cross_backend.rs` | The same event stream into the file adapter and Redis → identical logical value and guard state. |

**Guarantee under test:** Redis is behaviourally interchangeable with the file-backed KV adapter for every projection scenario, and safe under concurrent writers via optimistic retry.

**As built**
- **Deviation:** all five scenarios live in **one consolidated file, `crates/conduit-core/tests/redis_backend.rs`** (`redis_upsert_out_of_order`, `redis_delete_lifecycle`, `redis_facet_disjoint`, `redis_concurrent_writers`, `kv_cross_backend`), sharing one skip guard and a `fresh_namespace()` helper — mirroring Phase 19.6's own `pg_backend.rs` precedent and its stated rationale (every scenario shares the same skip guard and fresh-identifier helper).
- **Deviation:** gated on the **`CONDUIT_REDIS_URL`** env var (`SKIP: CONDUIT_REDIS_URL not set` when unset), not the `testcontainers` crate this doc's scope named — matching Phase 19.6's actual implementation (also env-var-gated, not `testcontainers`, despite that phase's doc naming `testcontainers` too; documented there as a Phase 21.2 "as built" deviation). CI's new `test-redis` job (`.github/workflows/ci.yml`) points `CONDUIT_REDIS_URL` at a `redis:7` service container.
- `redis_concurrent_writers` races two real OS threads (`std::thread::spawn`) against a shared `Arc<RedisAdapter>` instead of two threads/processes as sketched — sufficient to exercise the WATCH/MULTI/EXEC conflict path, since the retry logic has no awareness of thread vs. process boundaries (it only ever sees "the guard changed since my WATCH").
- **Not verified against a live Redis in the implementing environment** — no Docker daemon and no local `redis-server` available (the same limitation already documented for Phase 19's Postgres example, Phase 21.6's UDS listener, and the `examples/postgres` example). All five tests were confirmed to compile and to skip cleanly (`SKIP: CONDUIT_REDIS_URL not set`) with `cargo test -p conduit-core --test redis_backend`; the actual read-decide-write and WATCH/MULTI/EXEC logic runs for real only under CI's `test-redis` job.

---

## Non-Goals (explicit)

- **Not Document or Graph.** This phase is KV only — one backend, one kind, per the given scope. Neo4j and MongoDB stay open, undecided future work.
- **No TTL / expiry.** Same reasoning as Phase 14: time-based eviction breaks replay determinism. Deferred, not solved.
- **No Redis Cluster / sharding.** A single Redis endpoint (or a client-side cluster-aware `url` if the `redis` crate's default cluster support happens to work) — no custom sharding logic.
- **No Redis Streams / Pub-Sub usage.** Redis here is a plain KV store target, not a transport (that's Phase 20's job, and Kafka/Redis-as-broker is explicitly Pro-scope elsewhere).
- **No Lua-scripted gating.** `decide()` stays the one Rust implementation; the cost is optimistic retry, not a second state machine to keep in sync.
- **No HA/Sentinel config beyond a connection URL.** Failover topology is an operational concern, not this phase's.
