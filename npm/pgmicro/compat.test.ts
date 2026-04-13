import { expect, test } from "vitest";
import { Database } from "./compat.js";

test("in-memory db", () => {
  const db = new Database(":memory:");
  db.exec("CREATE TABLE t(x INT)");
  db.exec("INSERT INTO t VALUES (1), (2), (3)");
  const rows = db.prepare("SELECT * FROM t WHERE x % 2 = $1").all([1]);
  expect(rows).toEqual([{ x: 1 }, { x: 3 }]);
});

test("pg type cast", () => {
  const db = new Database(":memory:");
  const rows = db.prepare("SELECT 1::int AS val").all();
  expect(rows).toEqual([{ val: 1 }]);
});

test("pg create table and insert", () => {
  const db = new Database(":memory:");
  db.exec("CREATE TABLE users (id SERIAL PRIMARY KEY, name TEXT NOT NULL)");
  db.exec("INSERT INTO users (name) VALUES ('Alice')");
  db.exec("INSERT INTO users (name) VALUES ('Bob')");
  const rows = db.prepare("SELECT * FROM users").all();
  expect(rows).toEqual([
    { id: 1, name: "Alice" },
    { id: 2, name: "Bob" },
  ]);
});

test("pg_catalog tables", () => {
  const db = new Database(":memory:");
  db.exec("CREATE TABLE widgets (id SERIAL PRIMARY KEY, name TEXT)");
  const rows = db
    .prepare("SELECT tablename FROM pg_tables WHERE schemaname = 'public'")
    .all();
  expect(rows).toEqual([{ tablename: "widgets" }]);
});
