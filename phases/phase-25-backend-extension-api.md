# Phase 25 — Backend Extension API & Community Connectors 🎯 (Completed)

**Goal:** Turn the pattern proven four times over (`SqlTxn` for SQLite/Postgres, `KvBackend` for file/Redis, `DocumentBackend` for file/MongoDB, `GraphBackend` for file/Neo4j) into a **public, documented extension point**, so a contributor can ship a new backend — `conduit-mysql`, `conduit-dynamodb`, `conduit-elasticsearch` — as an independent crate without forking `conduit-core`. This is the community-connector lever named in `architecture.md` §1.4: more real backends grows adoption of the OSS core; it never touches the product tier, because the product doesn't sell capability.

**Why now, not earlier:** the four backend traits (Phases 19, 22–24) needed to exist *and prove themselves against genuinely different stores* — a relational DB, an in-memory KV store, a document DB, a graph DB — before generalizing an extension API from them. Designing the plugin surface after one example (Postgres) would have baked in SQL-shaped assumptions; after four, the actual shared shape is clear.

---

## Phase 25.1 — Public backend traits 📖 (Completed)

**Scope**
- `SqlTxn`, `KvBackend`, `DocumentBackend`, `GraphBackend` all become `pub`, each with doc comments spelling out the atomicity contract a backend must provide (what "the guard read and the value write are effectively one operation" means concretely — a transaction, a native CAS, or an optimistic-retry loop, per Phases 19/22/23/24's own choices).
- These four traits are declared the **stable extension surface** of `conduit-core` — a semver note: a breaking change to any of them is a breaking change to the crate, tracked deliberately, not incidentally.

**Guarantees**
- A community crate can `impl SqlTxn for MyBackend` today and get every SQL mapping feature (facets, deletes, tombstones, resurrection) for free — the trait is the entire contract.

**As built**
- All four traits and their supporting types (`GuardRow`/`Placeholders`/`SqlOutcome`/`SqlPlan`/`SqlWrite` for SQL; `KvFacetGuard`/`KvGuard`/`KvOutcome`/`KvPlan` for KV; `DocumentOutcome`/`DocumentPlan`/`FacetGuard`/`ProjectionGuard` for document; `EdgePlan`/`GraphFacetGuard`/`GraphGuard`/`GraphOutcome`/`NodePlan` for graph) are re-exported from each kind's `mod.rs`, with a doc comment pointing at the owning trait.
- Each trait's doc comment was written and then corrected against the actual code, not just the design intent: an early SQL draft claimed a backend could use "an optimistic-retry loop instead" of a pessimistic lock, which is false — `SqlError` has no conflict variant and `project()` never retries. The final text says a SQL backend needing optimistic retry would have to add that machinery itself, pointing at Redis's and Neo4j's retry loops as the pattern to copy.
- This sub-phase's own "stable extension surface" claim was exercised for real within the same phase (25.3), not left as an assertion — see the MySQL backend below.

---

## Phase 25.2 — Adapter factory registry 🧷 (Completed)

**Scope**
- `AdapterConfig` gains a catch-all: unrecognized `type:` strings are looked up in a runtime `AdapterRegistry` instead of failing to parse.

  ```rust
  pub fn register_adapter_factory(
      type_name: &str,
      factory: impl Fn(serde_yaml::Value) -> Result<Box<dyn StorageAdapter>, ConfigError> + 'static,
  );
  ```

- An embedding application (the "embedded crate" deployment mode, `architecture.md` §1.3) depends on both `conduit-core` and a community crate, calls `register_adapter_factory("mysql", conduit_mysql::factory)` at startup before `build_adapters_from_config` runs, and its config can then use `type: mysql` like any built-in adapter.
- `conduit-cli`, as shipped, has no community backends registered — getting one into the prebuilt binary means building a custom thin binary against the registry (consistent with the existing Thin CLI Pattern: `conduit-cli`'s `main.rs` is already argument-parsing-only, so a fork that adds three lines of registration is a small, legitimate customization, not a maintenance burden).

**Guarantees**
- No dynamic loading (`dlopen`, FFI) — Rust's compile-time extension model: implement the trait, depend on the crate, register the factory. This is deliberate; dynamic plugin loading is unsafe and heavy for a marginal ergonomic gain here.
- A registered community adapter participates in routing, dependency ordering, and dispatch identically to a built-in one — nothing in `dispatch.rs` or `runtime/dependency_graph.rs` distinguishes them (the same invariant Phase 14's "does the abstraction generalize" pass condition established).

**As built**
- `register_adapter_factory`'s real signature requires `factory: impl Fn(...) + Send + Sync + 'static`, not just `+ 'static` as sketched above. The registry is a process-wide `static` (`Lazy<Mutex<HashMap<String, AdapterFactory>>>`), and a `static` must be `Sync`; a factory closure without `Send + Sync` cannot live in it. This is a necessary correction, not a scope change — every realistic factory (opening a connection pool, parsing config) is `Send + Sync` already.
- The factory receives the **entire raw adapter list entry** (`type`, `id`, `priority`, `config`, `capabilities`, `depends_on` — everything the user wrote under `adapters:` for that entry), not just a `config:` sub-block. The doc's own sketch takes a single `serde_yaml::Value` argument and nothing else; since a factory has to build an adapter whose own `id()`/`priority()` match the config, that one `Value` has to carry `id`/`priority` too, so it carries the whole entry. `AdapterConfig::Custom(CustomAdapterConfig)` still separately extracts `id`/`priority`/`capabilities`/`depends_on` for generic routing/dependency-graph use *before* the real adapter is built (mirroring every built-in variant) — the raw `Value` and the extracted fields coexist, not one replacing the other.
- Implementation: `AdapterConfig` derives `Deserialize` on `KnownAdapterConfig` (the original `#[serde(tag = "type")]` enum, renamed and made private) and hand-writes `AdapterConfig`'s own `Deserialize` to try that first, falling back to `AdapterConfig::Custom` for anything not in a fixed list of built-in `type:` strings — so an unrecognized type is a valid parse, not a deserialization error, and can be resolved later (at `build_adapters_from_config` time, per the phase doc's stated timing) after a factory registers for it.
- An unregistered type (or a factory that itself returns `Err`) becomes an internal `FailedAdapter` placeholder pushed by `build_adapters_from_config` — same "store the error, surface it on `handle()`" pattern every connectable built-in backend (Postgres, Redis, MongoDB, Neo4j, MySQL) already uses for a bad URL. This keeps `build_adapters_from_config` infallible (the Zero Panic Rule, and `execute_event`'s existing stable-API contract) and needed one new thing: `StorageKind::Custom`, reported only by this placeholder, never by a real adapter (built-in or registered). The two exhaustive `match` sites over `StorageKind` (`routing.rs`'s `Display` impl, `conduit-cli/src/main.rs`'s `format_storage_kind`) each got one new arm; no other exhaustive match over `StorageKind` or `AdapterConfig` exists in the codebase.
- A new `ConfigError::UnregisteredAdapterType(String)` variant names the missing type in the resulting failure message.
- Verified with two unit tests in `runtime/registry.rs` (an unregistered lookup resolves to `None`; a registered factory is actually invoked with its config) and two integration tests in `tests/registry_adapter_dispatches_identically.rs` (Phase 25.5 — see there).

---

## Phase 25.3 — Reference connector: MySQL 🐬 (Completed)

**Scope (as originally planned)**
- A real, separate example crate (outside this repo's workspace — a linked example or a `contrib/` reference, not a first-party backend) implementing `SqlTxn` for MySQL.
- MySQL is the deliberate stress test: its upsert syntax is `INSERT … ON DUPLICATE KEY UPDATE`, not `ON CONFLICT … DO UPDATE` — genuinely different from both SQLite and Postgres. If the Phase 19.1 `SqlWrite` neutral representation renders cleanly to MySQL's syntax, the seam generalizes past the two dialects it was designed against; if it doesn't, this phase is where that's discovered and the seam is widened.

**Guarantees (as originally planned)**
- `conduit-mysql` passes the same SQL scenario suite (out-of-order, delete, facets, concurrent writers) as SQLite and Postgres, run from its own repo — proof the extension point is sufficient, not just plausible.

**As built — a deliberate scope change, explicitly directed**
- When asked how to handle this sub-phase's stated requirement for a real, separate crate outside the workspace, the direction given was: *"No it should be same as other adapter."* This is an explicit override of this sub-phase's own scope and of the Non-Goals section below ("MySQL is not becoming a first-party backend in this repo"). Per that direction, MySQL shipped as a genuine fifth first-party SQL backend inside `conduit-core` — `crates/conduit-core/src/adapter/sql/mysql.rs` — structured identically to SQLite/Postgres: its own file, wired through `AdapterConfig::MySql`/`factory.rs`/`validation.rs` exactly like Postgres, its own integration test file (`tests/mysql_backend.rs`), and its own CI job (`test-mysql`). It is **not** a separate repository and **not** a documentation-only sketch.
- This changes what "the extension point is sufficient" is proof of: the traits and registry are exercised for real by an in-tree backend built the same way a true out-of-tree one would be, rather than by an actual separate crate. `docs/writing-a-backend.md` names this directly and states that a real community backend follows the identical pattern from its own crate. Building an actual out-of-tree `conduit-mysql` (or another connector) to prove the registry from outside this repo remains open future work (see `phases.md`'s "Beyond Phase 25").
- MySQL's upsert syntax did stress-test `SqlWrite` exactly as predicted: `Placeholders` gained a `MySql` variant (bare, unnumbered `?` — MySQL's wire protocol has no numbered-placeholder syntax, unlike SQLite's `?1`/Postgres's `$1`) and `SqlWrite::render` gained a second upsert-clause branch (`ON DUPLICATE KEY UPDATE col = VALUES(col)` instead of `ON CONFLICT (…) DO UPDATE SET col = excluded.col`). No other part of `SqlWrite`'s shape needed to change — the neutral representation generalized past the two dialects it was designed against, confirming this sub-phase's own hypothesis.
- Unlike Postgres, the MySQL backend does not introspect column types (`information_schema.columns`) before binding. MySQL's wire protocol is comparatively permissive about parameter/column type mismatches — a bound string coerces into a DATE/DATETIME/numeric column under the server's default `sql_mode` — so JSON scalars bind by their own Rust shape instead (`bool` → `Value::Int(0/1)`, a JSON number → `Value::Int`/`Value::Double`, a JSON string → `Value::Bytes`, `null` → `Value::NULL`), the same "trust the server's own coercion" stance SQLite's backend takes. One real consequence, covered by `tests/mysql_backend.rs`'s `mysql_type_coercion` test: a temporal column needs MySQL's own `YYYY-MM-DD HH:MM:SS` string form, not RFC 3339 — there is no Postgres-style `DateTime<Utc>`-typed bind to parse an RFC 3339 string into.
- Multi-writer safety mirrors Postgres exactly: a `REPEATABLE READ` transaction with `SELECT … FOR UPDATE` on the guard row (InnoDB supports the identical row lock) — a real pessimistic lock, not a retry loop, consistent with `SqlTxn`'s documented "no built-in retry path" contract.
- Guard table DDL uses `VARCHAR(191)` (not `TEXT`) for the three primary-key columns (`target_table`, `entity_key`, `facet`). MySQL/InnoDB caps an index key at 3072 bytes; three `TEXT`-like columns in `utf8mb4` (4 bytes/char) would risk exceeding that on a composite primary key, and 191 characters is the long-established safe convention for a `utf8mb4` key column (`191 × 4 = 764` bytes, comfortably under even the older 767-byte per-column limit some MySQL/MariaDB configurations still enforce).
- Dependency added: `mysql = "28"` with `default-features = false, features = ["minimal-rust", "derive", "buffer-pool", "rustls-tls"]` — `minimal-rust` selects flate2's pure-Rust backend (no system zlib dependency) and `rustls-tls` avoids openssl, the same dependency-lightness goal Postgres's `NoTls` serves.
- Verified for real via CI (`test-mysql`, a `mysql:8` service container) — no Docker was available in this development environment, so (as with every other real-database backend added this project) correctness against an actual server could only be confirmed there, not locally.

---

## Phase 25.4 — `docs/writing-a-backend.md` 📚 (Completed)

**Scope**
- The guide: which trait to implement for which storage kind, the atomicity contract in concrete terms, how to reuse the existing guard struct pattern (`GuardState`, and each kind's on-disk/wire guard shape as a *model*, not a requirement — a community backend can shape its own guard storage as long as it round-trips `GuardState`), how to register a factory, and a walkthrough referencing `conduit-mysql`.

**Guarantees**
- A contributor can go from "I want a DynamoDB backend" to a working `impl KvBackend` without reading `conduit-core`'s source.

**As built**
- Written at `docs/writing-a-backend.md`. The walkthrough references the in-tree MySQL backend (`crates/conduit-core/src/adapter/sql/mysql.rs`) rather than a separate `conduit-mysql` crate, per 25.3's scope change, and says so explicitly — including that a real out-of-tree backend follows the identical pattern from its own crate.

---

## Phase 25.5 — Verification 🧪 (Completed)

- In this repo: a test registering a mock `StorageAdapter` factory and confirming it's indistinguishable from a built-in adapter in routing, dependency ordering, and dispatch (`registry_adapter_dispatches_identically.rs`).
- `conduit-mysql`'s own suite lives and runs in its own repo — out of this repo's CI, referenced from `docs/writing-a-backend.md` as the worked example.

**Guarantee under test:** the registry mechanism itself is correct; a specific third-party backend's correctness is that backend's own responsibility, same as any Rust trait implementation.

**As built**
- `tests/registry_adapter_dispatches_identically.rs` has two tests: one registers a mock factory under `id: "sql-primary"`, runs it alongside a real built-in `file` adapter (`id: "doc-readmodel"`, `depends_on: ["sql-primary"]`) through the actual `build_adapters_from_config` → `dispatch` path, and asserts priority sort order, dependency order, and both adapters' outcomes in the execution report — indistinguishable from two built-ins. The second registers nothing and confirms an unregistered `type:` yields exactly one `FailedAdapter` placeholder (not a panic, not a missing adapter) whose failure message names the unregistered type.
- The MySQL backend's own suite (`tests/mysql_backend.rs`) lives in *this* repo, not a separate one — a direct consequence of 25.3's scope change — and does run in this repo's CI (`test-mysql`), unlike what this sub-phase originally described for `conduit-mysql`.

---

## Non-Goals (explicit)

- **No dynamic (dlopen/FFI) plugin loading.** Extension is compile-time: implement, depend, register.
- **No plugin marketplace or registry service.** Discovery is crates.io + the docs guide, nothing bespoke.
- **No guaranteed trait stability before `conduit-core` reaches 1.0.** The traits are the extension surface starting now, but pre-1.0 semver allows breaking changes; documented, not hidden.
- ~~**MySQL is not becoming a first-party backend in this repo.**~~ **Superseded by explicit direction during this phase.** MySQL *did* become a first-party backend in this repo (see 25.3's "As built") — a deliberate, directed deviation from this stated non-goal, not an accidental one. The underlying intent this non-goal was protecting — that the extension point not be *validated only* by the core team's own in-tree work — is still open; see `phases.md`'s "Beyond Phase 25".
- **No sanctioned/certified-connector program.** Community backends are community-maintained; Conduit does not vet, certify, or support them.
