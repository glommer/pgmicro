use std::io::Write;
use std::process::{Command, Output, Stdio};

fn run_pgmicro(input: &[u8]) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_pgmicro"))
        .arg(":memory:")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to run pgmicro");

    let mut stdin = child.stdin.take().expect("failed to take stdin");
    stdin.write_all(input).expect("failed to write stdin");
    drop(stdin);

    child.wait_with_output().expect("failed to wait for output")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).to_string()
}

// ---------------------------------------------------------------------------
// DDL execution
// ---------------------------------------------------------------------------

#[test]
fn create_table_then_select() {
    let output = run_pgmicro(
        b"CREATE TABLE kv(k TEXT, v INT);\nINSERT INTO kv VALUES ('hello', 42);\nSELECT * FROM kv;\n",
    );
    assert_eq!(output.status.code(), Some(0));
    let out = stdout(&output);
    assert!(out.contains("hello"), "expected 'hello' in: {out}");
    assert!(out.contains("42"), "expected '42' in: {out}");
}

#[test]
fn create_multiple_tables() {
    let output = run_pgmicro(
        b"CREATE TABLE a(x INT);\nCREATE TABLE b(y INT);\nCREATE TABLE c(z INT);\nSELECT tablename FROM pg_tables WHERE schemaname = 'public' ORDER BY tablename;\n",
    );
    assert_eq!(output.status.code(), Some(0));
    let out = stdout(&output);
    assert!(out.contains("a"), "expected table 'a' in: {out}");
    assert!(out.contains("b"), "expected table 'b' in: {out}");
    assert!(out.contains("c"), "expected table 'c' in: {out}");
}

// ---------------------------------------------------------------------------
// Meta-commands: \dt
// ---------------------------------------------------------------------------

#[test]
fn dt_lists_created_tables() {
    let output = run_pgmicro(b"CREATE TABLE foo(bar TEXT);\n\\dt\n");
    assert_eq!(output.status.code(), Some(0));
    let out = stdout(&output);
    assert!(out.contains("foo"), "\\dt should list 'foo', got: {out}");
}

#[test]
fn dt_lists_multiple_tables() {
    let output = run_pgmicro(b"CREATE TABLE alpha(x INT);\nCREATE TABLE beta(y TEXT);\n\\dt\n");
    assert_eq!(output.status.code(), Some(0));
    let out = stdout(&output);
    assert!(out.contains("alpha"), "\\dt should list alpha");
    assert!(out.contains("beta"), "\\dt should list beta");
}

#[test]
fn dt_empty_database() {
    let output = run_pgmicro(b"\\dt\n");
    assert_eq!(output.status.code(), Some(0));
    let out = stdout(&output);
    assert!(
        out.contains("No tables found"),
        "expected 'No tables found', got: {out}"
    );
}

// ---------------------------------------------------------------------------
// Meta-commands: \d <table>
// ---------------------------------------------------------------------------

#[test]
fn d_describes_table_columns() {
    let output =
        run_pgmicro(b"CREATE TABLE users(id INT PRIMARY KEY, name TEXT, age INT);\n\\d users\n");
    assert_eq!(output.status.code(), Some(0));
    let out = stdout(&output);
    assert!(out.contains("id"), "should show column 'id'");
    assert!(out.contains("name"), "should show column 'name'");
    assert!(out.contains("age"), "should show column 'age'");
    assert!(out.contains("text"), "should show type 'text'");
}

#[test]
fn d_nonexistent_table() {
    let output = run_pgmicro(b"\\d nonexistent\n");
    assert_eq!(output.status.code(), Some(0));
    let out = stdout(&output);
    assert!(
        out.contains("not found"),
        "should report not found, got: {out}"
    );
}

// ---------------------------------------------------------------------------
// Meta-commands: \l
// ---------------------------------------------------------------------------

#[test]
fn l_lists_database() {
    let output = run_pgmicro(b"\\l\n");
    assert_eq!(output.status.code(), Some(0));
    let out = stdout(&output);
    assert!(
        out.contains(":memory:"),
        "\\l should show :memory:, got: {out}"
    );
}

// ---------------------------------------------------------------------------
// Meta-commands: \conninfo
// ---------------------------------------------------------------------------

#[test]
fn conninfo_shows_database_and_dialect() {
    let output = run_pgmicro(b"\\conninfo\n");
    assert_eq!(output.status.code(), Some(0));
    let out = stdout(&output);
    assert!(out.contains(":memory:"), "should show database path");
    assert!(out.contains("PostgreSQL"), "should show dialect");
}

// ---------------------------------------------------------------------------
// Meta-commands: \?
// ---------------------------------------------------------------------------

#[test]
fn help_lists_commands() {
    let output = run_pgmicro(b"\\?\n");
    assert_eq!(output.status.code(), Some(0));
    let out = stdout(&output);
    assert!(out.contains("\\dt"), "help should mention \\dt");
    assert!(out.contains("\\d"), "help should mention \\d");
    assert!(out.contains("\\l"), "help should mention \\l");
    assert!(out.contains("\\q"), "help should mention \\q");
}

// ---------------------------------------------------------------------------
// Meta-commands: unknown
// ---------------------------------------------------------------------------

#[test]
fn unknown_command_reports_error() {
    let output = run_pgmicro(b"\\bogus\n");
    assert_eq!(output.status.code(), Some(0));
    let out = stdout(&output);
    assert!(
        out.contains("Unknown command"),
        "should report unknown command, got: {out}"
    );
}

// ---------------------------------------------------------------------------
// PG catalog access
// ---------------------------------------------------------------------------

#[test]
fn pg_class_shows_created_table() {
    let output = run_pgmicro(
        b"CREATE TABLE test_tbl(id INT, name TEXT);\nSELECT relname FROM pg_class WHERE relkind = 'r';\n",
    );
    assert_eq!(output.status.code(), Some(0));
    let out = stdout(&output);
    assert!(
        out.contains("test_tbl"),
        "pg_class should show test_tbl, got: {out}"
    );
}

// ---------------------------------------------------------------------------
// SQL dialect enforcement
// ---------------------------------------------------------------------------

#[test]
fn rejects_sqlite_syntax() {
    let output = run_pgmicro(b"SELECT * FROM sqlite_schema;\n");
    assert_ne!(
        output.status.code(),
        Some(0),
        "sqlite_schema should fail in PG mode"
    );
}

// ---------------------------------------------------------------------------
// Exit codes
// ---------------------------------------------------------------------------

#[test]
fn success_returns_zero() {
    let output = run_pgmicro(b"SELECT 1;\n");
    assert_eq!(output.status.code(), Some(0));
}

#[test]
fn error_returns_nonzero() {
    let output = run_pgmicro(b"SELECT * FROM nonexistent;\n");
    assert_eq!(output.status.code(), Some(1));
}

#[test]
fn empty_input_returns_zero() {
    let output = run_pgmicro(b"");
    assert_eq!(output.status.code(), Some(0));
}
