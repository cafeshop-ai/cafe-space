// Explicit fake-Host/native-WebView probe. Does not build, register startup or launch real Pi.
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { readFile, mkdtemp, mkdir, rm } from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const repo = fileURLToPath(new URL('../..', import.meta.url));
export function managedArtifact(text) {
  const paths = new Set(text.trim().split('\n').map(line => JSON.parse(line))
    .filter(item => item.reason === 'compiler-artifact' && item.target?.name === 'managed' && item.profile?.test && item.executable)
    .map(item => item.executable));
  assert.equal(paths.size, 1, 'MANAGED_ARTIFACT_REQUIRED');
  const [executable] = paths;
  assert(path.isAbsolute(executable), 'MANAGED_ARTIFACT_MUST_BE_ABSOLUTE');
  return executable;
}
export function fixtureReady(text) {
  const value = JSON.parse(text.split('\n')[0]);
  assert.equal(value.marker, 'M1_DESKTOP_HOST_FIXTURE_READY');
  assert.equal(typeof value.endpoint, 'string');
  assert(value.endpoint.length > 0 && value.endpoint.length < 512);
  assert.equal(value.session?.id, 'host-commands');
  assert.equal(value.session?.state?.state, 'ready');
  return value;
}
function bounded(promise, ms, code) {
  let timer;
  return Promise.race([promise, new Promise((_, reject) => { timer = setTimeout(() => reject(new Error(code)), ms); })])
    .finally(() => clearTimeout(timer));
}
function run(executable, args, env) {
  const child = spawn(executable, args, { cwd: repo, env, stdio: ['ignore', 'pipe', 'pipe'] });
  let output = '';
  child.stdout.setEncoding('utf8');
  child.stdout.on('data', chunk => { output = (output + chunk).slice(-65536); });
  // Drain, but never export native diagnostics or fixture paths.
  child.stderr.resume();
  const done = new Promise(resolve => {
    child.once('error', () => resolve({ code: null, signal: 'SPAWN_ERROR' }));
    child.once('close', (code, signal) => resolve({ code, signal }));
  });
  return { child, done, output: () => output };
}
async function ready(host) {
  return bounded(new Promise((resolve, reject) => {
    const check = () => {
      if (!host.output().includes('\n')) return;
      host.child.stdout.off('data', check);
      try { resolve(fixtureReady(host.output())); } catch { reject(new Error('FIXTURE_READY_INVALID')); }
    };
    host.child.stdout.on('data', check);
    void host.done.then(() => reject(new Error('FIXTURE_EARLY_EXIT')));
    check();
  }), 15000, 'FIXTURE_START_TIMEOUT');
}
async function stop(process) {
  if (!process || process.child.exitCode !== null || process.child.signalCode !== null) return;
  process.child.kill();
  try { await bounded(process.done, 5000, 'PROBE_STOP_TIMEOUT'); }
  catch { process.child.kill('SIGKILL'); await bounded(process.done, 5000, 'PROBE_KILL_TIMEOUT'); }
}
export async function probe() {
  assert.equal(process.env.CAFE_SPACE_TEST_DESKTOP, '1');
  const artifact = managedArtifact(await readFile(path.join(repo, 'target/desktop-host-artifacts.json'), 'utf8'));
  const root = await mkdtemp(path.join(process.platform === 'darwin' ? '/tmp' : os.tmpdir(), 'cafe-space-m1-'));
  const env = { ...process.env, HOME: root, XDG_DATA_HOME: path.join(root, 'data'), XDG_CONFIG_HOME: path.join(root, 'config'), XDG_CACHE_HOME: path.join(root, 'cache'), XDG_RUNTIME_DIR: path.join(root, 'runtime'), TMPDIR: path.join(root, 'tmp'), LOCALAPPDATA: path.join(root, 'Local'), APPDATA: path.join(root, 'Roaming') };
  delete env.CAFE_SPACE_M0_PROBE;
  delete env.CAFE_SPACE_M1_PROBE;
  delete env.CAFE_SPACE_M1_PROBE_ENDPOINT;
  let host, app;
  try {
    for (const folder of [env.XDG_DATA_HOME, env.XDG_CONFIG_HOME, env.XDG_CACHE_HOME, env.XDG_RUNTIME_DIR, env.TMPDIR, env.LOCALAPPDATA, env.APPDATA, path.join(root, 'Library/Application Support')]) await mkdir(folder, { recursive: true, mode: 0o700 });
    host = run(artifact, ['--desktop-host-fixture'], env);
    const metadata = await ready(host);
    const suffix = process.platform === 'win32' ? '.exe' : '';
    app = run(path.join(repo, `desktop/src-tauri/target/debug/cafe-space-desktop${suffix}`), [], {
      ...env, CAFE_SPACE_M1_PROBE: '1', CAFE_SPACE_M1_PROBE_ENDPOINT: metadata.endpoint,
    });
    assert.deepEqual(await bounded(app.done, 60000, 'M1_WEBVIEW_TIMEOUT'), { code: 0, signal: null });
    assert(app.output().includes('M1_HOST_WEBVIEW_OK'));
    assert.deepEqual(await bounded(host.done, 10000, 'FIXTURE_STOP_TIMEOUT'), { code: 0, signal: null });
    const summary = host.output().trim().split('\n').map(line => JSON.parse(line)).at(-1);
    assert.equal(summary.marker, 'M1_DESKTOP_HOST_FIXTURE_STOPPED');
    assert.equal(summary.promptCount, 1, 'PROMPT_MUST_NOT_REPLAY');
    console.log(JSON.stringify({ platform: process.platform, nativeWebview: true, channel: true, host: true, fakeAgent: true, promptCount: 1, modelCalls: 0, productionDisabled: true, startupRegistration: false }));
  } finally {
    try { await stop(app); }
    finally {
      try { await stop(host); }
      finally { await rm(root, { recursive: true, force: true, maxRetries: 10, retryDelay: 200 }); }
    }
  }
}
if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  try { await probe(); } catch { console.error('M1_HOST_WEBVIEW_PROBE_FAILED'); process.exitCode = 1; }
}
