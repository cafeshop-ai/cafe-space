// SQL regression only: this does not execute Rust, the migration runner or the Host.
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { DatabaseSync } from 'node:sqlite';
import test from 'node:test';

const root = new URL('../', import.meta.url);
const source = readFileSync(new URL('crates/cafe-spaced/src/session_activity.rs', root), 'utf8');
const sql = name => {
  const match = source.match(new RegExp(`const ${name}: &str = "([^"]+)";`));
  assert.ok(match, `missing source SQL ${name}`);
  return match[1];
};
const metadataSql = sql('SAVE_METADATA');
const receiptSql = sql('SAVE_RECEIPT');
const migration = version => readFileSync(new URL(`fixtures/migrations/v${version}.sql`, root), 'utf8');
const digest = text => createHash('sha256').update(text).digest();
const bindings = values => Object.fromEntries(values.map((value, index) => [`?${index + 1}`, value]));
function database(t, path = ':memory:', version = 6) {
  const db = new DatabaseSync(path);
  t.after(() => db.close());
  db.exec('PRAGMA foreign_keys=ON');
  for (const v of [2, 5, 6].filter(v => v <= version)) db.exec(migration(v));
  db.exec("INSERT INTO sessions VALUES ('one','{}','{}'),('resumed','{}','{}')");
  return db;
}
const receipt = (db, values) => db.prepare(receiptSql).run(bindings(values)).changes;
const metadata = (db, values) => db.prepare(metadataSql).run(bindings(values)).changes;

test('schema six leaves legacy rows unknown and DDL failure is atomic', t => {
  const db = database(t, ':memory:', 5);
  const snapshot = JSON.parse(readFileSync(new URL('fixtures/migrations/v3-session.json', root), 'utf8'));
  snapshot.id = 'one';
  snapshot.capabilities.toolApproval = 'unknown';
  assert.equal(Object.hasOwn(snapshot, 'lastActivityAt'), false);
  assert.equal(Object.hasOwn(snapshot, 'preview'), false);
  db.prepare("UPDATE sessions SET snapshot=? WHERE id='one'").run(JSON.stringify(snapshot));
  assert.equal(db.prepare("SELECT count(*) AS n FROM sqlite_schema WHERE name IN ('session_activity','session_message_receipts')").get().n, 0);
  const before = db.prepare('SELECT * FROM sessions').all();
  db.exec('BEGIN');
  db.exec(migration(6));
  assert.deepEqual(db.prepare('SELECT * FROM sessions').all(), before);
  assert.equal(db.prepare('SELECT count(*) AS n FROM session_activity').get().n, 0);
  db.exec('ROLLBACK');
  assert.equal(db.prepare('PRAGMA user_version').get().user_version, 5);
  assert.equal(db.prepare("SELECT count(*) AS n FROM sqlite_schema WHERE name IN ('session_activity','session_message_receipts')").get().n, 0);
  db.exec("CREATE TABLE session_message_receipts(sentinel TEXT); INSERT INTO session_message_receipts VALUES ('keep')");
  db.exec('BEGIN');
  assert.throws(() => db.exec(migration(6)));
  db.exec('ROLLBACK');
  assert.equal(db.prepare("SELECT count(*) AS n FROM sqlite_schema WHERE name='session_activity'").get().n, 0);
  assert.equal(db.prepare('SELECT sentinel FROM session_message_receipts').get().sentinel, 'keep');
});

test('source receipt SQL suppresses identical replay after display eviction', t => {
  const db = database(t);
  const values = ['one', 'message-1', digest('display'), 1, digest('preview')];
  assert.equal(receipt(db, values), 1);
  db.exec("INSERT INTO transcript_messages VALUES ('one','message-1','display'); DELETE FROM transcript_messages");
  assert.equal(receipt(db, values), 0);
  assert.equal(receipt(db, ['one', 'message-1', digest('edited'), 1, digest('preview')]), 1);
  assert.equal(receipt(db, ['one', 'message-1', digest('edited'), 1, digest('clear')]), 1);
  const before = db.prepare('SELECT * FROM session_message_receipts').all();
  assert.equal(receipt(db, ['one', 'message-1', digest('edited'), null, null]), 0);
  assert.deepEqual(db.prepare('SELECT * FROM session_message_receipts').all(), before);
});

test('history receipts fill unknown display or provenance without losing known fields', t => {
  const db = database(t);
  assert.equal(receipt(db, ['one', 'evicted', null, 99, digest('history')]), 1);
  assert.equal(receipt(db, ['one', 'evicted', digest('display'), null, null]), 1);
  const before = db.prepare('SELECT * FROM session_message_receipts').all();
  assert.equal(receipt(db, ['one', 'evicted', null, 99, digest('history')]), 0);
  assert.deepEqual(db.prepare('SELECT * FROM session_message_receipts').all(), before);
  assert.equal(receipt(db, ['resumed', 'new-id', digest('new'), 99, digest('new-preview')]), 1);
  assert.throws(() => receipt(db, ['one', 'conflicting-id', digest('bad'), 99, digest('bad')]));
  assert.equal(receipt(db, ['one', 'display-only', digest('history'), null, null]), 1);
  assert.equal(receipt(db, ['one', 'display-only', null, 1, digest('learned')]), 1);
  assert.deepEqual(db.prepare("SELECT display_digest FROM session_message_receipts WHERE message_id='display-only'").get().display_digest, new Uint8Array(digest('history')));
});

test('metadata failure rolls back display and receipt inserts and updates', t => {
  const tables = ['transcript_messages', 'session_message_receipts', 'session_activity'];
  for (const existing of [false, true]) {
    const db = database(t);
    if (existing) {
      db.exec("INSERT INTO transcript_messages VALUES ('one','message-1','before')");
      receipt(db, ['one', 'message-1', digest('before'), 1, digest('before')]);
      metadata(db, ['one', 100, 'before', 'message-1', 1]);
    }
    const before = tables.map(table => db.prepare(`SELECT * FROM ${table}`).all());
    db.exec("CREATE TRIGGER fail_activity BEFORE INSERT ON session_activity BEGIN SELECT RAISE(ABORT,'synthetic failure'); END");
    db.exec('BEGIN');
    db.exec("INSERT INTO transcript_messages VALUES ('one','message-1','pending') ON CONFLICT(session_id,id) DO UPDATE SET payload=excluded.payload");
    receipt(db, ['one', 'message-1', digest('pending'), 1, digest('pending')]);
    assert.throws(() => metadata(db, ['one', 200, 'pending', 'message-1', 1]), /synthetic failure/);
    db.exec('ROLLBACK');
    assert.deepEqual(tables.map(table => db.prepare(`SELECT * FROM ${table}`).all()), before);
  }
});

test('session write savepoints preserve atomicity with and without an outer transaction', t => {
  const sessions = readFileSync(new URL('crates/cafe-spaced/src/sessions.rs', root), 'utf8');
  const helper = sessions.slice(sessions.indexOf('fn write_with_activity('), sessions.indexOf('\nfn save('));
  const statements = [...helper.matchAll(/execute_batch\("([^"]+)"\)/g)].map(match => match[1]);
  assert.equal(statements.length, 3);
  const [begin, commit, rollback] = statements;
  for (const nested of [false, true]) {
    for (const creation of [false, true]) {
      const db = database(t);
      metadata(db, ['one', 100, null, null, null]);
      const before = db.prepare('SELECT * FROM sessions').all();
      db.exec("CREATE TRIGGER fail_activity BEFORE INSERT ON session_activity BEGIN SELECT RAISE(ABORT,'synthetic failure'); END");
      if (nested) db.exec('BEGIN');
      db.exec(begin);
      if (creation) db.exec("INSERT INTO sessions VALUES ('new','{}','{}')");
      else db.exec("UPDATE sessions SET snapshot='changed' WHERE id='one'");
      assert.throws(() => metadata(db, [creation ? 'new' : 'one', 200, null, null, null]));
      db.exec(rollback);
      if (nested) db.exec('COMMIT');
      assert.deepEqual(db.prepare('SELECT * FROM sessions').all(), before);
      assert.equal(db.prepare("SELECT last_activity_at FROM session_activity WHERE session_id='one'").get().last_activity_at, 100);
      db.exec('DROP TRIGGER fail_activity');
      if (nested) db.exec('BEGIN');
      db.exec(begin);
      db.exec("INSERT INTO sessions VALUES ('new','{}','{}')");
      metadata(db, ['new', 200, null, null, null]);
      db.exec(commit);
      if (nested) db.exec('ROLLBACK');
      assert.equal(db.prepare("SELECT count(*) AS n FROM sessions WHERE id='new'").get().n, nested ? 0 : 1);
      assert.equal(db.prepare("SELECT count(*) AS n FROM session_activity WHERE session_id='new'").get().n, nested ? 0 : 1);
    }
  }
});

test('SQL bounds preserve exact 64-bit order and clear provenance', t => {
  const db = database(t);
  const maxOrder = 9223372036854775807n;
  metadata(db, ['one', 8640000000000000n, 'tea', 'latest', maxOrder]);
  metadata(db, ['one', 8640000000000000n, null, 'latest', maxOrder]);
  const query = db.prepare('SELECT * FROM session_activity');
  query.setReadBigInts(true);
  const row = query.get();
  assert.equal(row.last_activity_at, 8640000000000000n);
  assert.equal(row.last_user_message_order, maxOrder);
  assert.equal(row.last_user_message_id, 'latest');
  assert.equal(row.preview, null);
  for (const order of ['0', '-1', '1.5', '9223372036854775808']) {
    assert.throws(() => db.exec(`UPDATE session_activity SET last_user_message_order=${order}`));
    assert.throws(() => db.exec(`INSERT INTO session_message_receipts VALUES ('one','invalid',zeroblob(32),${order},zeroblob(32))`));
  }
  for (const time of ['0', '-1', '1.5', '8640000000000001']) {
    assert.throws(() => db.exec(`UPDATE session_activity SET last_activity_at=${time}`));
  }
  assert.throws(() => receipt(db, ['one', 'bad-length', Buffer.alloc(31), null, null]));
  assert.throws(() => receipt(db, ['one', 'unpaired', digest('x'), 1, null]));
  assert.throws(() => receipt(db, ['one', 'unknown', null, null, null]));
  assert.throws(() => receipt(db, ['missing-session', 'x', digest('x'), null, null]));
});

test('summary and receipts survive database reopen and cascade only with the session', () => {
  const dir = mkdtempSync(join(tmpdir(), 'cafe-activity-sql-'));
  const path = join(dir, 'state.sqlite3');
  let db;
  try {
    db = new DatabaseSync(path);
    db.exec('PRAGMA foreign_keys=ON');
    for (const v of [2, 5, 6]) db.exec(migration(v));
    db.exec("INSERT INTO sessions VALUES ('one','{}','{}')");
    receipt(db, ['one', 'message-1', digest('tea'), 1, digest('tea')]);
    metadata(db, ['one', 100, 'tea', 'message-1', 1]);
    db.exec("INSERT INTO transcript_messages VALUES ('one','message-1','tea'); DELETE FROM transcript_messages");
    db.close();
    db = new DatabaseSync(path);
    db.exec('PRAGMA foreign_keys=ON');
    assert.equal(db.prepare('SELECT preview FROM session_activity').get().preview, 'tea');
    assert.equal(receipt(db, ['one', 'message-1', digest('tea'), 1, digest('tea')]), 0);
    db.exec("DELETE FROM sessions WHERE id='one'");
    for (const table of ['session_activity', 'session_message_receipts']) {
      assert.equal(db.prepare(`SELECT count(*) AS n FROM ${table}`).get().n, 0);
    }
  } finally {
    db?.close();
    rmSync(dir, { recursive: true, force: true });
  }
});
