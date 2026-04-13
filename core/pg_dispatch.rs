//! PostgreSQL-specific dispatch logic for `Connection`.
//!
//! These methods handle PG session commands (SET/SHOW → PRAGMA),
//! schema management (CREATE/DROP SCHEMA → ATTACH/DETACH), and
//! PG SQL parsing (pg_query FFI → translator → Turso AST).
//!
//! Extracted from `connection.rs` so that merges from upstream Turso
//! never conflict with PG-only code.

use crate::connection::Connection;
use crate::statement::StatementOrigin;
use crate::{Cmd, LimboError, Result, SqlDialect, Statement, Value};
use turso_parser_pg::translator::{
    try_extract_create_schema, try_extract_drop_schema, try_extract_set, try_extract_show,
    PgCreateSchemaStmt, PgDropSchemaStmt, PostgreSQLTranslator,
};

use crate::sync::Arc;

impl Connection {
    /// Parse PostgreSQL SQL using pg_query and translate to Turso AST.
    pub(crate) fn parse_postgresql_sql(&self, sql: &str) -> Result<Option<Cmd>> {
        let parse_result =
            turso_parser_pg::parse(sql).map_err(|e| LimboError::ParseError(e.to_string()))?;

        let translator = PostgreSQLTranslator::new();
        let stmt = translator
            .translate(&parse_result)
            .map_err(|e| LimboError::ParseError(e.to_string()))?;

        Ok(Some(Cmd::Stmt(stmt)))
    }

    /// Handle PG session/schema commands that need connection state.
    /// Handles: SET (→ PRAGMA), SHOW (→ PRAGMA), CREATE/DROP SCHEMA.
    /// Returns Some(Statement) if handled, None to fall through to standard parse path.
    pub(crate) fn try_prepare_pg(self: &Arc<Self>, sql: &str) -> Result<Option<Statement>> {
        // If pg_query can't parse the SQL, return None to fall through.
        let parse_result = match turso_parser_pg::parse(sql) {
            Ok(result) => result,
            Err(_) => return Ok(None),
        };

        // SET name = value → PRAGMA name = value
        if let Some(set_stmt) = try_extract_set(&parse_result) {
            let pragma_sql = format!("PRAGMA {} = {}", set_stmt.name, set_stmt.value);
            return Ok(Some(self.prepare_sqlite_sql(&pragma_sql)?));
        }

        // SHOW name → PRAGMA name
        if let Some(show_stmt) = try_extract_show(&parse_result) {
            let pragma_sql = format!("PRAGMA {}", show_stmt.name);
            return Ok(Some(self.prepare_sqlite_sql(&pragma_sql)?));
        }

        // CREATE SCHEMA → ATTACH database
        if let Some(cs) = try_extract_create_schema(&parse_result) {
            self.handle_pg_create_schema(&cs)?;
            return Ok(Some(self.prepare_sqlite_sql("SELECT 0 WHERE 0")?));
        }

        // DROP SCHEMA → DROP tables + DETACH database
        if let Some(ds) = try_extract_drop_schema(&parse_result) {
            self.handle_pg_drop_schema(&ds)?;
            return Ok(Some(self.prepare_sqlite_sql("SELECT 0 WHERE 0")?));
        }

        Ok(None)
    }

    /// Parse SQL with the SQLite parser without changing the connection dialect.
    /// Used for SET/SHOW → PRAGMA translation where the current dialect must be
    /// preserved (PRAGMAs like sql_dialect read the dialect at compile time).
    fn prepare_sqlite_sql(self: &Arc<Self>, sql: &str) -> Result<Statement> {
        self.prepare_with_origin(sql, StatementOrigin::InternalHelper)
    }

    /// Handle CREATE SCHEMA in PostgreSQL mode.
    /// Maps to ATTACH with an in-memory database.
    /// File-backed persistence (turso-postgres-schema-<name>.db) is handled by the CLI layer.
    fn handle_pg_create_schema(self: &Arc<Self>, stmt: &PgCreateSchemaStmt) -> Result<()> {
        let name = stmt.name.to_lowercase();
        if name == "public" {
            // "public" always exists
            if stmt.if_not_exists {
                return Ok(());
            }
            return Err(LimboError::ParseError(format!(
                "schema \"{name}\" already exists"
            )));
        }
        if self.is_attached(&name) {
            if stmt.if_not_exists {
                return Ok(());
            }
            return Err(LimboError::ParseError(format!(
                "schema \"{name}\" already exists"
            )));
        }
        self.attach_database(":memory:", &name)
    }

    /// Handle DROP SCHEMA in PostgreSQL mode.
    /// For "public": drops all user tables from main DB.
    /// For other schemas: drops all tables, then DETACHes.
    fn handle_pg_drop_schema(self: &Arc<Self>, stmt: &PgDropSchemaStmt) -> Result<()> {
        let name = stmt.name.to_lowercase();
        if name == "public" {
            return self.handle_pg_drop_schema_public(stmt.cascade);
        }
        if !self.is_attached(&name) {
            if stmt.if_exists {
                return Ok(());
            }
            return Err(LimboError::ParseError(format!(
                "schema \"{name}\" does not exist"
            )));
        }
        if stmt.cascade {
            self.drop_all_tables_in_schema(&name)?;
        }
        self.detach_database(&name)
    }

    /// Drop all user tables in the main ("public") schema.
    fn handle_pg_drop_schema_public(self: &Arc<Self>, cascade: bool) -> Result<()> {
        let table_names = self.list_user_tables(None)?;
        if !cascade && !table_names.is_empty() {
            return Err(LimboError::ParseError(
                "cannot drop schema \"public\" because other objects depend on it".to_string(),
            ));
        }
        // Use Root origin (not InternalHelper) because DROP TABLE is DDL that
        // needs a write transaction. InternalHelper sets is_nested which prevents
        // the Transaction opcode from upgrading to a write tx.
        let saved_dialect = self.get_sql_dialect();
        self.set_sql_dialect(SqlDialect::Sqlite);
        let result = (|| {
            for table_name in table_names {
                let sql = format!("DROP TABLE \"{table_name}\"");
                let mut stmt = self.prepare_with_origin(&sql, StatementOrigin::Root)?;
                stmt.run_ignore_rows()?;
            }
            Ok(())
        })();
        self.set_sql_dialect(saved_dialect);
        result
    }

    /// Drop all tables in an attached schema.
    fn drop_all_tables_in_schema(self: &Arc<Self>, schema_name: &str) -> Result<()> {
        let table_names = self.list_user_tables(Some(schema_name))?;
        let saved_dialect = self.get_sql_dialect();
        self.set_sql_dialect(SqlDialect::Sqlite);
        let result = (|| {
            for table_name in table_names {
                let sql = format!("DROP TABLE \"{schema_name}\".\"{table_name}\"");
                let mut stmt = self.prepare_with_origin(&sql, StatementOrigin::Root)?;
                stmt.run_ignore_rows()?;
            }
            Ok(())
        })();
        self.set_sql_dialect(saved_dialect);
        result
    }

    /// List user-visible table names in a schema.
    /// If schema_name is None, queries main DB's sqlite_schema.
    /// If schema_name is Some(name), queries name.sqlite_schema.
    fn list_user_tables(self: &Arc<Self>, schema_name: Option<&str>) -> Result<Vec<String>> {
        let filter =
            "type='table' AND name NOT LIKE 'sqlite_%' AND name NOT LIKE '__turso_internal_%'";
        let sql = match schema_name {
            Some(name) => format!("SELECT name FROM \"{name}\".sqlite_schema WHERE {filter}"),
            None => format!("SELECT name FROM sqlite_schema WHERE {filter}"),
        };
        let mut stmt = self.prepare_internal(&sql)?;
        let rows = stmt.run_collect_rows()?;
        Ok(rows
            .into_iter()
            .filter_map(|row| match row.first() {
                Some(Value::Text(t)) => Some(t.as_str().to_string()),
                _ => None,
            })
            .collect())
    }
}
