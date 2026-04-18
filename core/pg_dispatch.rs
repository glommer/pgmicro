//! PostgreSQL-specific dispatch logic for `Connection`.
//!
//! These methods handle PG session commands (SET/SHOW → PRAGMA),
//! schema management (CREATE/DROP SCHEMA → ATTACH/DETACH), and
//! PG SQL parsing (pg_query FFI → translator → Turso AST).
//!
//! Extracted from `connection.rs` so that merges from upstream Turso
//! never conflict with PG-only code.

use std::num::NonZero;

use crate::connection::Connection;
use crate::copy::parse_copy_text_format;
use crate::statement::StatementOrigin;
use crate::types::Text;
use crate::{Cmd, LimboError, Result, SqlDialect, Statement, Value};
use turso_parser_pg::translator::{
    is_refresh_matview, try_extract_copy_from, try_extract_create_schema, try_extract_drop_schema,
    try_extract_set, try_extract_show, PgCopyFromStmt, PgCreateSchemaStmt, PgDropSchemaStmt,
    PostgreSQLTranslator,
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

        // REFRESH MATERIALIZED VIEW → no-op (Turso matviews are live)
        if is_refresh_matview(&parse_result) {
            return Ok(Some(self.prepare_sqlite_sql("SELECT 0 WHERE 0")?));
        }

        // COPY table FROM '/path/to/file' → read file, INSERT rows
        if let Some(copy_stmt) = try_extract_copy_from(&parse_result) {
            let rows_inserted = self.handle_pg_copy_from(&copy_stmt)?;
            let stmt = self.prepare_sqlite_sql("SELECT 0 WHERE 0")?;
            stmt.set_n_change(rows_inserted as i64);
            return Ok(Some(stmt));
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
    /// Maps to ATTACH 'turso-postgres-schema-<name>.db' AS "<name>".
    /// The schema database file is created in the same directory as the main database.
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
        let path = self.schema_file_path(&name);
        self.attach_database(&path, &name)
    }

    /// Compute the file path for a schema database.
    /// For file-backed main databases, creates it in the same directory.
    /// For in-memory main databases, creates a file in the current directory.
    fn schema_file_path(&self, schema_name: &str) -> String {
        let main_path = &self.db.path;
        let filename = format!("turso-postgres-schema-{schema_name}.db");
        if main_path == ":memory:" {
            filename
        } else {
            let parent = std::path::Path::new(main_path)
                .parent()
                .unwrap_or_else(|| std::path::Path::new("."));
            parent.join(&filename).to_string_lossy().to_string()
        }
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

    /// Handle COPY FROM file: read tab-delimited data and INSERT rows.
    /// Returns the number of rows inserted.
    fn handle_pg_copy_from(self: &Arc<Self>, stmt: &PgCopyFromStmt) -> Result<usize> {
        let data = std::fs::read_to_string(&stmt.filename).map_err(|e| {
            LimboError::ParseError(format!("COPY FROM: cannot read '{}': {}", stmt.filename, e))
        })?;

        // Determine column info from table
        let table_name = match &stmt.schema_name {
            Some(schema) => format!("\"{schema}\".\"{name}\"", name = stmt.table_name),
            None => format!("\"{}\"", stmt.table_name),
        };
        let column_names = self.get_table_columns(&stmt.table_name, stmt.schema_name.as_deref())?;
        if column_names.is_empty() {
            return Err(LimboError::ParseError(format!(
                "COPY FROM: table '{}' not found or has no columns",
                stmt.table_name
            )));
        }

        // If specific columns are listed, use those; otherwise use all table columns
        let (insert_cols, num_columns) = match &stmt.columns {
            Some(cols) => {
                let col_list = cols
                    .iter()
                    .map(|c| format!("\"{c}\""))
                    .collect::<Vec<_>>()
                    .join(", ");
                (format!(" ({col_list})"), cols.len())
            }
            None => (String::new(), column_names.len()),
        };

        let placeholders = (0..num_columns).map(|_| "?").collect::<Vec<_>>().join(", ");
        let insert_sql = format!("INSERT INTO {table_name}{insert_cols} VALUES ({placeholders})");

        let delimiter = stmt
            .delimiter
            .as_ref()
            .and_then(|d| d.chars().next())
            .unwrap_or('\t');
        let null_string = stmt.null_string.as_deref().unwrap_or("\\N");

        let mut rows = parse_copy_text_format(&data, delimiter, null_string, num_columns)?;

        // Skip header row if requested
        if stmt.header && !rows.is_empty() {
            rows.remove(0);
        }

        let rows_inserted = rows.len();

        // Execute inserts with SQLite dialect, wrapped in a transaction
        let saved_dialect = self.get_sql_dialect();
        self.set_sql_dialect(SqlDialect::Sqlite);
        let result = (|| {
            let mut begin = self.prepare_with_origin("BEGIN", StatementOrigin::Root)?;
            begin.run_ignore_rows()?;

            let mut insert_stmt = self.prepare_with_origin(&insert_sql, StatementOrigin::Root)?;

            for row in &rows {
                for (i, val) in row.iter().enumerate() {
                    let index = NonZero::new(i + 1).unwrap();
                    match val {
                        Some(s) => insert_stmt.bind_at(index, Value::Text(Text::new(s.clone()))),
                        None => insert_stmt.bind_at(index, Value::Null),
                    }
                }
                insert_stmt.run_ignore_rows()?;
                insert_stmt.reset()?;
                insert_stmt.clear_bindings();
            }

            let mut commit = self.prepare_with_origin("COMMIT", StatementOrigin::Root)?;
            commit.run_ignore_rows()?;

            Ok(rows_inserted)
        })();
        self.set_sql_dialect(saved_dialect);
        result
    }

    /// Get column names for a table using PRAGMA table_info.
    fn get_table_columns(
        self: &Arc<Self>,
        table_name: &str,
        schema_name: Option<&str>,
    ) -> Result<Vec<String>> {
        let sql = match schema_name {
            Some(schema) => format!("PRAGMA \"{schema}\".table_info('{table_name}')"),
            None => format!("PRAGMA table_info('{table_name}')"),
        };
        let mut stmt = self.prepare_internal(&sql)?;
        let rows = stmt.run_collect_rows()?;
        // table_info returns: cid, name, type, notnull, dflt_value, pk
        Ok(rows
            .into_iter()
            .filter_map(|row| match row.get(1) {
                Some(Value::Text(t)) => Some(t.as_str().to_string()),
                _ => None,
            })
            .collect())
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
