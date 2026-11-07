// Explicit native WebView probe. It never registers startup or touches an existing daemon.
import assert from 'node:assert/strict';
import { spawn, spawnSync } from 'node:child_process';
import { mkdtemp, mkdir, rm } from 'node:fs/promises';
import net from 'node:net';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { setTimeout as delay } from 'node:timers/promises';

assert.equal(process.env.CAFE_SPACE_TEST_DESKTOP, '1', 'Enable only for an isolated native desktop probe.');
const repo = fileURLToPath(new URL('../..', import.meta.url));
const root = await mkdtemp(path.join(process.platform === 'darwin' ? '/tmp' : os.tmpdir(), 'cafe-space-m0-'));
const env = { ...process.env, HOME: root, XDG_DATA_HOME: path.join(root, 'data'), XDG_CONFIG_HOME: path.join(root, 'config'), XDG_CACHE_HOME: path.join(root, 'cache'), XDG_RUNTIME_DIR: path.join(root, 'runtime'), TMPDIR: path.join(root, 'tmp'), LOCALAPPDATA: path.join(root, 'Local'), APPDATA: path.join(root, 'Roaming'), CAFE_SPACE_M0_PROBE: '1' };
for (const folder of [env.XDG_DATA_HOME, env.XDG_CONFIG_HOME, env.XDG_CACHE_HOME, env.XDG_RUNTIME_DIR, env.TMPDIR, env.LOCALAPPDATA, env.APPDATA, path.join(root, 'Library/Application Support')]) await mkdir(folder, { recursive: true, mode: 0o700 });
let endpoint;
if (process.platform === 'win32') {
  const sid = spawnSync('powershell.exe', ['-NoProfile', '-NonInteractive', '-Command', '[System.Security.Principal.WindowsIdentity]::GetCurrent().User.Value'], { encoding: 'utf8', timeout: 10000 });
  assert.equal(sid.status, 0, 'Cannot resolve the current user SID.');
  assert.match(sid.stdout.trim(), /^S-1-[0-9-]+$/);
  endpoint = `\\\\.\\pipe\\cafe-spaced-dev-${sid.stdout.trim()}`;
} else if (process.platform === 'darwin') {
  const normal = path.join(root, 'Library/Application Support/cafe-space-dev/run/cafe-spaced-dev.sock');
  endpoint = Buffer.byteLength(normal) < 104 ? normal : path.join(env.TMPDIR, 'cafe-space-dev/cafe-spaced-dev.sock');
} else endpoint = path.join(env.XDG_RUNTIME_DIR, 'cafe-space-dev/cafe-spaced-dev.sock');

function connect() {
  return new Promise((resolve, reject) => {
    const socket = net.createConnection(endpoint);
    socket.setTimeout(5000, () => socket.destroy(new Error('IPC_READ_TIMEOUT')));
    socket.once('error', reject);
    socket.once('connect', () => resolve(socket));
  });
}
async function checkSessions() {
  const socket = await connect();
  const frames = [], waiting = [];
  let buffer = Buffer.alloc(0), failure;
  const fail = error => { failure = error; while (waiting.length) waiting.shift().reject(error); };
  socket.on('error', fail);
  socket.on('close', () => fail(new Error('IPC_CLOSED')));
  socket.on('data', bytes => {
    buffer = Buffer.concat([buffer, bytes]);
    if (buffer.length > 262148 * 4) { socket.destroy(new Error('IPC_BUFFER_LIMIT')); return; }
    while (buffer.length >= 4) {
      const length = buffer.readUInt32BE();
      if (!length || length > 262144) { socket.destroy(new Error('IPC_FRAME_LIMIT')); return; }
      if (buffer.length < 4 + length) return;
      let frame;
      try { frame = JSON.parse(buffer.subarray(4, 4 + length).toString('utf8')); }
      catch { socket.destroy(new Error('IPC_INVALID_JSON')); return; }
      buffer = buffer.subarray(4 + length);
      if (waiting.length) waiting.shift().resolve(frame);
      else if (frames.length < 4) frames.push(frame);
      else socket.destroy(new Error('IPC_QUEUE_LIMIT'));
    }
  });
  const receive = () => failure ? Promise.reject(failure) : frames.length ? Promise.resolve(frames.shift()) : new Promise((resolve, reject) => waiting.push({ resolve, reject }));
  const send = value => { const body = Buffer.from(JSON.stringify(value)); const header = Buffer.alloc(4); header.writeUInt32BE(body.length); socket.write(Buffer.concat([header, body])); };
  try {
    send({ type: 'hello', supportedProtocolVersions: [6], clientVersion: 'm0-native-probe' });
    assert.equal((await receive()).type, 'welcome');
    send({ type: 'request', protocolVersion: 6, requestId: 'probe-list', request: { method: 'list_sessions' } });
    const response = await receive();
    assert.equal(response.type, 'response');
    assert.equal(response.result.status, 'applied');
    assert.equal(response.result.data.result.kind, 'sessions');
    assert.deepEqual(response.result.data.result.data.items, []);
  } finally { socket.destroy(); }
}

const suffix = process.platform === 'win32' ? '.exe' : '';
let daemon, app;
const run = executable => {
  const child = spawn(executable, [], { cwd: repo, env, stdio: ['ignore', 'pipe', 'pipe'] });
  let output = '';
  for (const stream of [child.stdout, child.stderr]) stream.on('data', chunk => { output = (output + chunk.toString('utf8')).slice(-65536); });
  child.on('error', () => {});
  child.probeOutput = () => output;
  return child;
};
const stop = async child => {
  if (!child || child.exitCode !== null || child.signalCode !== null) return;
  const exited = new Promise(resolve => child.once('exit', resolve));
  child.kill();
  await Promise.race([exited, delay(5000, undefined, { ref: false })]);
  if (child.exitCode === null && child.signalCode === null) { child.kill('SIGKILL'); await exited; }
};
try {
  let existing;
  try { existing = await connect(); } catch (error) { if (!['ENOENT', 'ECONNREFUSED'].includes(error.code)) throw error; }
  if (existing) { existing.destroy(); throw new Error('IPC_ENDPOINT_ALREADY_IN_USE'); }
  daemon = run(path.join(repo, `target/debug/cafe-spaced-dev${suffix}`));
  const readyDeadline = Date.now() + 10000;
  while (true) {
    assert.equal(daemon.exitCode, null, `Daemon exited before readiness: ${daemon.probeOutput()}`);
    try { await checkSessions(); break; }
    catch (error) { if (Date.now() >= readyDeadline) throw error; await delay(100); }
  }
  app = run(path.join(repo, `desktop/src-tauri/target/debug/cafe-space-desktop${suffix}`));
  const result = await Promise.race([
    new Promise((resolve, reject) => { app.once('exit', code => resolve(code)); app.once('error', reject); }),
    delay(120000, undefined, { ref: false }).then(() => { throw new Error('NATIVE_WEBVIEW_PROBE_TIMEOUT'); }),
  ]);
  assert.equal(result, 0, `Native WebView failed: ${app.probeOutput()}`);
  assert.match(app.probeOutput(), /M0_PROXY_WEBVIEW_OK/);
  assert.equal(daemon.exitCode, null, 'Closing the shell must not terminate the daemon.');
  await checkSessions();
  console.log(JSON.stringify({ platform: process.platform, nativeWebview: true, channel: true, sessions: 0, daemonSurvivedShellExit: true, startupRegistration: false }));
} finally {
  await stop(app); await stop(daemon);
  // Only this freshly created, isolated probe directory is removed.
  await rm(root, { recursive: true, force: true, maxRetries: 10, retryDelay: 200 });
}
