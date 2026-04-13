import { expect, test } from "vitest";
import { connect } from "./promise.js";

test("in-memory db", async () => {
  const db = await connect(":memory:");
  await db.exec("CREATE TABLE t(x INT)");
  await db.exec("INSERT INTO t VALUES (1), (2), (3)");
  const rows = await db.prepare("SELECT * FROM t WHERE x % 2 = $1").all([1]);
  expect(rows).toEqual([{ x: 1 }, { x: 3 }]);
});

test("pg type cast", async () => {
  const db = await connect(":memory:");
  const rows = await db.prepare("SELECT 1::int AS val").all();
  expect(rows).toEqual([{ val: 1 }]);
});

test("pg create table with SERIAL", async () => {
  const db = await connect(":memory:");
  await db.exec(
    "CREATE TABLE users (id SERIAL PRIMARY KEY, name TEXT NOT NULL)"
  );
  await db.exec("INSERT INTO users (name) VALUES ('Alice')");
  await db.exec("INSERT INTO users (name) VALUES ('Bob')");
  const rows = await db.prepare("SELECT * FROM users").all();
  expect(rows).toEqual([
    { id: 1, name: "Alice" },
    { id: 2, name: "Bob" },
  ]);
});

test("insert and select round-trip", async () => {
  const db = await connect(":memory:");
  await db.exec("CREATE TABLE items (id SERIAL PRIMARY KEY, label TEXT)");
  const insert = db.prepare("INSERT INTO items (label) VALUES ($1)");
  await insert.run("foo");
  await insert.run("bar");
  const rows = await db.prepare("SELECT id, label FROM items").all();
  expect(rows).toEqual([
    { id: 1, label: "foo" },
    { id: 2, label: "bar" },
  ]);
});

test("pg_catalog tables", async () => {
  const db = await connect(":memory:");
  await db.exec("CREATE TABLE widgets (id SERIAL PRIMARY KEY, name TEXT)");
  const rows = await db
    .prepare("SELECT tablename FROM pg_tables WHERE schemaname = 'public'")
    .all();
  expect(rows).toEqual([{ tablename: "widgets" }]);
});

test("parameters", async () => {
  const db = await connect(":memory:");
  await db.exec("CREATE TABLE kv (k TEXT, v INTEGER)");
  await db.prepare("INSERT INTO kv VALUES ($1, $2)").run("a", 10);
  await db.prepare("INSERT INTO kv VALUES ($1, $2)").run("b", 20);
  const rows = await db.prepare("SELECT * FROM kv WHERE v > $1").all([5]);
  expect(rows).toEqual([
    { k: "a", v: 10 },
    { k: "b", v: 20 },
  ]);
});

test("exec multiple statements", async () => {
  const db = await connect(":memory:");
  await db.exec("CREATE TABLE t(x INT)");
  await db.exec("INSERT INTO t VALUES (1)");
  await db.exec("INSERT INTO t VALUES (2)");
  const rows = await db.prepare("SELECT * FROM t").all();
  expect(rows).toEqual([{ x: 1 }, { x: 2 }]);
});

test("transactions", async () => {
  const db = await connect(":memory:");
  await db.exec("CREATE TABLE accounts (name TEXT, balance INTEGER)");

  const transaction = db.transaction(async (entries: { name: string; balance: number }[]) => {
    const insert = db.prepare(
      "INSERT INTO accounts (name, balance) VALUES ($1, $2)"
    );
    for (const e of entries) {
      await insert.run(e.name, e.balance);
    }
  });

  await transaction([
    { name: "Alice", balance: 100 },
    { name: "Bob", balance: 200 },
  ]);

  const rows = await db.prepare("SELECT * FROM accounts").all();
  expect(rows).toEqual([
    { name: "Alice", balance: 100 },
    { name: "Bob", balance: 200 },
  ]);
});
