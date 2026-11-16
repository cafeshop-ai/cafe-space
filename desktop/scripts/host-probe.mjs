// Explicit fake-Host/native-WebView probe. Does not build, register startup or launch real Pi.
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { readFile, mkdtemp, mkdir, rm } from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const repo = fileURLToPath(new URL('../..', import.meta.url));
const contract = JSON.parse(await readFile(new URL('../../fixtures/pi-rpc/desktop-host-v5.json', import.meta.url), 'utf8'));
export function managedArtifact(text) {
  const paths = new Set(text.trim().split('\n').flatMap(line => {
    try { return [JSON.parse(line)]; } catch { return []; }
  }).filter(item => item?.reason === 'compiler-artifact' && item.target?.name === 'managed' && item.profile?.test && item.executable)
    .map(item => item.executable));
  assert.equal(paths.size, 1, 'MANAGED_ARTIFACT_REQUIRED');
  const [executable] = paths;
  assert(path.isAbsolute(executable), 'MANAGED_ARTIFACT_MUST_BE_ABSOLUTE');
  return executable;
}
function markedValue(text, marker, code) {
  let invalid = false;
  for (const line of text.split(/\r?\n/).filter(value => value.includes(marker) && value.trimStart().startsWith('{'))) {
    try {
      const value = JSON.parse(line);
      if (value && typeof value === 'object' && value.marker === marker) return value;
    } catch { invalid = true; }
  }
  throw new Error(invalid ? `${code}_INVALID_JSON` : `${code}_REQUIRED`);
}
export function fixtureReady(text) {
  const value = markedValue(text, 'M1_DESKTOP_HOST_FIXTURE_READY', 'FIXTURE_READY');
  assert.equal(typeof value.endpoint, 'string', 'FIXTURE_READY_ENDPOINT_REQUIRED');
  assert(value.endpoint.length > 0 && value.endpoint.length < 512, 'FIXTURE_READY_ENDPOINT_INVALID');
  assert.equal(value.session?.id, contract.sessionId, 'FIXTURE_READY_SESSION_REQUIRED');
  assert.equal(value.session?.state?.state, 'ready', 'FIXTURE_READY_STATE_REQUIRED');
  return value;
}
export function fixtureStopped(text) {
  const summary = markedValue(text, 'M1_DESKTOP_HOST_FIXTURE_STOPPED', 'FIXTURE_STOPPED');
  assert.deepEqual(summary.nativeCommands, contract.expectedNativeCommands, 'NATIVE_COMMAND_ORDER_OR_REPLAY');
  assert.equal(summary.promptCount, contract.expectedNativeCommands.filter(command => command === 'prompt').length, 'FIXTURE_PROMPT_COUNT_MISMATCH');
  return summary;
}
export function webviewFinished(result, output) {
  assert.deepEqual(result, { code: 0, signal: null }, 'WEBVIEW_EXIT_UNCONFIRMED');
  const lines = output.split(/\r?\n/);
  const markers = ['M1_HOST_WEBVIEW_OK', 'M1_HOST_WEBVIEW_FINAL_ACK_OK'];
  for (const marker of markers) assert(lines.includes(marker), marker + '_REQUIRED');
  assert(!lines.includes('M1_HOST_WEBVIEW_FAILED'), 'M1_HOST_WEBVIEW_FAILED');
  return lines.filter(line => markers.includes(line));
}
function bounded(promise, ms, code) {
  let timer;
  return Promise.race([promise, new Promise((_, reject) => { timer = setTimeout(() => reject(new Error(code)), ms); })])
    .finally(() => clearTimeout(timer));
}
function run(executable, args, env) {
  const child = spawn(executable, args, { cwd: repo, env, stdio: ['ignore', 'pipe', 'pipe'] });
  let output = '', spawned = false;
  const started = new Promise(resolve => {
    child.once('spawn', () => { spawned = true; resolve(true); });
    child.once('close', () => resolve(false));
  });
  child.on('error', () => {});
  child.stdout.setEncoding('utf8');
  child.stdout.on('data', chunk => { output = (output + chunk).slice(-65536); });
  // Drain, but never export native diagnostics or fixture paths.
  child.stderr.resume();
  const done = new Promise(resolve => child.once('close', (code, signal) =>
    resolve(spawned ? { code, signal } : { code: null, signal: 'SPAWN_ERROR' })));
  return { child, done, started, spawned: () => spawned, output: () => output };
}
export function ready(host) {
  return new Promise((resolve, reject) => {
    let timer;
    const finish = (error, value) => {
      clearTimeout(timer);
      host.child.stdout.off('data', check);
      host.child.off('close', onClose);
      host.child.off('error', onError);
      if (error) reject(error); else resolve(value);
    };
    const check = () => {
      try { finish(null, fixtureReady(host.output())); }
      catch (error) {
        if (['FIXTURE_READY_REQUIRED', 'FIXTURE_READY_INVALID_JSON'].includes(error.message)) return;
        finish(new Error(safeCode(error)));
      }
    };
    const onClose = () => finish(new Error('FIXTURE_EARLY_EXIT'));
    const onError = () => finish(new Error('FIXTURE_SPAWN_FAILED'));
    timer = setTimeout(() => finish(new Error('FIXTURE_START_TIMEOUT')), 15000);
    host.child.stdout.on('data', check);
    host.child.once('close', onClose);
    host.child.once('error', onError);
    check();
  });
}
function abandon(process) {
  const { child } = process;
  if (process.spawned()) {
    if (child.exitCode === null && child.signalCode === null) try { child.kill('SIGKILL'); } catch {}
  } else child.once('spawn', () => { try { child.kill('SIGKILL'); } catch {} });
  // Do not hold CI open after a failed stop; the isolated root stays for diagnosis.
  child.stdout?.destroy();
  child.stderr?.destroy();
  child.unref();
}
export async function stop(process) {
  if (!process) return;
  const { child, done } = process;
  if (!process.spawned()) {
    try { if (!await bounded(process.started, 5000, 'PROBE_CLOSE_TIMEOUT')) return; }
    catch { abandon(process); throw new Error('PROBE_CLOSE_TIMEOUT'); }
  }
  if (child.exitCode !== null || child.signalCode !== null) {
    try { await bounded(done, 5000, 'PROBE_CLOSE_TIMEOUT'); }
    catch { abandon(process); throw new Error('PROBE_CLOSE_TIMEOUT'); }
    return;
  }
  try {
    child.kill();
    await bounded(done, 5000, 'PROBE_STOP_TIMEOUT');
  } catch {
    try {
      child.kill('SIGKILL');
      await bounded(done, 5000, 'PROBE_KILL_TIMEOUT');
    } catch { abandon(process); throw new Error('PROBE_KILL_TIMEOUT'); }
  }
}
const stages = new Set(['authorization', 'artifact', 'isolation', 'fixture_ready', 'webview_exit', 'webview_proof', 'fixture_stop', 'fixture_summary', 'stop_app', 'stop_host', 'cleanup']);
const codes = new Set(['UNEXPECTED_PROBE_ERROR', 'M1_TEST_OPT_IN_REQUIRED', 'MANAGED_ARTIFACT_REQUIRED', 'MANAGED_ARTIFACT_MUST_BE_ABSOLUTE', 'FIXTURE_READY_REQUIRED', 'FIXTURE_READY_INVALID_JSON', 'FIXTURE_READY_ENDPOINT_REQUIRED', 'FIXTURE_READY_ENDPOINT_INVALID', 'FIXTURE_READY_SESSION_REQUIRED', 'FIXTURE_READY_STATE_REQUIRED', 'FIXTURE_SPAWN_FAILED', 'FIXTURE_EARLY_EXIT', 'FIXTURE_START_TIMEOUT', 'M1_WEBVIEW_TIMEOUT', 'WEBVIEW_SPAWN_FAILED', 'WEBVIEW_EXIT_UNCONFIRMED', 'M1_HOST_WEBVIEW_OK_REQUIRED', 'M1_HOST_WEBVIEW_FINAL_ACK_OK_REQUIRED', 'M1_HOST_WEBVIEW_FAILED', 'FIXTURE_STOP_TIMEOUT', 'FIXTURE_EXIT_UNCONFIRMED', 'FIXTURE_STOPPED_REQUIRED', 'FIXTURE_STOPPED_INVALID_JSON', 'NATIVE_COMMAND_ORDER_OR_REPLAY', 'FIXTURE_PROMPT_COUNT_MISMATCH', 'PROBE_CLOSE_TIMEOUT', 'PROBE_STOP_TIMEOUT', 'PROBE_KILL_TIMEOUT']);
export function failureStage(error) {
  return stages.has(error?.cause?.stage) ? error.cause.stage : 'unknown';
}
export function failureCode(error) {
  return codes.has(error?.cause?.code) ? error.cause.code : 'UNEXPECTED_PROBE_ERROR';
}
export function failureCleanup(error) {
  return Array.isArray(error?.cause?.cleanup) ? error.cause.cleanup
    .filter(item => stages.has(item?.stage) && codes.has(item?.code))
    .map(({ stage, code }) => ({ stage, code })) : [];
}
export function safeCode(error) {
  const code = error?.message?.split(/\r?\n/, 1)[0];
  return codes.has(code) ? code : 'UNEXPECTED_PROBE_ERROR';
}
export async function probe() {
  let stage = 'authorization', failed, root, host, app, finished, summary;
  const cleanup = [];
  try {
    if (process.env.CAFE_SPACE_TEST_DESKTOP !== '1') throw new Error('M1_TEST_OPT_IN_REQUIRED');
    stage = 'artifact';
    const artifact = managedArtifact(await readFile(path.join(repo, 'target/desktop-host-artifacts.json'), 'utf8'));
    stage = 'isolation';
    root = await mkdtemp(path.join(process.platform === 'darwin' ? '/tmp' : os.tmpdir(), 'cafe-space-m1-'));
    const env = { ...process.env, HOME: root, XDG_DATA_HOME: path.join(root, 'data'), XDG_CONFIG_HOME: path.join(root, 'config'), XDG_CACHE_HOME: path.join(root, 'cache'), XDG_RUNTIME_DIR: path.join(root, 'runtime'), TMPDIR: path.join(root, 'tmp'), LOCALAPPDATA: path.join(root, 'Local'), APPDATA: path.join(root, 'Roaming') };
    delete env.CAFE_SPACE_M0_PROBE;
    delete env.CAFE_SPACE_M1_PROBE;
    delete env.CAFE_SPACE_M1_PROBE_ENDPOINT;
    for (const folder of [env.XDG_DATA_HOME, env.XDG_CONFIG_HOME, env.XDG_CACHE_HOME, env.XDG_RUNTIME_DIR, env.TMPDIR, env.LOCALAPPDATA, env.APPDATA, path.join(root, 'Library/Application Support')]) await mkdir(folder, { recursive: true, mode: 0o700 });
    stage = 'fixture_ready';
    host = run(artifact, ['--desktop-host-fixture'], env);
    const metadata = await ready(host);
    const suffix = process.platform === 'win32' ? '.exe' : '';
    stage = 'webview_exit';
    app = run(path.join(repo, `desktop/src-tauri/target/debug/cafe-space-desktop${suffix}`), [], {
      ...env, CAFE_SPACE_M1_PROBE: '1', CAFE_SPACE_M1_PROBE_ENDPOINT: metadata.endpoint,
    });
    const result = await bounded(app.done, 60000, 'M1_WEBVIEW_TIMEOUT');
    if (result.signal === 'SPAWN_ERROR') throw new Error('WEBVIEW_SPAWN_FAILED');
    if (app.output().split(/\r?\n/).includes('M1_HOST_WEBVIEW_FAILED')) throw new Error('M1_HOST_WEBVIEW_FAILED');
    assert.deepEqual(result, { code: 0, signal: null }, 'WEBVIEW_EXIT_UNCONFIRMED');
    stage = 'webview_proof';
    finished = webviewFinished(result, app.output());
    stage = 'fixture_stop';
    const stopped = await bounded(host.done, 10000, 'FIXTURE_STOP_TIMEOUT');
    assert.deepEqual(stopped, { code: 0, signal: null }, 'FIXTURE_EXIT_UNCONFIRMED');
    stage = 'fixture_summary';
    summary = fixtureStopped(host.output());
  } catch (error) {
    failed = { stage, code: safeCode(error) };
  } finally {
    // Keep the isolated root if either owned child did not close within its cleanup deadline.
    for (const [process, stage] of [[app, 'stop_app'], [host, 'stop_host']]) {
      try { await stop(process); } catch (error) { cleanup.push({ stage, code: safeCode(error) }); }
    }
    if (root && !cleanup.length) {
      try { await rm(root, { recursive: true, force: true, maxRetries: 10, retryDelay: 200 }); }
      catch (error) { cleanup.push({ stage: 'cleanup', code: safeCode(error) }); }
    }
  }
  if (failed || cleanup.length) {
    const primary = failed ?? cleanup.shift();
    throw new Error('M1_HOST_WEBVIEW_PROBE_FAILED', { cause: { ...primary, cleanup } });
  }
  for (const marker of finished) console.log(marker);
  console.log(JSON.stringify({ platform: process.platform, nativeWebview: true, channel: true, finalChannelAck: true, host: true, fakeAgent: true, normalUi: true, promptCount: summary.promptCount, cancelCount: summary.nativeCommands.filter(command => command === 'abort').length, cancelOrder: true, confirmationCount: summary.nativeCommands.filter(command => command === 'extension_ui_response').length, confirmationOrder: true, modelCalls: 0, productionDisabled: true, startupRegistration: false }));
}
if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  try { await probe(); } catch (error) {
    console.error('M1_HOST_WEBVIEW_PROBE_FAILED');
    console.error(JSON.stringify({ stage: failureStage(error), code: failureCode(error), cleanup: failureCleanup(error) }));
    process.exitCode = 1;
  }
}
