// Synthetic ACP only: no imports of agent runtimes, network, credentials or project data.
import { readFileSync } from 'node:fs';
import { once } from 'node:events';

const fixture = JSON.parse(readFileSync(new URL('../fixtures/acp/contract.json', import.meta.url), 'utf8'));
const mode = process.argv[2] ?? 'normal';
if (!['normal', 'crash', 'malformed', 'wrong-session', 'permission', 'cancel', 'load-fails'].includes(mode)) throw new Error('unknown fixture mode');
const nativeId = 'private-synthetic-session';
let prompt;
let initialized = false;
let opened = false;
async function send(value) {
  if (!process.stdout.write(`${JSON.stringify(value)}\n`)) await once(process.stdout, 'drain');
}
const respond = (request, result) => send({ jsonrpc: '2.0', id: request.id, result });
async function handle(request) {
  if (request.jsonrpc !== '2.0') throw new Error('bad RPC envelope');
  if (request.method === 'initialize') {
    if (initialized || request.params.protocolVersion !== 1) throw new Error('bad initialization');
    initialized = true;
    await respond(request, fixture.initialized.result);
  } else if (request.method === 'session/new' || request.method === 'session/load') {
    if (!initialized || opened || request.params.mcpServers.length) throw new Error('bad open');
    if (request.method === 'session/load') {
      if (request.params.sessionId !== nativeId) throw new Error('wrong resume identity');
      if (mode === 'load-fails') {
        await send({jsonrpc:'2.0', id:request.id, error:{code:-32000, message:'synthetic unavailable history'}});
        return;
      }
    }
    opened = true;
    await respond(request, fixture.created.result);
  } else if (request.method === 'session/prompt') {
    if (!opened || prompt || request.params.sessionId !== nativeId) throw new Error('bad prompt');
    prompt = request;
    if (mode === 'crash') process.exit(23);
    if (mode === 'malformed') { process.stdout.write('invalid-synthetic-json\n'); return; }
    const update = structuredClone(fixture.update);
    if (mode === 'wrong-session') update.params.sessionId = 'wrong-private-session';
    await send(update);
    if (mode === 'permission' || mode === 'cancel') await send(fixture.permission);
    else { await respond(prompt, fixture.completed.result); prompt = undefined; }
  } else if (request.method === 'session/cancel') {
    if (!prompt || request.params.sessionId !== nativeId) throw new Error('bad cancel');
    // Wait for the mandatory cancelled permission response before settling.
  } else if (request.id === fixture.permission.id && request.result) {
    if (!prompt) throw new Error('stale approval');
    if (mode === 'cancel' && request.result.outcome.outcome !== 'cancelled') throw new Error('cancel was not denied');
    if (mode === 'permission' && request.result.outcome.optionId !== 'once') throw new Error('wrong permission selection');
    await respond(prompt, mode === 'cancel' ? fixture.cancelled.result : fixture.completed.result);
    prompt = undefined;
  } else if (request.method === 'session/set_mode') {
    await respond(request, {});
  } else if (request.method === 'session/set_config_option') {
    await respond(request, {configOptions:fixture.created.result.configOptions});
  } else throw new Error('unexpected synthetic request');
}
let pending = Buffer.alloc(0);
for await (const chunk of process.stdin) {
  pending = Buffer.concat([pending, chunk]);
  let end;
  while ((end = pending.indexOf(10)) !== -1) {
    if (end > fixture.limits.recordBytes) throw new Error('record too large');
    const line = pending.subarray(0, end).toString('utf8');
    pending = pending.subarray(end + 1);
    await handle(JSON.parse(line));
  }
  if (pending.length > fixture.limits.recordBytes) throw new Error('record too large');
}
if (pending.length) throw new Error('incomplete record');
