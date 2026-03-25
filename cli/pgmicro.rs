// pgmicro — PostgreSQL-compatible micro database CLI
// Standalone binary with PG-style backslash commands.
#![allow(clippy::arc_with_non_send_sync)]

mod config;
mod helper;
mod pg_server;
mod read_state_machine;

// Stubs for shared modules that reference crate-level types from the tursodb binary.
// pgmicro doesn't use these codepaths, but the types must exist so shared code compiles.
mod commands {
    #[derive(clap::Parser, Debug)]
    #[command(name = "pgmicro-stub", disable_help_flag(true))]
    pub struct CommandParser {}
}
mod input {
    #[derive(Copy, Clone, Debug, PartialEq, Eq)]
    #[allow(dead_code)]
    pub enum OutputMode {
        List,
        Pretty,
        Line,
    }
}

use clap::Parser;
use comfy_table::{Attribute, Cell, CellAlignment, ContentArrangement, Row, Table};
use config::{TableConfig, CONFIG_DIR};
use helper::LimboHelper;
use pg_server::TursoPgServer;
use read_state_machine::ReadState;
use rustyline::error::ReadlineError;
use rustyline::history::DefaultHistory;
use rustyline::Editor;
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock};
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;
use turso_core::{
    Connection, Database, DatabaseOpts, LimboError, OpenFlags, SqlDialect, Statement, Value,
};

// ---------------------------------------------------------------------------
// Statics
// ---------------------------------------------------------------------------

pub static HOME_DIR: LazyLock<PathBuf> =
    LazyLock::new(|| dirs::home_dir().expect("Could not determine home directory"));

pub static HISTORY_FILE: LazyLock<PathBuf> = LazyLock::new(|| HOME_DIR.join(".pgmicro_history"));

const PROMPT: &str = "pgmicro> ";
const PROMPT_CONT: &str = "pgmicro-> ";

// ---------------------------------------------------------------------------
// CLI options
// ---------------------------------------------------------------------------

#[derive(Parser, Debug)]
#[command(name = "pgmicro")]
#[command(author, version, about = "PostgreSQL-compatible micro database")]
struct Opts {
    #[clap(index = 1, help = "Database file", default_value = ":memory:")]
    database: Option<PathBuf>,

    #[clap(index = 2, help = "Optional SQL command to execute")]
    sql: Option<String>,

    #[clap(short, long, help = "Don't display program information on start")]
    quiet: bool,

    #[clap(short = 'v', long, help = "Select VFS")]
    vfs: Option<String>,

    #[clap(long, help = "Open the database in read-only mode")]
    readonly: bool,

    #[clap(short = 't', long, help = "Specify output file for log traces")]
    tracing_output: Option<String>,

    #[clap(
        long,
        help = "Start PostgreSQL wire protocol server at given address (e.g. 0.0.0.0:5432)"
    )]
    server: Option<String>,
}

// ---------------------------------------------------------------------------
// Database setup
// ---------------------------------------------------------------------------

fn open_database(
    db_path: &str,
    vfs: Option<&String>,
    readonly: bool,
) -> anyhow::Result<(Arc<dyn turso_core::IO>, Arc<Connection>)> {
    let db_opts = DatabaseOpts::new()
        .with_views(true)
        .with_custom_types(true)
        .with_encryption(true)
        .with_index_method(true)
        .with_autovacuum(true)
        .with_attach(true)
        .with_generated_columns(true)
        .with_postgres(true);

    let flags = if readonly {
        OpenFlags::default().union(OpenFlags::ReadOnly)
    } else {
        OpenFlags::default()
    };

    let (io, db) = Database::open_new(db_path, vfs, flags, db_opts.turso_cli(), None)?;
    let conn = db.connect()?;
    conn.set_sql_dialect(SqlDialect::Postgres);
    Ok((io, conn))
}

/// Discover and attach existing PG schema database files in the same directory.
fn auto_attach_pg_schemas(conn: &Arc<Connection>, db_file: &str) {
    if db_file == ":memory:" {
        return;
    }
    let dir = Path::new(db_file).parent().unwrap_or(Path::new("."));
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let file_name = entry.file_name();
        let Some(name) = file_name.to_str() else {
            continue;
        };
        let Some(schema) = name
            .strip_prefix("turso-postgres-schema-")
            .and_then(|s| s.strip_suffix(".db"))
        else {
            continue;
        };
        let path = entry.path().to_string_lossy().to_string();
        let sql = format!("ATTACH '{path}' AS \"{schema}\"");
        tracing::info!("Auto-attaching PG schema '{}' from {}", schema, path);
        if let Err(e) = conn.execute(&sql) {
            tracing::warn!("Failed to attach schema '{}': {}", schema, e);
        }
    }
}

// ---------------------------------------------------------------------------
// Tracing setup
// ---------------------------------------------------------------------------

fn init_tracing(opts: &Opts) -> Result<WorkerGuard, std::io::Error> {
    let (non_blocking, guard) = if let Some(ref path) = opts.tracing_output {
        let file = std::fs::File::create(path)?;
        tracing_appender::non_blocking(file)
    } else {
        tracing_appender::non_blocking(std::io::sink())
    };
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::registry()
        .with(env_filter)
        .with(tracing_subscriber::fmt::layer().with_writer(non_blocking))
        .init();
    Ok(guard)
}

// ---------------------------------------------------------------------------
// Meta-commands
// ---------------------------------------------------------------------------

fn cmd_help(w: &mut dyn Write) {
    let _ = writeln!(w, "Meta-commands:");
    let _ = writeln!(w, "  \\dt            List tables");
    let _ = writeln!(w, "  \\d <table>     Describe table columns");
    let _ = writeln!(w, "  \\l             List databases");
    let _ = writeln!(w, "  \\conninfo      Show connection info");
    let _ = writeln!(w, "  \\?             Show this help");
    let _ = writeln!(w, "  \\q             Quit");
}

fn cmd_list_tables(conn: &Arc<Connection>, w: &mut dyn Write) {
    let sql = "SELECT tablename FROM pg_tables WHERE schemaname = 'public' ORDER BY tablename";
    match conn.query(sql) {
        Ok(Some(mut rows)) => {
            let mut table = Table::new();
            table.set_content_arrangement(ContentArrangement::Dynamic);
            table.set_header(vec![Cell::new("Table").add_attribute(Attribute::Bold)]);
            let _ = rows.run_with_row_callback(|row| {
                let name = row.get_value(0).to_string();
                table.add_row(vec![Cell::new(&name)]);
                Ok(())
            });
            if !table.is_empty() {
                let _ = writeln!(w, "{table}");
            } else {
                let _ = writeln!(w, "No tables found.");
            }
        }
        Ok(None) => {
            let _ = writeln!(w, "No tables found.");
        }
        Err(e) => {
            let _ = writeln!(w, "Error: {e}");
        }
    }
}

fn cmd_describe_table(conn: &Arc<Connection>, table_name: &str, w: &mut dyn Write) {
    // Join pg_attribute with pg_class and pg_type to get column info.
    // attnum > 0 filters out system columns.
    let safe_name = table_name.replace('\'', "''");
    let sql = format!(
        "SELECT a.attname, t.typname, a.attnotnull, a.atthasdef, a.attnum \
         FROM pg_attribute a \
         JOIN pg_class c ON a.attrelid = c.oid \
         JOIN pg_type t ON a.atttypid = t.oid \
         WHERE c.relname = '{safe_name}' AND a.attnum > 0 AND a.attisdropped = 0 \
         ORDER BY a.attnum"
    );
    match conn.query(&sql) {
        Ok(Some(mut rows)) => {
            let mut table = Table::new();
            table.set_content_arrangement(ContentArrangement::Dynamic);
            table.set_header(vec![
                Cell::new("Column").add_attribute(Attribute::Bold),
                Cell::new("Type").add_attribute(Attribute::Bold),
                Cell::new("Nullable").add_attribute(Attribute::Bold),
                Cell::new("Default").add_attribute(Attribute::Bold),
            ]);
            let mut found = false;
            let _ = rows.run_with_row_callback(|row| {
                found = true;
                let col_name = row.get_value(0).to_string();
                let col_type = row.get_value(1).to_string();
                let notnull = row.get_value(2).to_string();
                let nullable = if notnull == "1" { "NOT NULL" } else { "NULL" };
                let hasdef = row.get_value(3).to_string();
                let default_str = if hasdef == "1" { "YES" } else { "" };
                table.add_row(vec![
                    Cell::new(&col_name),
                    Cell::new(&col_type),
                    Cell::new(nullable),
                    Cell::new(default_str),
                ]);
                Ok(())
            });
            if found {
                let _ = writeln!(w, "Table: {table_name}");
                let _ = writeln!(w, "{table}");
            } else {
                let _ = writeln!(w, "Table '{table_name}' not found.");
            }
        }
        Ok(None) => {
            let _ = writeln!(w, "Table '{table_name}' not found.");
        }
        Err(e) => {
            let _ = writeln!(w, "Error: {e}");
        }
    }
}

fn cmd_list_databases(conn: &Arc<Connection>, w: &mut dyn Write) {
    let sql = "SELECT datname FROM pg_database ORDER BY datname";
    match conn.query(sql) {
        Ok(Some(mut rows)) => {
            let mut table = Table::new();
            table.set_content_arrangement(ContentArrangement::Dynamic);
            table.set_header(vec![Cell::new("Name").add_attribute(Attribute::Bold)]);
            let _ = rows.run_with_row_callback(|row| {
                let name = row.get_value(0).to_string();
                table.add_row(vec![Cell::new(&name)]);
                Ok(())
            });
            let _ = writeln!(w, "{table}");
        }
        Ok(None) => {}
        Err(e) => {
            let _ = writeln!(w, "Error: {e}");
        }
    }
}

fn cmd_conninfo(db_file: &str, w: &mut dyn Write) {
    let _ = writeln!(w, "Database: {db_file}");
    let _ = writeln!(w, "Dialect:  PostgreSQL");
}

/// Dispatch a backslash meta-command. Returns true if the REPL should quit.
fn handle_meta_command(
    line: &str,
    conn: &Arc<Connection>,
    db_file: &str,
    w: &mut dyn Write,
) -> bool {
    let trimmed = line.trim();
    let (cmd, arg) = match trimmed.find(char::is_whitespace) {
        Some(pos) => (&trimmed[..pos], trimmed[pos..].trim()),
        None => (trimmed, ""),
    };
    match cmd {
        "\\q" => return true,
        "\\?" => cmd_help(w),
        "\\dt" => cmd_list_tables(conn, w),
        "\\d" => {
            if arg.is_empty() {
                cmd_list_tables(conn, w);
            } else {
                cmd_describe_table(conn, arg, w);
            }
        }
        "\\l" => cmd_list_databases(conn, w),
        "\\conninfo" => cmd_conninfo(db_file, w),
        _ => {
            let _ = writeln!(w, "Unknown command: {cmd}. Type \\? for help.");
        }
    }
    false
}

// ---------------------------------------------------------------------------
// SQL execution
// ---------------------------------------------------------------------------

fn execute_sql(
    conn: &Arc<Connection>,
    sql: &str,
    table_config: &TableConfig,
    w: &mut dyn Write,
) -> bool {
    let runner = conn.query_runner(sql.as_bytes());
    let mut had_error = false;
    for mut output in runner {
        match output {
            Ok(Some(ref mut stmt)) => {
                if let Err(e) = print_result_set(stmt, table_config, w) {
                    let _ = writeln!(w, "Error: {e}");
                    had_error = true;
                    break;
                }
            }
            Ok(None) => {}
            Err(ref e) => {
                let _ = writeln!(w, "Error: {e}");
                had_error = true;
                break;
            }
        }
    }
    had_error
}

fn print_result_set(
    stmt: &mut Statement,
    table_config: &TableConfig,
    w: &mut dyn Write,
) -> Result<(), LimboError> {
    let num_columns = stmt.num_columns();
    if num_columns == 0 {
        // DDL statements (CREATE TABLE, INSERT, etc.) have no result columns
        // but still need to be stepped to execute the bytecode.
        stmt.run_with_row_callback(|_| Ok(()))?;
        return Ok(());
    }

    let header_color = table_config.header_color.as_comfy_table_color();
    let column_colors: Vec<_> = table_config
        .column_colors
        .iter()
        .map(|c| c.as_comfy_table_color())
        .collect();

    let column_names: Vec<String> = (0..num_columns)
        .map(|i| stmt.get_column_name(i).to_string())
        .collect();

    let mut table = Table::new();
    table
        .set_content_arrangement(ContentArrangement::Dynamic)
        .apply_modifier("││──├─┼┤│─┼├┤┬┴┌┐└┘");

    let header: Vec<Cell> = column_names
        .iter()
        .map(|name| {
            Cell::new(name)
                .add_attribute(Attribute::Bold)
                .fg(header_color)
        })
        .collect();
    table.set_header(header);

    stmt.run_with_row_callback(|row| {
        let mut table_row = Row::new();
        table_row.max_height(1);
        for (idx, value) in row.get_values().enumerate() {
            let (content, alignment) = match value {
                Value::Null => ("NULL".to_string(), CellAlignment::Left),
                Value::Numeric(_) => (format!("{value}"), CellAlignment::Right),
                Value::Text(_) => (format!("{value}"), CellAlignment::Left),
                Value::Blob(_) => (format!("{value}"), CellAlignment::Left),
            };
            table_row.add_cell(
                Cell::new(content)
                    .set_alignment(alignment)
                    .fg(column_colors[idx % column_colors.len()]),
            );
        }
        table.add_row(table_row);
        Ok(())
    })?;

    if !table.is_empty() {
        writeln!(w, "{table}").map_err(|e| turso_core::io_error(e, "write"))?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// REPL
// ---------------------------------------------------------------------------

fn rustyline_config() -> rustyline::Config {
    rustyline::Config::builder()
        .completion_type(rustyline::CompletionType::List)
        .auto_add_history(true)
        .build()
}

struct Repl {
    conn: Arc<Connection>,
    #[allow(dead_code)]
    io: Arc<dyn turso_core::IO>,
    db_file: String,
    table_config: TableConfig,
    input_buf: String,
    read_state: ReadState,
    interrupt_count: Arc<AtomicUsize>,
    had_error: bool,
}

impl Repl {
    fn new(
        conn: Arc<Connection>,
        io: Arc<dyn turso_core::IO>,
        db_file: String,
        table_config: TableConfig,
        interrupt_count: Arc<AtomicUsize>,
    ) -> Self {
        Self {
            conn,
            io,
            db_file,
            table_config,
            input_buf: String::new(),
            read_state: ReadState::default(),
            interrupt_count,
            had_error: false,
        }
    }

    fn prompt(&self) -> &str {
        if self.input_buf.is_empty() {
            PROMPT
        } else {
            PROMPT_CONT
        }
    }

    fn reset_input(&mut self) {
        self.input_buf.clear();
        self.read_state = ReadState::default();
    }

    fn consume(&mut self, flush: bool) {
        if self.input_buf.trim().is_empty() {
            return;
        }

        let trimmed = self.input_buf.trim();

        // Backslash meta-commands
        if trimmed.starts_with('\\') {
            let input = self.input_buf.clone();
            let quit = handle_meta_command(
                input.trim(),
                &self.conn,
                &self.db_file,
                &mut std::io::stdout(),
            );
            self.reset_input();
            if quit {
                std::process::exit(0);
            }
            return;
        }

        let is_complete = self.read_state.is_complete();
        if is_complete || flush {
            let sql = self.input_buf.clone();
            let had_err = execute_sql(
                &self.conn,
                sql.trim(),
                &self.table_config,
                &mut std::io::stdout(),
            );
            if had_err {
                self.had_error = true;
            }
            self.reset_input();
        }
        // Otherwise keep accumulating (continuation line)
    }

    fn run_interactive(&mut self) {
        let mut rl = match Editor::<LimboHelper, DefaultHistory>::with_config(rustyline_config()) {
            Ok(rl) => rl,
            Err(e) => {
                eprintln!("Failed to initialize readline: {e}");
                return;
            }
        };

        if HISTORY_FILE.exists() {
            let _ = rl.load_history(HISTORY_FILE.as_path());
        }

        // Load config for syntax highlighting
        let config_file = CONFIG_DIR.join("limbo.toml");
        let config = config::Config::from_config_file(config_file);
        let h = LimboHelper::new(self.conn.clone(), Some(config.highlight.clone()));
        rl.set_helper(Some(h));

        println!("pgmicro v{}", env!("CARGO_PKG_VERSION"));
        println!("Type \\? for help, \\q to quit.");
        if self.db_file == ":memory:" {
            println!("Connected to a transient in-memory database.");
        }

        loop {
            let prompt = self.prompt().to_string();
            match rl.readline(&prompt) {
                Ok(line) => {
                    self.interrupt_count.store(0, Ordering::Release);
                    self.read_state.process(&line);
                    self.input_buf.push_str(&line);
                    if !self.input_buf.ends_with(char::is_whitespace) {
                        self.input_buf.push('\n');
                    }
                    self.consume(false);
                }
                Err(ReadlineError::Interrupted) => {
                    if self.interrupt_count.fetch_add(1, Ordering::SeqCst) >= 1 {
                        eprintln!("Interrupted. Exiting...");
                        break;
                    }
                    println!("Use \\q to exit or press Ctrl-C again to force quit.");
                    self.reset_input();
                }
                Err(ReadlineError::Eof) => {
                    self.consume(true);
                    break;
                }
                Err(e) => {
                    eprintln!("Error: {e}");
                    break;
                }
            }
        }

        let _ = rl.save_history(HISTORY_FILE.as_path());
    }

    fn run_stdin(&mut self) {
        let stdin = std::io::stdin();
        loop {
            let prev_len = self.input_buf.len();
            if std::io::BufRead::read_line(&mut stdin.lock(), &mut self.input_buf).unwrap_or(0) == 0
            {
                self.consume(true);
                break;
            }
            self.read_state.process(&self.input_buf[prev_len..]);
            self.consume(false);
        }
    }
}

// ---------------------------------------------------------------------------
// main
// ---------------------------------------------------------------------------

fn main() -> anyhow::Result<()> {
    let opts = Opts::parse();
    let _guard = init_tracing(&opts)?;

    let db_file = opts
        .database
        .as_ref()
        .map_or(":memory:".to_string(), |p| p.to_string_lossy().to_string());

    let (io, conn) = open_database(&db_file, opts.vfs.as_ref(), opts.readonly)?;

    let interrupt_count = Arc::new(AtomicUsize::new(0));
    {
        let ic = Arc::clone(&interrupt_count);
        ctrlc::set_handler(move || {
            ic.fetch_add(1, Ordering::Release);
        })
        .expect("Error setting Ctrl-C handler");
    }

    // Server mode: start PG wire protocol server and exit
    if let Some(ref address) = opts.server {
        auto_attach_pg_schemas(&conn, &db_file);
        let server = TursoPgServer::new(address.clone(), db_file, conn, interrupt_count);
        return server.run();
    }

    let table_config = TableConfig::adaptive_colors();

    // Execute a single SQL command and exit
    if let Some(ref sql) = opts.sql {
        let had_error = execute_sql(&conn, sql, &table_config, &mut std::io::stdout());
        conn.close()?;
        if had_error {
            std::process::exit(1);
        }
        return Ok(());
    }

    let mut repl = Repl::new(conn.clone(), io, db_file, table_config, interrupt_count);

    let interactive = IsTerminal::is_terminal(&std::io::stdin());
    if interactive {
        repl.run_interactive();
    } else {
        repl.run_stdin();
    }

    let _ = conn.close();
    if !interactive && repl.had_error {
        std::process::exit(1);
    }
    Ok(())
}
