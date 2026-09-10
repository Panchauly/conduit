//! Phase 19.2–19.4: the Postgres SQL backend.
//!
//! Same `SqlMapping` / `decide()` / guard model as SQLite — this file supplies
//! only what a driver genuinely owns: an r2d2 connection pool, a
//! `READ COMMITTED` transaction with `SELECT … FOR UPDATE` on the guard row
//! (real multi-writer safety, Phase 19.4), `$N` placeholders, typed column
//! binding (Phase 19.3), and the Postgres guard-table DDL.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};
use postgres::NoTls;
use postgres::types::ToSql;
use r2d2_postgres::PostgresConnectionManager;
use serde_json::Value;

use super::adapter::SqlError;
use super::exec::{self, GuardRow, Placeholders, SqlOutcome, SqlPlan, SqlTxn, SqlWrite};
use super::runtime::SqlRuntimeBuilder;
use crate::adapter::{AdapterError, AdapterResult, GuardState, SkipReason, StorageAdapter};
use crate::event::Event;
use crate::routing::StorageKind;
use crate::runtime::config::MigrationPolicy;
use crate::upcast::UpcasterRegistry;

type PgPool = r2d2::Pool<PostgresConnectionManager<NoTls>>;

/// Introspected column type — enough to bind a JSON scalar correctly (Phase 19.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PgColType {
    Text,
    Int,
    Float,
    Bool,
    Timestamptz,
    Timestamp,
    Date,
    /// numeric / uuid / json / jsonb / anything else — bound as text and left
    /// to Postgres's assignment cast.
    Other,
}

impl PgColType {
    fn from_data_type(dt: &str) -> Self {
        match dt {
            "smallint" | "integer" | "bigint" => PgColType::Int,
            "real" | "double precision" => PgColType::Float,
            "boolean" => PgColType::Bool,
            "timestamp with time zone" => PgColType::Timestamptz,
            "timestamp without time zone" => PgColType::Timestamp,
            "date" => PgColType::Date,
            "character varying" | "character" | "text" | "name" | "citext" => PgColType::Text,
            _ => PgColType::Other,
        }
    }
}

type ColTypes = HashMap<String, PgColType>;

/// Postgres SQL adapter (Phase 19). The first adapter **without** a
/// single-writer assumption — concurrent dispatch threads and concurrent
/// Conduit instances serialize on the `SELECT … FOR UPDATE` guard row.
pub struct PostgresAdapter {
    id: String,
    priority: u32,
    /// `Err` (a stored message) when the URL didn't parse — every `handle()`
    /// then fails cleanly rather than the factory being fallible.
    pool: Result<PgPool, String>,
    builder: SqlRuntimeBuilder,
    upcasters: Arc<UpcasterRegistry>,
    migration_policy: MigrationPolicy,
    /// Per-target-table `information_schema.columns` map, introspected once.
    col_types: Arc<Mutex<HashMap<String, Arc<ColTypes>>>>,
}

impl PostgresAdapter {
    /// Build the pool from a connection URL (`postgres://user:pass@host/db`).
    /// Infallible — a bad URL is stored and surfaced per-event, keeping
    /// `build_adapters_from_config` free of a `Result`. The pool is
    /// `build_unchecked` (lazy connect): connectivity problems surface on the
    /// first real write, not at construction.
    pub fn new(
        id: String,
        url: &str,
        pool_size: u32,
        priority: u32,
        builder: SqlRuntimeBuilder,
        upcasters: Arc<UpcasterRegistry>,
        migration_policy: MigrationPolicy,
    ) -> Self {
        let pool = url
            .parse::<postgres::Config>()
            .map_err(|e| format!("invalid postgres url: {e}"))
            .map(|config| {
                let manager = PostgresConnectionManager::new(config, NoTls);
                r2d2::Pool::builder()
                    .max_size(pool_size.max(1))
                    .build_unchecked(manager)
            });
        Self {
            id,
            priority,
            pool,
            builder,
            upcasters,
            migration_policy,
            col_types: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    fn failure(&self, error: AdapterError) -> AdapterResult {
        AdapterResult::failure(self.id.clone(), StorageKind::Sql, error)
    }

    /// Column types for `table`, introspected + cached on first use.
    fn column_types(
        &self,
        client: &mut postgres::Client,
        table: &str,
    ) -> Result<Arc<ColTypes>, SqlError> {
        if let Some(c) = self
            .col_types
            .lock()
            .ok()
            .and_then(|m| m.get(table).cloned())
        {
            return Ok(c);
        }
        let rows = client
            .query(
                "SELECT column_name, data_type FROM information_schema.columns WHERE table_name = $1",
                &[&table],
            )
            .map_err(|e| SqlError::ExecutionFailed(e.to_string()))?;
        let mut map = ColTypes::new();
        for row in rows {
            let name: String = row.get(0);
            let dt: String = row.get(1);
            map.insert(name, PgColType::from_data_type(&dt));
        }
        let arc = Arc::new(map);
        if let Ok(mut m) = self.col_types.lock() {
            m.insert(table.to_string(), Arc::clone(&arc));
        }
        Ok(arc)
    }
}

impl StorageAdapter for PostgresAdapter {
    fn kind(&self) -> StorageKind {
        StorageKind::Sql
    }
    fn id(&self) -> &str {
        &self.id
    }
    fn priority(&self) -> u32 {
        self.priority
    }

    fn handle(&self, event: &Event) -> AdapterResult {
        let projection = match self.builder.build(event, &self.upcasters) {
            Ok(p) => p,
            Err(SqlError::UnsupportedVersion {
                from_version,
                to_version,
                reason,
                ..
            }) => {
                return match self.migration_policy {
                    MigrationPolicy::Strict => AdapterResult::unsupported_version(
                        self.id.clone(),
                        StorageKind::Sql,
                        reason,
                        from_version,
                        to_version,
                    ),
                    MigrationPolicy::IgnoreUnmatched => AdapterResult::skipped_versioned(
                        self.id.clone(),
                        StorageKind::Sql,
                        SkipReason::UnsupportedVersion,
                        from_version,
                        to_version,
                    ),
                };
            }
            Err(e) => return self.failure(AdapterError::WriteFailed(e.to_string())),
        };

        let source_version = projection.source_version;
        let projected_version = projection.projected_version;
        let plan = SqlPlan {
            table: projection.table,
            entity_key: projection.entity_key,
            key_values: projection.key_values,
            primary_key_columns: projection.primary_key_columns,
            on_existing: projection.on_existing,
            operation: projection.operation,
            permanent: projection.permanent,
            facet: projection.facet,
            write: projection.write,
        };

        let pool = match &self.pool {
            Ok(p) => p,
            Err(e) => return self.failure(AdapterError::WriteFailed(e.clone())),
        };
        let mut client = match pool.get() {
            Ok(c) => c,
            Err(e) => return self.failure(AdapterError::WriteFailed(e.to_string())),
        };

        let col_types = match self.column_types(&mut client, &plan.table) {
            Ok(c) => c,
            Err(e) => return self.failure(AdapterError::WriteFailed(e.to_string())),
        };

        let tx = match client
            .build_transaction()
            .isolation_level(postgres::IsolationLevel::ReadCommitted)
            .start()
        {
            Ok(tx) => tx,
            Err(e) => return self.failure(AdapterError::WriteFailed(e.to_string())),
        };

        match exec::project(PostgresTxn { tx, col_types }, &plan, event) {
            Ok(SqlOutcome::Created) => AdapterResult::created_versioned(
                self.id.clone(),
                StorageKind::Sql,
                source_version,
                projected_version,
            ),
            Ok(SqlOutcome::Updated) => AdapterResult::updated_versioned(
                self.id.clone(),
                StorageKind::Sql,
                source_version,
                projected_version,
            ),
            Ok(SqlOutcome::Deleted) => AdapterResult::deleted_versioned(
                self.id.clone(),
                StorageKind::Sql,
                source_version,
                projected_version,
            ),
            Ok(SqlOutcome::Skipped(reason)) => AdapterResult::skipped_versioned(
                self.id.clone(),
                StorageKind::Sql,
                reason,
                source_version,
                projected_version,
            ),
            Err(e) => self.failure(AdapterError::WriteFailed(e.to_string())),
        }
    }
}

// ---------------------------------------------------------------------------
// PostgresTxn
// ---------------------------------------------------------------------------

pub struct PostgresTxn<'a> {
    tx: postgres::Transaction<'a>,
    col_types: Arc<ColTypes>,
}

fn pg_err(e: postgres::Error) -> SqlError {
    SqlError::ExecutionFailed(e.to_string())
}

/// A single owned, bindable value.
enum Bound {
    Null,
    Text(String),
    Int(i64),
    Float(f64),
    Bool(bool),
    Tstz(DateTime<Utc>),
    Ts(NaiveDateTime),
    Date(NaiveDate),
}

impl Bound {
    fn as_sql(&self) -> &(dyn ToSql + Sync) {
        match self {
            Bound::Null => &Option::<&str>::None,
            Bound::Text(s) => s,
            Bound::Int(v) => v,
            Bound::Float(v) => v,
            Bound::Bool(v) => v,
            Bound::Tstz(v) => v,
            Bound::Ts(v) => v,
            Bound::Date(v) => v,
        }
    }
}

/// Convert a JSON scalar to the bind form for `col_type` (Phase 19.3).
fn bind_value(v: &Value, col_type: PgColType) -> Result<Bound, SqlError> {
    if v.is_null() {
        return Ok(Bound::Null);
    }
    match col_type {
        PgColType::Bool => v
            .as_bool()
            .map(Bound::Bool)
            .ok_or_else(|| SqlError::ExecutionFailed(format!("expected boolean, got {v}"))),
        PgColType::Int => v
            .as_i64()
            .map(Bound::Int)
            .ok_or_else(|| SqlError::ExecutionFailed(format!("expected integer, got {v}"))),
        PgColType::Float => v
            .as_f64()
            .map(Bound::Float)
            .ok_or_else(|| SqlError::ExecutionFailed(format!("expected number, got {v}"))),
        PgColType::Timestamptz => v
            .as_str()
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| Bound::Tstz(dt.with_timezone(&Utc)))
            .ok_or_else(|| {
                SqlError::ExecutionFailed(format!("expected an RFC 3339 timestamp, got {v}"))
            }),
        PgColType::Timestamp => v
            .as_str()
            .and_then(|s| {
                NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S")
                    .or_else(|_| NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S"))
                    .ok()
            })
            .map(Bound::Ts)
            .ok_or_else(|| SqlError::ExecutionFailed(format!("expected a timestamp, got {v}"))),
        PgColType::Date => v
            .as_str()
            .and_then(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok())
            .map(Bound::Date)
            .ok_or_else(|| SqlError::ExecutionFailed(format!("expected a date, got {v}"))),
        PgColType::Text | PgColType::Other => Ok(Bound::Text(exec::scalar_to_string(v))),
    }
}

impl PostgresTxn<'_> {
    fn bind_write(&self, w: &SqlWrite) -> Result<Vec<Bound>, SqlError> {
        let mut out = Vec::with_capacity(w.values.len());
        for (col, val) in w.columns.iter().zip(&w.values) {
            let ct = self.col_types.get(col).copied().ok_or_else(|| {
                SqlError::ExecutionFailed(format!(
                    "column '{col}' of mapping is not in table '{}'",
                    w.table
                ))
            })?;
            out.push(bind_value(val, ct)?);
        }
        Ok(out)
    }
}

impl SqlTxn for PostgresTxn<'_> {
    fn ensure_guard_table(&mut self) -> Result<(), SqlError> {
        self.tx
            .batch_execute(
                "CREATE TABLE IF NOT EXISTS conduit_projection_state (
                    target_table  text        NOT NULL,
                    entity_key    text        NOT NULL,
                    facet         text        NOT NULL DEFAULT '',
                    last_sequence bigint      NOT NULL,
                    last_event_id text        NOT NULL,
                    deleted       boolean     NOT NULL DEFAULT false,
                    permanent     boolean     NOT NULL DEFAULT false,
                    processed_at  timestamptz NOT NULL DEFAULT now(),
                    PRIMARY KEY (target_table, entity_key, facet)
                )",
            )
            .map_err(pg_err)
    }

    fn read_guard_lane(
        &mut self,
        table: &str,
        entity_key: &str,
        facet: &str,
        lock: bool,
    ) -> Result<Option<GuardState>, SqlError> {
        // Phase 19.4: FOR UPDATE serializes concurrent writers on this row.
        let sql = if lock {
            "SELECT last_sequence, deleted, permanent FROM conduit_projection_state
             WHERE target_table = $1 AND entity_key = $2 AND facet = $3 FOR UPDATE"
        } else {
            "SELECT last_sequence, deleted, permanent FROM conduit_projection_state
             WHERE target_table = $1 AND entity_key = $2 AND facet = $3"
        };
        let row = self
            .tx
            .query_opt(sql, &[&table, &entity_key, &facet])
            .map_err(pg_err)?;
        Ok(row.map(|r| GuardState {
            last_sequence: r.get::<_, i64>(0) as u64,
            deleted: r.get(1),
            permanent: r.get(2),
        }))
    }

    fn execute_write(&mut self, w: &SqlWrite) -> Result<(), SqlError> {
        let sql = w.render(Placeholders::Dollar);
        let bounds = self.bind_write(w)?;
        let params: Vec<&(dyn ToSql + Sync)> = bounds.iter().map(|b| b.as_sql()).collect();
        self.tx
            .execute(sql.as_str(), &params)
            .map(|_| ())
            .map_err(pg_err)
    }

    fn delete_row(&mut self, table: &str, cols: &[String], vals: &[Value]) -> Result<(), SqlError> {
        let where_clause = cols
            .iter()
            .enumerate()
            .map(|(i, c)| format!("{c} = ${}", i + 1))
            .collect::<Vec<_>>()
            .join(" AND ");
        let sql = format!("DELETE FROM {table} WHERE {where_clause}");
        // The key columns' types come from the same introspected map.
        let mut bounds = Vec::with_capacity(vals.len());
        for (col, v) in cols.iter().zip(vals) {
            let ct = self.col_types.get(col).copied().unwrap_or(PgColType::Text);
            bounds.push(bind_value(v, ct)?);
        }
        let params: Vec<&(dyn ToSql + Sync)> = bounds.iter().map(|b| b.as_sql()).collect();
        self.tx
            .execute(sql.as_str(), &params)
            .map(|_| ())
            .map_err(pg_err)
    }

    fn upsert_guard_row(&mut self, row: &GuardRow) -> Result<(), SqlError> {
        self.tx
            .execute(
                "INSERT INTO conduit_projection_state
                    (target_table, entity_key, facet, last_sequence, last_event_id, deleted, permanent, processed_at)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, now())
                 ON CONFLICT (target_table, entity_key, facet) DO UPDATE SET
                    last_sequence = excluded.last_sequence,
                    last_event_id = excluded.last_event_id,
                    deleted = excluded.deleted,
                    permanent = excluded.permanent,
                    processed_at = excluded.processed_at",
                &[
                    &row.table,
                    &row.entity_key,
                    &row.facet,
                    &(row.last_sequence as i64),
                    &row.last_event_id,
                    &row.deleted,
                    &row.permanent,
                ],
            )
            .map(|_| ())
            .map_err(pg_err)
    }

    fn bump_guard_lane(
        &mut self,
        table: &str,
        entity_key: &str,
        facet: &str,
        seq: u64,
        event_id: &str,
    ) -> Result<(), SqlError> {
        self.tx
            .execute(
                "UPDATE conduit_projection_state
                    SET last_sequence = $4, last_event_id = $5, processed_at = now()
                 WHERE target_table = $1 AND entity_key = $2 AND facet = $3",
                &[&table, &entity_key, &facet, &(seq as i64), &event_id],
            )
            .map(|_| ())
            .map_err(pg_err)
    }

    fn cascade_facets(
        &mut self,
        table: &str,
        entity_key: &str,
        deleted: bool,
        permanent: bool,
        seq: u64,
        event_id: &str,
    ) -> Result<(), SqlError> {
        if deleted {
            self.tx.execute(
                "UPDATE conduit_projection_state
                    SET deleted = true, permanent = $4, last_sequence = $3,
                        last_event_id = $5, processed_at = now()
                 WHERE target_table = $1 AND entity_key = $2 AND facet <> ''",
                &[&table, &entity_key, &(seq as i64), &permanent, &event_id],
            )
        } else {
            self.tx.execute(
                "UPDATE conduit_projection_state
                    SET deleted = false, permanent = false
                 WHERE target_table = $1 AND entity_key = $2 AND facet <> ''",
                &[&table, &entity_key],
            )
        }
        .map(|_| ())
        .map_err(pg_err)
    }

    fn commit(self) -> Result<(), SqlError> {
        self.tx.commit().map_err(pg_err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_type_names_map_to_bind_categories() {
        assert_eq!(PgColType::from_data_type("bigint"), PgColType::Int);
        assert_eq!(PgColType::from_data_type("integer"), PgColType::Int);
        assert_eq!(PgColType::from_data_type("boolean"), PgColType::Bool);
        assert_eq!(
            PgColType::from_data_type("double precision"),
            PgColType::Float
        );
        assert_eq!(
            PgColType::from_data_type("timestamp with time zone"),
            PgColType::Timestamptz
        );
        assert_eq!(PgColType::from_data_type("date"), PgColType::Date);
        assert_eq!(PgColType::from_data_type("text"), PgColType::Text);
        assert_eq!(
            PgColType::from_data_type("character varying"),
            PgColType::Text
        );
        assert_eq!(PgColType::from_data_type("uuid"), PgColType::Other);
        assert_eq!(PgColType::from_data_type("jsonb"), PgColType::Other);
    }

    #[test]
    fn bind_value_coerces_by_column_type() {
        use serde_json::json;
        assert!(matches!(
            bind_value(&json!(42), PgColType::Int).unwrap(),
            Bound::Int(42)
        ));
        assert!(matches!(
            bind_value(&json!(true), PgColType::Bool).unwrap(),
            Bound::Bool(true)
        ));
        assert!(matches!(
            bind_value(&json!("2026-01-02T03:04:05Z"), PgColType::Timestamptz).unwrap(),
            Bound::Tstz(_)
        ));
        assert!(matches!(
            bind_value(&json!("2026-01-02"), PgColType::Date).unwrap(),
            Bound::Date(_)
        ));
        assert!(matches!(
            bind_value(&json!(null), PgColType::Int).unwrap(),
            Bound::Null
        ));
        // wrong shape for the column → a clear error, not a silent coercion
        assert!(bind_value(&json!("not a number"), PgColType::Int).is_err());
        assert!(bind_value(&json!("nonsense"), PgColType::Timestamptz).is_err());
    }
}
