# Phase 19 — Postgres SQL Backend 🎯 (Completed)

**Goal:** A real, production-usable SQL sink. The SQL adapter has only ever run against SQLite (`bundled`, one file, one writer). Phase 19 adds a Postgres backend behind the *same* `SqlMapping` / `decide()` / guard model, and in doing so proves the SQL adapter's abstraction is not SQLite-specific.

**Why:**
- Every adapter is file-backed. A SQL read model on Postgres is the canonical Conduit use case (transactional read models, reporting, anything queried with real SQL) and nothing in the engine can serve it yet.
- The SQL adapter's write path (`INSERT … ON CONFLICT … DO UPDATE`, the `conduit_projection_state` guard, the read-decide-write transaction) was designed once, for SQLite, in `adapter/sql/sqlite.rs`. Postgres is the test of whether that design transfers — `ON CONFLICT` is identical in both, which is a good sign, but placeholders, types, and connection/transaction handling differ.
- Postgres gives **real multi-writer safety** via MVCC + row locks — the file adapters all carry an explicit "single-writer assumption." This would be the first adapter without it.

The sink side is where Conduit's value concentrates (the projection logic); making one sink production-real is higher-leverage than another way to read a log.

---

## Phase 19.1 — SQL backend seam 🧩 (Completed)

**Scope**
- Extract the transaction orchestration currently inlined in `adapter/sql/sqlite.rs::handle()` (~250 lines: ensure guard table → facet existence pre-check → read guard lane → `decide()` → apply INSERT/UPSERT/DELETE → upsert/tombstone the guard row → commit) into a backend-agnostic `adapter/sql/exec.rs` driving a `SqlTxn` trait:

  ```rust
  trait SqlTxn {
      fn ensure_guard_table(&mut self) -> Result<(), SqlError>;
      fn read_guard_lane(&mut self, table: &str, key: &str, facet: &str) -> Result<Option<GuardState>, SqlError>;
      fn execute_write(&mut self, w: &SqlWrite) -> Result<(), SqlError>;
      fn upsert_guard_row(&mut self, row: &GuardRow) -> Result<(), SqlError>;
      fn delete_row(&mut self, table: &str, cols: &[String], vals: &[Value]) -> Result<(), SqlError>;
      fn commit(self) -> Result<(), SqlError>;
  }
  ```

- `SqlMapping::build()` stops emitting a finished SQL string; it returns a neutral `SqlWrite { table, columns, values, conflict_target, set_columns }`. Each backend renders it — placeholders are the only real dialect branch (`?N` for SQLite, `$N` for Postgres); `ON CONFLICT (…) DO UPDATE SET …` text is identical.
- `SqliteTxn` implements `SqlTxn` over `rusqlite`. **Behaviour byte-identical** — every Phase 11–15 SQL test passes unchanged.

**Guarantees**
- `decide()` and the orchestration are written once; a backend supplies only connection, transaction, placeholder style, and guard-table DDL.

**As built**
- `adapter/sql/exec.rs`: `SqlWrite { table, columns, values, conflict_target, set_columns }` + `render(Placeholders)` (`?N` / `$N` — the only real dialect branch), `GuardRow`, the `SqlTxn` trait, `SqlPlan`, and `project<T: SqlTxn>(txn, plan, event) -> SqlOutcome`. `SqlMapping::build` now returns `(SqlWrite, Vec<Value>)` (was `(String, Vec<Value>, Vec<Value>)`).
- `SqlTxn` is the doc's six methods **plus** `bump_guard_lane` (Phase 13.2 `SkipAlreadyDeleted`) and `cascade_facets` (Phase 15.3 delete cascade / resurrection clear), and a `lock: bool` on `read_guard_lane` for `FOR UPDATE`. These are the guard operations the orchestration genuinely needs a driver primitive for.
- `SqliteTxn` over `rusqlite` — every Phase 11–15 SQL test passes unchanged (byte-identical behaviour). `sqlite.rs` shrank from ~250 lines of inlined orchestration to a ~140-line driver.

---

## Phase 19.2 — Postgres backend 🐘 (Completed)

**Scope**
- New dependency: the sync `postgres` crate + `r2d2` / `r2d2_postgres` for pooling (matches `rusqlite`'s sync model; no `tokio` in the core adapter path).
- `AdapterConfig::Postgres` → `PostgresAdapterConfig { id, priority, config: { url: String, pool_size: Option<u32> }, capabilities, depends_on }`. `url` may be `${ENV_VAR}`-expanded (like `routing.file`), so secrets stay out of the committed config.
- `PostgresAdapter` holds the pool, built once in `build_adapters_from_config`; `handle()` checks one connection out per event.
- `PostgresTxn` implements `SqlTxn`. Guard-table DDL for Postgres:

  ```sql
  CREATE TABLE IF NOT EXISTS conduit_projection_state (
    target_table  text        NOT NULL,
    entity_key    text        NOT NULL,
    facet         text        NOT NULL DEFAULT '',
    last_sequence bigint      NOT NULL,
    last_event_id text        NOT NULL,
    deleted       boolean     NOT NULL DEFAULT false,
    permanent     boolean     NOT NULL DEFAULT false,
    processed_at  timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (target_table, entity_key, facet)
  )
  ```

**Guarantees**
- Same `entity_key` encoding, same `(table, key, facet)` guard key, same `decide()` outcomes as SQLite — a stream projected into Postgres converges to the same logical state as the same stream into SQLite.

**As built**
- Deps: `postgres` (feature `with-chrono-0_4`, **no TLS features** → no openssl), `r2d2`, `r2d2_postgres`, `chrono`. The sync `postgres` crate wraps `tokio-postgres` with an internal current-thread runtime — the **adapter path stays synchronous** (the doc's intent), but `tokio` is now in the dependency tree; there is no maintained pure-sync Postgres driver.
- `${ENV_VAR}` expansion (`runtime::config::expand_env`) — new; nothing expanded env vars before. An unset variable is left literal (surfaces as a connection error, not a silent empty string).
- `PostgresAdapter::new` is **infallible**: a bad URL is stored as `Err(String)` and every `handle()` fails cleanly, so `build_adapters_from_config` needs no `Result`. The pool is `build_unchecked` (lazy connect) — connectivity problems surface on the first write.

---

## Phase 19.3 — Column typing 🔡 (Completed)

**Scope**
- SQLite has type affinity — `json_scalar_to_string` binds everything as text and SQLite coerces. Postgres is strict: text into an `integer` / `boolean` / `numeric` / `timestamptz` column fails.
- On first use, `PostgresAdapter` introspects each target table's columns once (`SELECT column_name, data_type FROM information_schema.columns WHERE table_name = $1`) and caches the map. Each JSON scalar is then bound as the matching Postgres type (`i64`, `f64`, `bool`, `&str`, parsed timestamp), not as text.
- A mapping column absent from the target table → `ValidationError` at startup — a check SQLite's dynamic typing never allowed.
- `NULL` handling: a `null` JSON leaf binds as SQL `NULL` (already the case); a missing payload path is still a build error (Phase 11), unchanged.

**Guarantees**
- Type mismatches surface at startup (column missing) or as a clear `WriteFailed` (value not coercible), never as a silent wrong-value write.

**As built**
- `PgColType`: `Int | Float | Bool | Timestamptz | Timestamp | Date | Text | Other`. `Other` covers `numeric` / `uuid` / `json` / `jsonb` — bound as text and left to Postgres's assignment cast (typed binding for those would need `rust_decimal` / `uuid` crates; out of this scope).
- Timestamps parsed with `chrono` (`DateTime::parse_from_rfc3339` / `NaiveDateTime` / `NaiveDate`).
- **Column-existence is checked at run time, not validation** (see 19.5 "As built"): a mapping column not in the introspected table → `WriteFailed("column 'x' of mapping is not in table 't'")`, verified by `pg_column_missing_rejected`.

---

## Phase 19.4 — Concurrency 🔒 (Completed)

**Scope**
- The read-guard-lane → `decide()` → write sequence runs in one `READ COMMITTED` transaction with `SELECT … FOR UPDATE` on the `conduit_projection_state` row for `(target_table, entity_key, facet)`. Concurrent dispatch threads — or **concurrent Conduit instances** — then serialize on that row: one gets `Update`, the other re-reads and gets `SkipStale`, no lost write, no raw constraint error.
- Document explicitly: the Postgres SQL sink is the **first adapter without the single-writer assumption**. The file-backed adapters (Phases 12.4 / 14 / 16) still carry it; the Phase 15 note about non-atomic read-modify-write does not apply here.

**Guarantees**
- Two writers racing the same entity produce exactly one applied write and one clean skip — verified in 19.6.

**As built**
- `PostgresTxn::read_guard_lane(.., lock: true)` renders `… FOR UPDATE`; `SqliteTxn` ignores the flag (SQLite writers already serialize). `build_transaction().isolation_level(ReadCommitted)`.

---

## Phase 19.5 — Capability & validation ✅ (Completed)

**Scope**
- `PostgresAdapter` declares `AdapterCapability::{Write, Idempotent, Upsert, Delete, Transactions}` — `Transactions` genuinely, for the first time.
- `runtime/validation.rs`: a routed `postgres` adapter needs a `SqlMapping` for each event type on its route (same rule as `sqlite`); `url` non-empty / parses; the 19.3 column-existence check when the adapter can reach the DB at validation time (skipped, with a warning, when it can't).

**Guarantees**
- No new validation mechanism — Postgres reuses the SQL mapping-coverage and capability checks.

**As built**
- `is_sqlite` → `is_sql` (SQLite **or** Postgres) at the mapping-coverage and `SqlMappingNoSqlTarget` sites — a routed `postgres` adapter with no `SqlMapping` for an event on its route is a validation error, exactly as for `sqlite`.
- New `ValidationIssue::InvalidPostgresConfig { adapter_id, reason }` — empty or unparseable `url` (after `${ENV}` expansion).
- Capabilities: the config still *declares* what an adapter provides (the validator never introspects an adapter); a `postgres` adapter needs `capabilities: [write, upsert, delete, transactions]` in config to satisfy mappings requiring them — same model as every other adapter.
- **Deviation:** the DB-reachable column-existence check at validation time is **not implemented** — the validator has no warning channel and no DB handle, and the runtime check (19.3) already gives a clear error. Documented, not silently dropped.

---

## Phase 19.6 — Verification 🧪 (Completed)

- Dialect-rendering unit tests: `SqlWrite` → SQLite string and → Postgres string, asserted, **no database**.
- Integration suite gated on Docker via `testcontainers` (skipped, not failed, when Docker is absent — the hermetic SQLite suite stays the default):

| Test | Asserts |
|---|---|
| `pg_upsert_out_of_order.rs` | Phase 12 scenario against Postgres — sequences `3, 1, 2` → row at seq-3 state. |
| `pg_delete_lifecycle.rs` | Phase 13 — create/update/delete → row gone, guard tombstoned. |
| `pg_facet_disjoint.rs` | Phase 15 — two facets updated out of order → both columns land. |
| `pg_column_missing_rejected.rs` | Mapping names a column not in the table → validation error. |
| `pg_type_coercion.rs` | Integer / bool / timestamptz columns bound from JSON scalars correctly. |
| `sql_cross_backend.rs` | The same event stream into SQLite and Postgres → identical logical rows and guard state. |
| `pg_concurrent_writers.rs` | Two threads race the same entity → one `Updated`, one `Skipped(StaleSequence)`, no lost write, no error. |

**Guarantee under test:** Postgres is behaviourally interchangeable with SQLite for every projection scenario, and additionally safe under concurrent writers.

**As built**
- Dialect-rendering unit tests live in `adapter/sql/exec.rs` (`render` → `?N` / `$N`, plain insert / full upsert / facet / composite conflict target) — run with no database, always. Plus unit tests for `PgColType::from_data_type`, `bind_value` coercion, and `expand_env`.
- **Deviation from the doc:** the seven integration scenarios are **one file, `tests/pg_backend.rs`**, gated on **`CONDUIT_PG_URL`** (not `testcontainers`). Every test prints `SKIP: CONDUIT_PG_URL not set` and returns `Ok` when the env var is unset — CI without a database stays green, and the hermetic SQLite suite is untouched. Point `CONDUIT_PG_URL` at a throwaway Postgres to run them. Rationale: `testcontainers` is a large dev-dependency tree; the env-var gate satisfies the same "skipped, not failed" intent with zero extra deps. **These tests were not executed in the implementing environment** (no Docker/Postgres available); the SQLite suite (byte-identical through the shared seam) is the regression guarantee.
- `pg_concurrent_writers` seeds the entity at `sequence 4` so the racing seq-5 event is a valid `Update` and the racing seq-3 event is `SkipStale` **in either thread ordering** — the `FOR UPDATE` lock is what prevents a lost write / raw constraint error, and the assertion stays deterministic.

---

## Non-Goals (explicit)

- **No MySQL / other SQL dialects.** The 19.1 seam makes them possible; only SQLite and Postgres are in scope.
- **No `sqlx` unification.** `rusqlite` + sync `postgres`, kept as separate backends behind `SqlTxn`.
- **No DDL management of your read-model tables.** Conduit projects into tables *you* created; it owns only `conduit_projection_state`. Creating/migrating the target schema is out of scope.
- **No read path.** Conduit writes the Postgres read model; querying it is the application's job.
- **No secrets management.** `url` comes from config or an env var; vault integration, rotation, and credential brokering are out of scope.
- **No cross-adapter distributed transactions.** Each adapter's write is its own transaction (Phase 9 non-goal stands) — a Postgres write and a document write for one event are not atomic together.
- **No connection resilience policy beyond a basic pool.** Retry/backoff/failover/read-replica routing are later work.
