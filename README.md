<p align="center">
  <img src="pgmicro-logo.png" alt="pgmicro" width="600"/>
</p>

# pgmicro

An in-process reimplementation of PostgreSQL, backed by a SQLite-compatible storage engine.

pgmicro is built as an experimental fork of [Turso](https://github.com/tursodatabase/turso) — a full from-scratch rewrite of SQLite in Rust — with PostgreSQL added as a native dialect. The result is a fast, embeddable, single-file database that speaks PostgreSQL.

## Why?

AI agents are driving an explosion of databases. Many of them are ephemeral, low-touch, short-lived, and small — a scratch database for a task, a session store that lives for minutes, a per-user sandbox.

SQLite has traditionally been king in these environments, and it's easy to see why: it's just a file (or even in-memory), no server to manage, no ports to configure. But many developers prefer PostgreSQL — whether out of familiarity, taste, or because PostgreSQL is legitimately more powerful in areas like its type system, JSON operators, and query capabilities.

Other approaches to bring PostgreSQL in-process try to compile PostgreSQL itself to WebAssembly. But PostgreSQL's architecture — particularly its process-per-connection model, shared memory assumptions, and reliance on a full OS environment — makes this fundamentally constrained. You end up fighting the architecture rather than benefiting from it.

pgmicro takes a different path entirely.

## How is pgmicro different?

pgmicro does not translate PostgreSQL to SQLite syntax. It does not embed or compile PostgreSQL. Instead, it **parses the PostgreSQL language and compiles it directly to SQLite bytecode**.

Here's how it works:

```
                        pgmicro architecture
                        ====================

  PostgreSQL SQL                          SQLite SQL
       │                                      │
       ▼                                      ▼
 ┌─────────────┐                       ┌───────────-──┐
 │ libpg_query │                       │ Turso Parser │
 │ (PG parser) │                       │ (SQLite)     │
 └─────┬───────┘                       └──────┬─────-─┘
       │ PG parse tree                        │ SQLite AST
       ▼                                      │
 ┌─────────────┐                              │
 │  Translator │──── Turso AST ──────────────►│
 │ (parser_pg) │                              │
 └─────────────┘                              ▼
                                    ┌──────────────────┐
                                    │  Turso Compiler  │
                                    │ (translate/*.rs) │
                                    └────────┬─────────┘
                                             │ VDBE bytecode
                                             ▼
                                    ┌──────────────────┐
                                    │  Bytecode Engine │
                                    │  (vdbe/*.rs)     │
                                    └────────┬─────────┘
                                             │
                                             ▼
                                    ┌────────────────────┐
                                    │  SQLite Storage    │
                                    │ (B-tree, WAL, etc) │
                                    └────────────────────┘
```

The key pieces:

- **PostgreSQL parser**: We use [`libpg_query`](https://github.com/pganalyze/libpg_query) (via the [`pg_query`](https://crates.io/crates/pg_query) Rust crate), which extracts PostgreSQL's *actual* parser from the PostgreSQL source code. This means pgmicro parses PostgreSQL syntax with 100% fidelity — it's the same parser PostgreSQL itself uses. We did not write a PostgreSQL parser.

- **Translator** (`parser_pg/`): A translation layer that converts the PostgreSQL parse tree into Turso's internal AST. This handles the mapping of PostgreSQL-specific syntax (e.g., `$$dollar quoting$$`, `::` casts, `SERIAL` types, PostgreSQL-style `CREATE TABLE`) into the representation that Turso's compiler understands.

- **Turso engine**: The full [Turso](https://github.com/tursodatabase/turso) database engine — a complete, from-scratch reimplementation of SQLite in Rust. It compiles the AST to bytecode and executes it against a SQLite-compatible B-tree storage format. Your data lives in a standard `.db` file.

- **PostgreSQL catalog**: Virtual tables (`pg_class`, `pg_attribute`, `pg_type`, `pg_namespace`, etc.) that expose schema metadata in the way PostgreSQL tools expect, enabling compatibility with `psql` and other PostgreSQL clients.

- **Dialect switching**: Turso supports dynamic dialect switching at the connection level. A single database can be accessed via both PostgreSQL and SQLite syntax — useful for tooling, migration, and interop.

## Installation

### CLI

Run directly with npx (no install needed):

```
npx pg-micro
```

Or install globally:

```
npm install -g pg-micro
pg-micro myapp.db
```

### JavaScript/TypeScript SDK

```
npm install pg-micro
```

```javascript
import { connect } from "pg-micro";

const db = await connect(":memory:");
await db.exec("CREATE TABLE users (id SERIAL PRIMARY KEY, name TEXT, email TEXT)");
await db.exec("INSERT INTO users (name, email) VALUES ('Alice', 'alice@example.com')");
await db.exec("INSERT INTO users (name, email) VALUES ('Bob', 'bob@example.com')");

const rows = await db.prepare("SELECT * FROM users").all();
console.log(rows);
// [
//   { id: 1, name: 'Alice', email: 'alice@example.com' },
//   { id: 2, name: 'Bob', email: 'bob@example.com' }
// ]

await db.close();
```

The SDK API is compatible with [`@tursodatabase/database`](https://www.npmjs.com/package/@tursodatabase/database).

## Examples

### In-memory database

```
$ pgmicro
pgmicro v0.6.0-pre.7
Type \? for help, \q to quit.
Connected to a transient in-memory database.

pgmicro> CREATE TABLE users (id SERIAL PRIMARY KEY, name TEXT, email TEXT);
pgmicro> INSERT INTO users (name, email) VALUES ('Alice', 'alice@example.com');
pgmicro> INSERT INTO users (name, email) VALUES ('Bob', 'bob@example.com');
pgmicro> SELECT * FROM users;
┌────┬───────┬───────────────────┐
│ id │ name  │ email             │
├────┼───────┼───────────────────┤
│  1 │ Alice │ alice@example.com │
├────┼───────┼───────────────────┤
│  2 │ Bob   │ bob@example.com   │
└────┴───────┴───────────────────┘

pgmicro> \dt
+-------+
| Table |
+=======+
| users |
+-------+

pgmicro> \d users
Table: users
+--------+------+----------+---------+
| Column | Type | Nullable | Default |
+========+======+==========+=========+
| id     | int4 | NULL     | YES     |
+--------+------+----------+---------+
| name   | text | NULL     |         |
+--------+------+----------+---------+
| email  | text | NULL     |         |
+--------+------+----------+---------+
```

### File-backed database

```
$ pgmicro myapp.db
pgmicro v0.6.0-pre.7
Type \? for help, \q to quit.

pgmicro> CREATE TABLE events (id INT, payload TEXT, ts TEXT DEFAULT CURRENT_TIMESTAMP);
pgmicro> INSERT INTO events (id, payload) VALUES (1, 'user.signup');
pgmicro> \q

$ file myapp.db
myapp.db: SQLite 3.x database

$ pgmicro myapp.db
pgmicro> SELECT * FROM events;
┌────┬─────────────┬─────────────────────┐
│ id │ payload     │ ts                  │
├────┼─────────────┼─────────────────────┤
│  1 │ user.signup │ 2025-03-26 12:00:00 │
└────┴─────────────┴─────────────────────┘
```

The underlying file is a standard SQLite database — you can inspect it with any SQLite tool.

### Server mode with psql

Although I expect most of the value of this to be derived from uses in-memory or in-file, pgmicro includes a PostgreSQL wire protocol server, so standard PostgreSQL clients can connect. It is a very simple server at this point, but very helpful to make sure that tools work:

```
$ pgmicro :memory: --server 127.0.0.1:5432
PostgreSQL server listening on 127.0.0.1:5432 (database: :memory:)
```

In another terminal:

```
$ psql -h 127.0.0.1 -p 5432 -U turso -d main
main=> CREATE TABLE users (id INT, name TEXT);
CREATE TABLE
main=> INSERT INTO users VALUES (1, 'Alice');
INSERT 0 1
main=> INSERT INTO users VALUES (2, 'Bob');
INSERT 0 1
main=> SELECT * FROM users;
 id | name
----+-------
  1 | Alice
  2 | Bob
(2 rows)

main=> \dt
         List of tables
 Schema | Name  | Type  | Owner
--------+-------+-------+-------
 public | users | table | turso
(1 row)
```

## Status

This is a heavily experimental project. I want to see how far I can take this. At this point, no guarantees are made about stability, compatibility, or completeness.

This project is not officially affiliated with Turso, although I am the founder of Turso.

## Contributing

Contributions are welcome. pgmicro is fully licensed under the MIT license (same as Turso).

Some key guidelines:

**Build on Turso, don't hack around it.** I believe this project has potential to achieve very good results, which means the right approach is often to add native support in Turso's core first — with efficient bytecode — and then have pgmicro wrap it. A good example is the type system: while it would be tempting to just map PostgreSQL types to SQLite types at translation time, we instead [added support for custom types in Turso](https://github.com/tursodatabase/turso/pull/5729) and implemented PostgreSQL types on top of that. The result is cleaner, faster, and more correct.

**Minimize changes to the Turso core.** Turso is under heavy development, so touching core code will lead to frequent merge conflicts. Some of it is unavoidable (like the pragmas to set the dialect), but significant code changes in the Turso core are a signal that perhaps this is a feature that should be proposed and pushed to Turso first.

**AI is encouraged, but do the work.** Prompting an LLM and sending the result without review, testing, or understanding will lead me to stop paying attention to your contributions. See my thoughts on this: [What happens with OSS in the age of AI](https://turso.tech/blog/what-happens-with-oss-in-the-age-of-ai).
