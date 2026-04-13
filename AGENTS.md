# pgmicro Agent Guidelines

In-process PostgreSQL reimplementation backed by Turso's SQLite-compatible engine.

## Quick Reference

```bash
cargo build                                      # build everything
cargo build -p pgmicro                           # build pgmicro binary only
cargo test -p pgmicro                            # pgmicro integration tests
cargo test -p turso_parser_pg                    # PG parser/translator tests
cargo test --test integration -- postgres        # PG integration tests (catalog, dialect, table)
cargo fmt                                        # format (required)
cargo clippy --workspace --all-features --all-targets -- --deny=warnings  # lint

# Run the pgmicro REPL
cargo run -p pgmicro -- :memory:

# Run with PG wire protocol server
cargo run -p pgmicro -- :memory: --server 127.0.0.1:5432

# Then connect with psql
psql -h 127.0.0.1 -p 5432 -U turso -d main
```

## Architecture

```
  PostgreSQL SQL                          SQLite SQL
       |                                      |
       v                                      v
  libpg_query (C FFI)                   Turso Parser
       | protobuf parse tree                  | Turso AST
       v                                      |
  PostgreSQLTranslator  ----> Turso AST ----->|
  (parser_pg/src/translator.rs)               v
                                     Turso Compiler (core/translate/)
                                              | VDBE bytecode
                                              v
                                     Bytecode Engine (core/vdbe/)
                                              |
                                              v
                                     SQLite Storage (B-tree, WAL)
```

Key: PG SQL is parsed by PostgreSQL's own parser (via libpg_query), then translated
**directly to Turso's AST** — never re-serialized as SQLite text. From the AST onward,
the entire Turso compilation and execution pipeline is shared.

## Structure

```
limbo/
├── parser_pg/              # PG parser + translator crate
│   ├── src/
│   │   ├── lib.rs          # Public API: parse(), split_statements()
│   │   └── translator.rs   # PG protobuf → Turso AST (5.5k LOC, the heart of pgmicro)
│   └── tests/              # Parse validity and error tests
├── pgmicro/                # Standalone binary crate
│   ├── src/main.rs         # REPL, meta-commands (\dt, \d, \l), --server flag
│   ├── build.rs            # Syntax highlighting compilation
│   └── tests/pgmicro.rs    # Binary integration tests (stdin/stdout)
├── core/
│   ├── pg_catalog.rs       # PG catalog virtual tables (3.6k LOC)
│   ├── functions/postgres.rs # PG system functions (pg_get_userbyid, format_type, etc.)
│   ├── connection.rs       # Dialect dispatch: parse_postgresql_sql(), try_prepare_pg()
│   └── lib.rs              # SqlDialect enum, DatabaseOpts::enable_postgres
├── cli/pg_server.rs        # PG wire protocol server (pgwire crate, 800 LOC)
├── npm/pgmicro/            # npm package (pg-micro)
├── tests/integration/postgres/
│   ├── catalog.rs          # PG catalog virtual table tests
│   ├── dialect.rs          # Dialect switching + PG query tests
│   └── table.rs            # Data read-through tests
└── bindings/javascript/    # NAPI bindings (shared with tursodb, PG-aware)
```

## Where to Look

| Task | Location | Notes |
|------|----------|-------|
| Translate new PG syntax | `parser_pg/src/translator.rs` | Map pg_query protobuf nodes to `turso_parser::ast` |
| Add PG catalog table | `core/pg_catalog.rs` | Implement `InternalVirtualTable` trait |
| Add PG system function | `core/functions/postgres.rs` | Register in `core/function.rs` |
| Wire protocol issues | `cli/pg_server.rs` | `pgwire` crate, type encoding/decoding |
| Dialect switching | `core/connection.rs` | `parse_postgresql_sql()`, `try_prepare_pg()` |
| Meta-commands (\dt, \d) | `pgmicro/src/main.rs` | Inline in the REPL loop |
| Schema support | `core/connection.rs` | CREATE/DROP SCHEMA → ATTACH/DETACH |
| PG type mapping | `core/pg_catalog.rs` | `sqlite_type_to_pg_oid()`, type OID tables |
| Wire type encoding | `cli/pg_server.rs` | `pg_bytes_to_value()`, `encode_value()` |
| Test PG features | `tests/integration/postgres/` | Rust API tests (dialect, catalog, table) |
| Test pgmicro binary | `pgmicro/tests/pgmicro.rs` | Spawns binary, pipes stdin |

## The Translator: How PG SQL Becomes Bytecode

The translator (`parser_pg/src/translator.rs`) is the most important file. It converts
`pg_query::ParseResult` (protobuf from PostgreSQL's actual C parser) into `turso_parser::ast::Stmt`.

### Flow

1. `turso_parser_pg::parse(sql)` → calls `pg_query::parse()` (libpg_query C FFI) → protobuf tree
2. `PostgreSQLTranslator::translate(&parse_result)` → dispatches on node type:
   - `SelectStmt` → `translate_select()` (CTEs, UNION, ORDER BY, LIMIT)
   - `InsertStmt` → `translate_insert()` (RETURNING, ON CONFLICT)
   - `CreateStmt` → `translate_create_table()` (column defs, constraints, SERIAL)
   - `UpdateStmt`, `DeleteStmt`, `DropStmt`, `AlterTableStmt`, etc.
3. `translate_expr()` is the central expression dispatcher — handles:
   - `ColumnRef` → `ast::Expr::Id` / `ast::Expr::Qualified`
   - `A_Const` → `ast::Expr::Literal`
   - `A_Expr` → operators (comparison, arithmetic, LIKE, BETWEEN, IN, IS NULL, JSON `->`)
   - `TypeCast` (`::` syntax) → `ast::Expr::Cast`
   - `FuncCall` → `ast::Expr::FunctionCall` (with aggregate FILTER/DISTINCT/window)
   - `SubLink` → subqueries (EXISTS, IN, scalar)
   - `BoolExpr` → AND/OR/NOT
   - `CaseExpr`, `CoalesceExpr`, `NullTest`, `BooleanTest`, etc.
4. Result: a `turso_parser::ast::Stmt` — identical type to what the SQLite parser produces
5. Normal `core/translate/` pipeline compiles AST → VDBE bytecode

### Table Name Mapping

`map_table_name()` in the translator handles PG system table routing:
- `pg_class`, `pg_namespace`, `pg_attribute`, etc. → pass through (resolved as virtual tables)
- `information_schema.tables` → `sqlite_master`
- Schema-qualified names (`myschema.mytable`) → resolved via ATTACH'd databases

### Special Cases in `try_prepare_pg()`

Before the full parse path, `connection.rs::try_prepare_pg()` intercepts:
- `SET name = value` → rewritten to `PRAGMA name = value`
- `SHOW name` → rewritten to `PRAGMA name`
- `CREATE SCHEMA name` → `ATTACH 'turso-postgres-schema-<name>.db' AS "<name>"`
- `DROP SCHEMA name` → detach + drop tables

## PG Catalog Virtual Tables

Implemented in `core/pg_catalog.rs` as `InternalVirtualTable` impls, registered in
`Schema::postgres_catalog_tables`. Each reads from `Schema` at scan time.

### Populated tables

| Table | Source |
|-------|--------|
| `pg_class` | User tables from Schema, OIDs starting at 16384 |
| `pg_namespace` | `pg_catalog` (oid=11), `public` (oid=2200), + ATTACH'd schemas |
| `pg_attribute` | Column metadata per table, with PG type OIDs |
| `pg_type` | ~30 standard PG types with OIDs |
| `pg_tables` | User tables (schemaname='public') |
| `pg_index` | Index metadata from Schema |
| `pg_constraint` | PK, UNIQUE, CHECK, FK constraints |
| `pg_attrdef` | Column DEFAULT expressions |
| `pg_roles` | Single row: user `turso` |
| `pg_proc` | Built-in scalar functions |
| `pg_database` | Main db + ATTACH'd schemas |
| `pg_am` | Access methods: heap + btree |
| `pg_get_tabledef` | Table-valued function: returns CREATE TABLE DDL |

### Stub tables (empty, exist for psql compatibility)

`pg_policy`, `pg_trigger`, `pg_statistic_ext`, `pg_inherits`, `pg_rewrite`,
`pg_foreign_table`, `pg_partitioned_table`, `pg_collation`, `pg_description`,
`pg_publication`, `pg_publication_namespace`, `pg_publication_rel`

## PG Wire Protocol

`cli/pg_server.rs` implements both Simple and Extended query protocols via the `pgwire` crate.

- **Simple protocol**: `psql` default. Splits multi-statement input, executes sequentially.
- **Extended protocol**: Prepared statements with `$1`/`$2` parameters. `bind_portal_parameters()` maps portal params to bytecode register indices.
- **Type encoding**: `sqlite_type_to_pg_type()` for column metadata, `pg_bytes_to_value()` for parameter decoding, `encode_value()` for result encoding. Special handling for BOOL, TIMESTAMPTZ, and array types.

## Dialect Mechanism

- `SqlDialect` enum: `Sqlite` (0) or `Postgres` (1), stored per-connection as `AtomicSqlDialect`
- `default-postgres` Cargo feature on `turso_core`: makes `SqlDialect::default()` return `Postgres`
- pgmicro enables this feature; tursodb does not — same engine, different default
- `prepare_internal()` temporarily forces SQLite dialect for internal schema queries
- `PRAGMA sql_dialect = 'postgres'` / `'sqlite'` toggles at runtime

## pgmicro Core Principles

1. **Build on Turso, don't hack around it.** If a PG feature would be better served by native Turso support (types, arrays, expressions), propose and implement it in Turso core first, then wrap it in pgmicro. The type system is a good example: we added custom types to Turso rather than faking them in the translator.

2. **Production quality, not a demo.** This is intended for real use. Don't cut corners on type fidelity, error messages, or edge cases. If PostgreSQL does it one way, we match that behavior or explicitly document the gap.

3. **Minimize core/ changes.** Turso is under active development. Every line changed in `core/` is a future merge conflict. If you're adding significant code to core, that's a signal the feature should be proposed upstream to Turso first.

4. **Translator correctness over coverage.** It's better to reject unsupported syntax with a clear error than to silently produce wrong results. When adding new syntax support in the translator, test edge cases against real PostgreSQL behavior.

5. **Two-plan rule for PG features.** When a feature requires core changes: (a) write a Turso-core plan with no mention of postgres — it must be self-justifying as a Turso feature, (b) write a separate pgmicro plan that builds on top. This keeps the layers clean and makes upstream contribution possible.

6. **Test with the REPL first, psql second.** Primary testing uses `cargo run -p pgmicro` or the Rust integration tests. Wire protocol testing via psql is verification, not the primary test path.

## Dead Code Note

`parser_pg/src/` contains `ast.rs` (1.1k LOC), `lexer.rs` (848 LOC), `parser.rs` (2.9k LOC),
and `token.rs` (515 LOC). These are a hand-written PG parser that is **not used in the execution
path**. The actual path uses `pg_query` (libpg_query C FFI) → protobuf → `translator.rs` → Turso AST.
These files may be removed or repurposed in the future. Do not build on them.

## Common Workflows

### Adding support for a new PG statement type

1. Check if `pg_query` parses it (it almost certainly does — it's PostgreSQL's actual parser)
2. Add a new `translate_*` method in `parser_pg/src/translator.rs`
3. Map it to the appropriate `turso_parser::ast::Stmt` variant
4. If the AST variant doesn't exist, check if Turso core needs the feature first (two-plan rule)
5. Add tests in `parser_pg/tests/` and `tests/integration/postgres/`

### Adding support for a new PG expression/operator

1. Find the protobuf node type in the `pg_query` crate docs
2. Add handling in `translate_expr()` or `translate_a_expr()` in `translator.rs`
3. Map to the appropriate `turso_parser::ast::Expr` variant
4. Test with `cargo test -p turso_parser_pg` and integration tests

### Adding a new PG catalog table

1. Implement `InternalVirtualTable` in `core/pg_catalog.rs`
2. Add to the `pg_catalog_virtual_tables()` function
3. Add tests in `tests/integration/postgres/catalog.rs`
4. If `psql` queries it via `\d` or similar, test with psql too

### Adding a PG system function

1. Implement in `core/functions/postgres.rs`
2. Register in `core/function.rs` scalar function list
3. The translator maps PG function names in `translate_func_call()` — update if the PG name differs from the registered name

### Debugging PG translation issues

1. Parse the SQL with `pg_query` to see the protobuf tree:
   ```rust
   let result = turso_parser_pg::parse("YOUR SQL HERE").unwrap();
   println!("{:#?}", result);
   ```
2. Check what AST the translator produces — add a `dbg!(&stmt)` after `translate()`
3. Compare the Turso AST against what the SQLite parser produces for equivalent SQL
4. Use `EXPLAIN` in both pgmicro and tursodb to compare bytecode

## pgmicro CI

pgmicro has its own CI workflows:
- `.github/workflows/pgmicro-ci.yml` — build + test
- `.github/workflows/pgmicro-napi.yml` — NAPI binary builds for npm

---

# Turso Engine Guidelines

pgmicro is built on top of the Turso engine. The guidelines below are equally applicable
when working on `core/`, `parser/`, `testing/`, or any shared infrastructure.

## Quick Reference

```bash
cargo build                    # build. never build with release.
cargo test                     # rust unit/integration tests
cargo fmt                      # format (required)
cargo clippy --workspace --all-features --all-targets -- --deny=warnings  # lint
cargo run -q --bin tursodb -- -q # run the interactive cli

make test                      # TCL compat + sqlite3 + extensions + MVCC
make test-single TEST=foo.test # single TCL test
CI=1 make -C testing/sqltests run-rust  # sqltest runner (preferred for new tests)

scripts/diff.sh "SQL" [label]  # compare sqlite3 vs tursodb output
```

## Structure

```
limbo/
├── core/           # Database engine (translate/, storage/, vdbe/, io/, mvcc/)
├── parser/         # SQL parser (lexer, AST, grammar)
├── cli/            # tursodb CLI (REPL, MCP server, sync server)
├── bindings/       # Python, JS, Java, .NET, Go, Rust
├── extensions/     # crypto, regexp, csv, fuzzy, ipaddr, percentile
├── testing/        # simulator/, concurrent-simulator/, differential-oracle/
├── sync/           # engine/, sdk-kit/ (Turso Cloud sync)
├── sdk-kit/        # High-level SDK abstraction
└── tools/          # dbhash utility
```

## Where to Look

| Task | Location | Notes |
|------|----------|-------|
| Query execution | `core/vdbe/execute.rs` | 12k LOC bytecode interpreter |
| SQL compilation | `core/translate/` | AST → bytecode, optimizer in `optimizer/` |
| B-tree/pages | `core/storage/btree.rs` | 10k LOC, SQLite-compatible format |
| WAL/durability | `core/storage/wal.rs` | Write-ahead log, checkpointing |
| SQL parsing | `parser/src/parser.rs` | 11k LOC recursive descent |
| Add extension | `extensions/core/` | ExtensionApi, scalar/aggregate/vtab traits |
| Add binding | `bindings/` | PyO3, NAPI, JNI, FRB, CGO patterns |
| Deterministic tests | `testing/simulator/` | Fault injection, differential testing |
| New SQL tests | `testing/sqltests/tests/` | `.sqltest` format preferred |
| Quick sqlite3 diff | `scripts/diff.sh` | Compare sqlite3 vs tursodb output for a query |
| MVCC testing REPL | `cli/mvcc_repl.rs` | Multi-conn concurrent txn testing REPL        |

## Guides

- **[Testing](docs/agent-guides/testing.md)** - test types, when to use, how to write
- **[Code Quality](docs/agent-guides/code-quality.md)** - correctness rules, Rust patterns, comments
- **[Debugging](docs/agent-guides/debugging.md)** - bytecode comparison, logging, sanitizers
- **[PR Workflow](docs/agent-guides/pr-workflow.md)** - commits, CI, dependencies
- **[Transaction Correctness](docs/agent-guides/transaction-correctness.md)** - WAL, checkpointing, concurrency
- **[Storage Format](docs/agent-guides/storage-format.md)** - file format, B-trees, pages
- **[Async I/O Model](docs/agent-guides/async-io-model.md)** - IOResult, state machines, re-entrancy
- **[MVCC](docs/agent-guides/mvcc.md)** - experimental multi-version concurrency (WIP)

## Core Principles

1. **Correctness paramount.** Production DB, not a toy. Crash > corrupt
2. **SQLite compatibility.** Compare bytecode with `EXPLAIN`
3. **Every change needs a test.** Must fail without change, pass with it
4. **Assert invariants.** Don't silently fail. Don't hedge with if-statements
5. **Own your regressions.** If tests fail after your change, they are your regressions. Debug them directly. Never stash/revert to "check if they fail on main" — that wastes time and is categorically banned.
6. **Validate your hypotheses.**: If you suspect a given cause for a bug, validate it and provide incontrovertible evidence. NEVER make unearned assumptions.

## CI Note

Running in GitHub Action? Max-turns limit in `.github/workflows/claude.yml`. OK to push WIP and continue in another action. Stay focused, avoid rabbit holes.
