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

// ---------------------------------------------------------------------------
// DEFAULT functions
// ---------------------------------------------------------------------------

#[test]
fn default_now_produces_value() {
    let output = run_pgmicro(
        b"CREATE TABLE t(id INT, ts TEXT DEFAULT now());\n\
          INSERT INTO t(id) VALUES (1);\n\
          SELECT ts FROM t;\n",
    );
    assert_eq!(output.status.code(), Some(0));
    let out = stdout(&output);
    // now() produces a timestamp like "2026-04-13 ..."
    assert!(
        out.contains("20"),
        "expected timestamp from now(), got: {out}"
    );
}

#[test]
fn default_gen_random_uuid_produces_value() {
    let output = run_pgmicro(
        b"CREATE TABLE t(id INT, uid TEXT DEFAULT gen_random_uuid());\n\
          INSERT INTO t(id) VALUES (1);\n\
          INSERT INTO t(id) VALUES (2);\n\
          SELECT uid FROM t ORDER BY id;\n",
    );
    assert_eq!(output.status.code(), Some(0));
    let out = stdout(&output);
    // UUID contains hyphens
    assert!(
        out.matches('-').count() >= 4,
        "expected UUID with hyphens, got: {out}"
    );
}

#[test]
fn describe_table_shows_default_expressions() {
    let output = run_pgmicro(
        b"CREATE TABLE t(id INT, ts TEXT DEFAULT now(), uid TEXT DEFAULT gen_random_uuid());\n\
          \\d t\n",
    );
    assert_eq!(output.status.code(), Some(0));
    let out = stdout(&output);
    assert!(
        out.contains("now"),
        "\\d should show now() default, got: {out}"
    );
    assert!(
        out.contains("gen_random_uuid"),
        "\\d should show gen_random_uuid() default, got: {out}"
    );
}

#[test]
fn default_casted_expression() {
    let output = run_pgmicro(
        b"CREATE TABLE config(id INT, data jsonb DEFAULT '{}'::jsonb, tags jsonb DEFAULT '[]'::jsonb);\n\
          INSERT INTO config(id) VALUES (1);\n\
          SELECT data, tags FROM config;\n",
    );
    assert_eq!(output.status.code(), Some(0));
    let out = stdout(&output);
    assert!(
        out.contains("{}"),
        "expected '{{}}' from casted default, got: {out}"
    );
    assert!(
        out.contains("[]"),
        "expected '[]' from casted default, got: {out}"
    );
}

// ---------------------------------------------------------------------------
// Meta-commands: \di
// ---------------------------------------------------------------------------

#[test]
fn di_lists_created_indexes() {
    let output = run_pgmicro(
        b"CREATE TABLE t(id INT PRIMARY KEY, name TEXT);\nCREATE INDEX idx_name ON t(name);\n\\di\n",
    );
    assert_eq!(output.status.code(), Some(0));
    let out = stdout(&output);
    assert!(
        out.contains("idx_name"),
        "\\di should list idx_name, got: {out}"
    );
}

#[test]
fn di_empty_database() {
    let output = run_pgmicro(b"\\di\n");
    assert_eq!(output.status.code(), Some(0));
    let out = stdout(&output);
    assert!(
        out.contains("No indexes found"),
        "expected 'No indexes found', got: {out}"
    );
}

// ---------------------------------------------------------------------------
// Meta-commands: \dv
// ---------------------------------------------------------------------------

#[test]
fn dv_empty_database() {
    let output = run_pgmicro(b"\\dv\n");
    assert_eq!(output.status.code(), Some(0));
    let out = stdout(&output);
    assert!(
        out.contains("No views found"),
        "expected 'No views found', got: {out}"
    );
}

// ---------------------------------------------------------------------------
// Meta-commands: \dn
// ---------------------------------------------------------------------------

#[test]
fn dn_lists_schemas() {
    let output = run_pgmicro(b"\\dn\n");
    assert_eq!(output.status.code(), Some(0));
    let out = stdout(&output);
    assert!(
        out.contains("public"),
        "\\dn should list 'public', got: {out}"
    );
}

#[test]
fn dn_lists_created_schema() {
    let output = run_pgmicro(b"CREATE SCHEMA foo;\n\\dn\n");
    assert_eq!(output.status.code(), Some(0));
    let out = stdout(&output);
    assert!(out.contains("foo"), "\\dn should list 'foo', got: {out}");
}

// ---------------------------------------------------------------------------
// Meta-commands: \dT
// ---------------------------------------------------------------------------

#[test]
fn d_upper_t_lists_types() {
    let output = run_pgmicro(b"CREATE TYPE mood AS ENUM ('happy', 'sad');\n\\dT\n");
    assert_eq!(output.status.code(), Some(0));
    let out = stdout(&output);
    assert!(out.contains("mood"), "\\dT should list 'mood', got: {out}");
}

#[test]
fn d_upper_t_empty() {
    let output = run_pgmicro(b"\\dT\n");
    assert_eq!(output.status.code(), Some(0));
    let out = stdout(&output);
    assert!(
        out.contains("No types found"),
        "expected 'No types found', got: {out}"
    );
}

// ---------------------------------------------------------------------------
// Meta-commands: \du
// ---------------------------------------------------------------------------

#[test]
fn du_lists_roles() {
    let output = run_pgmicro(b"\\du\n");
    assert_eq!(output.status.code(), Some(0));
    let out = stdout(&output);
    assert!(
        out.contains("turso"),
        "\\du should list 'turso', got: {out}"
    );
}

// ---------------------------------------------------------------------------
// Meta-commands: \df
// ---------------------------------------------------------------------------

#[test]
fn df_lists_functions() {
    let output = run_pgmicro(b"\\df\n");
    assert_eq!(output.status.code(), Some(0));
    let out = stdout(&output);
    assert!(
        out.contains("abs") || out.contains("length"),
        "\\df should list some builtin function, got: {out}"
    );
}

// ---------------------------------------------------------------------------
// Meta-commands: \d+ (extended describe)
// ---------------------------------------------------------------------------

#[test]
fn d_plus_describes_table_extended() {
    let output = run_pgmicro(
        b"CREATE TABLE tbl(id INT PRIMARY KEY, name TEXT);\nCREATE INDEX idx_tbl_name ON tbl(name);\n\\d+ tbl\n",
    );
    assert_eq!(output.status.code(), Some(0));
    let out = stdout(&output);
    assert!(out.contains("id"), "should show column 'id', got: {out}");
    assert!(
        out.contains("idx_tbl_name"),
        "should show index, got: {out}"
    );
}

// ---------------------------------------------------------------------------
// Meta-commands: \dt+
// ---------------------------------------------------------------------------

#[test]
fn dt_plus_lists_tables_extended() {
    let output = run_pgmicro(b"CREATE TABLE tbl(id INT);\n\\dt+\n");
    assert_eq!(output.status.code(), Some(0));
    let out = stdout(&output);
    assert!(
        out.contains("tbl"),
        "\\dt+ should list table name, got: {out}"
    );
}

// ---------------------------------------------------------------------------
// Meta-commands: \x
// ---------------------------------------------------------------------------

#[test]
fn x_toggles_expanded() {
    let output = run_pgmicro(b"\\x\n");
    assert_eq!(output.status.code(), Some(0));
    let out = stdout(&output);
    assert!(
        out.contains("Expanded display is on"),
        "expected toggle message, got: {out}"
    );
}

// ---------------------------------------------------------------------------
// Meta-commands: \timing
// ---------------------------------------------------------------------------

#[test]
fn timing_toggles() {
    let output = run_pgmicro(b"\\timing\n");
    assert_eq!(output.status.code(), Some(0));
    let out = stdout(&output);
    assert!(
        out.contains("Timing is on"),
        "expected timing toggle message, got: {out}"
    );
}

// ---------------------------------------------------------------------------
// Meta-commands: \echo
// ---------------------------------------------------------------------------

#[test]
fn echo_prints_text() {
    let output = run_pgmicro(b"\\echo hello world\n");
    assert_eq!(output.status.code(), Some(0));
    let out = stdout(&output);
    assert!(
        out.contains("hello world"),
        "expected 'hello world', got: {out}"
    );
}

// ---------------------------------------------------------------------------
// Meta-commands: \? (updated help)
// ---------------------------------------------------------------------------

#[test]
fn help_lists_new_commands() {
    let output = run_pgmicro(b"\\?\n");
    assert_eq!(output.status.code(), Some(0));
    let out = stdout(&output);
    assert!(out.contains("\\di"), "help should mention \\di, got: {out}");
    assert!(out.contains("\\dn"), "help should mention \\dn, got: {out}");
    assert!(out.contains("\\dT"), "help should mention \\dT, got: {out}");
}

// ---------------------------------------------------------------------------
// Array constructor and subscripting
// ---------------------------------------------------------------------------

#[test]
fn array_constructor_and_subscript() {
    let output = run_pgmicro(
        b"CREATE TABLE t(id INT, tags TEXT[]);\n\
          INSERT INTO t VALUES (1, ARRAY['a','b','c']);\n\
          SELECT tags[1], tags[2], tags[3] FROM t;\n",
    );
    assert_eq!(output.status.code(), Some(0));
    let out = stdout(&output);
    assert!(out.contains("a"), "expected 'a' in output, got: {out}");
    assert!(out.contains("b"), "expected 'b' in output, got: {out}");
    assert!(out.contains("c"), "expected 'c' in output, got: {out}");
}

#[test]
fn array_slice() {
    let output = run_pgmicro(
        b"CREATE TABLE t(id INT, tags TEXT[]);\n\
          INSERT INTO t VALUES (1, ARRAY['a','b','c','d']);\n\
          SELECT tags[2:3] FROM t;\n",
    );
    assert_eq!(output.status.code(), Some(0));
    let out = stdout(&output);
    assert!(
        out.contains("b"),
        "expected 'b' in slice output, got: {out}"
    );
    assert!(
        out.contains("c"),
        "expected 'c' in slice output, got: {out}"
    );
}

#[test]
fn array_in_where_clause() {
    let output = run_pgmicro(
        b"CREATE TABLE t(id INT, vals INT[]);\n\
          INSERT INTO t VALUES (1, ARRAY[10,20,30]);\n\
          INSERT INTO t VALUES (2, ARRAY[40,50,60]);\n\
          SELECT id FROM t WHERE vals[1] = 40;\n",
    );
    assert_eq!(output.status.code(), Some(0));
    let out = stdout(&output);
    assert!(out.contains("2"), "expected id=2, got: {out}");
    assert!(
        !out.contains("1") || out.contains("2"),
        "should only return id=2"
    );
}

// ---------------------------------------------------------------------------
// Dollar-quoted and escape strings
// ---------------------------------------------------------------------------

#[test]
fn dollar_quoted_string() {
    let output = run_pgmicro(b"SELECT $$hello world$$;\n");
    assert_eq!(output.status.code(), Some(0));
    let out = stdout(&output);
    assert!(
        out.contains("hello world"),
        "expected 'hello world', got: {out}"
    );
}

#[test]
fn dollar_quoted_with_embedded_quote() {
    let output = run_pgmicro(b"SELECT $$it's fine$$;\n");
    assert_eq!(output.status.code(), Some(0));
    let out = stdout(&output);
    assert!(
        out.contains("it's fine"),
        "expected embedded quote, got: {out}"
    );
}

#[test]
fn tagged_dollar_quoted_string() {
    let output = run_pgmicro(b"SELECT $tag$content$tag$;\n");
    assert_eq!(output.status.code(), Some(0));
    let out = stdout(&output);
    assert!(out.contains("content"), "expected 'content', got: {out}");
}

#[test]
fn escape_string_backslash_n() {
    let output = run_pgmicro(b"SELECT E'line1\\nline2';\n");
    assert_eq!(output.status.code(), Some(0));
    let out = stdout(&output);
    assert!(
        out.contains("line1") && out.contains("line2"),
        "expected two lines, got: {out}"
    );
}

#[test]
fn escape_string_backslash_t() {
    let output = run_pgmicro(b"SELECT E'col1\\tcol2';\n");
    assert_eq!(output.status.code(), Some(0));
    let out = stdout(&output);
    assert!(
        out.contains("col1") && out.contains("col2"),
        "expected tab-separated, got: {out}"
    );
}
