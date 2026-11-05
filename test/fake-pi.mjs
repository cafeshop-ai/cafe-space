// Offline fixture only. Does not import Pi, read credentials, use tools or call a model/network.
import assert from 'node:assert/strict';
import { appendFileSync, readFileSync, renameSync, statSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';

const args = process.argv.slice(2);
const value = flag => args[args.indexOf(flag) + 1];
for (const flag of ['--mode', '--offline', '--no-extensions', '--no-approve', '--no-themes', '--session-dir']) assert(args.includes(flag));
assert.equal(value('--mode'), 'rpc');
assert.equal(process.env.CAFE_SPACE_TEST_SECRET, undefined);
assert.notEqual(args.includes('--session'), args.includes('--session-id'));
const historyFile = args.includes('--session') ? value('--session') : join(value('--session-dir'), `2000-01-01_${value('--session-id')}.jsonl`);
const history = args.includes('--session') ? readFileSync(historyFile, 'utf8').trim().split('\n').map(line => JSON.parse(line)) : [];
const nativeId = args.includes('--session') ? history[0].id : value('--session-id');
const mode = args.includes('--fake-mode') ? value('--fake-mode') : 'normal';
if (args.includes('--fake-pid-file')) {
  const file = value('--fake-pid-file');
  writeFileSync(file + '.tmp', String(process.pid), { flag: 'wx' });
  renameSync(file + '.tmp', file);
}
if (mode === 'no-read') setInterval(() => {}, 1000);
let model = mode === 'initial-config' ? {provider:'synthetic',id:'initial'} : undefined;
let thinkingLevel;
let delayedSteer;
let dialog;
let buffer = Buffer.alloc(0);
const emit = record => process.stdout.write(JSON.stringify(record) + '\n');
const response = (request, data) => emit({type:'response', id:request.id, command:request.type, success:true, ...(data === undefined ? {} : {data})});
const settled = aborted => { emit({type:'agent_end', willRetry:false, messages:[]}); emit({type:'agent_settled', aborted}); };
process.stderr.write('synthetic diagnostics are not RPC records\n');
process.stdin.on('data', chunk => {
  buffer = Buffer.concat([buffer, chunk]);
  assert(buffer.length <= 1048576);
  let newline;
  while ((newline = buffer.indexOf(10)) !== -1) {
    const record = JSON.parse(buffer.subarray(0,newline).toString('utf8'));
    buffer = buffer.subarray(newline+1);
    if (args.includes('--fake-command-file')) appendFileSync(value('--fake-command-file'), record.type + '\n');
    switch (record.type) {
      case 'get_state':
        if (mode === 'startup-crash') process.exit(17);
        if (mode === 'startup-timeout') break;
        if (mode === 'startup-malformed') { process.stdout.write('{synthetic invalid record}\n'); break; }
        if (mode === 'readback-rejected' && model) { emit({type:'response',id:record.id,command:record.type,success:false,error:'synthetic-private-detail'}); break; }
        response(record, {sessionId:nativeId, sessionFile:'/private/must-not-forward.jsonl', isStreaming:false, isCompacting:false, pendingMessageCount:0, ...(model && mode !== 'readback-missing' ? {model,thinkingLevel:thinkingLevel ?? 'off'} : {})});
        if (mode === 'unowned') emit({type:'agent_start'});
        if (mode === 'no-read') process.stdin.pause();
        break;
      case 'prompt':
        if (mode === 'stderr-flood') process.stderr.write('synthetic-private-diagnostics'.repeat(100000));
        if (mode === 'no-response') break;
        if (mode === 'start-no-ack') { emit({type:'agent_start'}); break; }
        if (mode === 'rejected') { emit({type:'response',id:record.id,command:record.type,success:false,error:'synthetic-private-detail'}); break; }
        if (mode === 'crash') process.exit(17);
        if (mode === 'malformed') { process.stdout.write('{synthetic invalid record}\n'); break; }
        if (record.message === '/handled') {response(record, {disposition:'handled'}); break;}
        if (mode === 'delayed-start') { response(record, {disposition:'started'}); break; }
        // Events may precede the command acknowledgement.
        emit({type:'agent_start'});
        response(record, {disposition:'started'});
        if (record.message === 'busy') break;
        if (record.message === 'display-limits') {
          emit({type:'message_start',message:{role:'assistant',content:[]}});
          emit({type:'message_update',assistantMessageEvent:{type:'text_start',contentIndex:0}});
          emit({type:'message_update',assistantMessageEvent:{type:'text_delta',contentIndex:0,delta:'x'.repeat(1048577)}});
          emit({type:'message_end',message:{role:'assistant',content:[{type:'image',data:'synthetic-not-displayed'},{type:'future-content',data:'synthetic-not-displayed'}]}});
          emit({type:'tool_execution_start',toolCallId:'large-tool',toolName:'synthetic',args:{}});
          emit({type:'tool_execution_end',toolCallId:'large-tool',isError:false,result:{content:[{type:'text',text:'茶'.repeat(65537)}]}});
          emit({type:'message_end',message:{role:'assistant',content:[{type:'text',text:'display limit survived'}]}});
          settled(false);
          break;
        }
        if (record.message === 'approval' || record.message === 'large-approval') {
          dialog = 'private-confirm';
          emit({type:'extension_ui_request', id:dialog, method:'confirm', title: record.message === 'large-approval' ? '\\'.repeat(40000) : 'Synthetic confirmation'});
          break;
        }
        if (record.message === 'local-ui') { emit({type:'extension_ui_request',id:'private-input',method:'input',title:'Synthetic input'}); break; }
        const reply = record.message === 'recall'
          ? history.at(-1)?.message?.content?.[0]?.text ?? 'No native history'
          : 'Final synthetic 茶';
        const message = {role:'assistant',content:[{type:'text',text:reply}],stopReason:'stop'};
        if (!history.length) {
          history.push({type:'session',version:3,id:nativeId,cwd:process.cwd()});
          const header = JSON.stringify(history[0]) + '\n';
          try { writeFileSync(historyFile, header, {flag:'wx'}); }
          catch (error) {
            if (error.code !== 'EEXIST') throw error;
            // The Rust fixture may preallocate an owner-only empty file on elevated Windows CI.
            assert.equal(statSync(historyFile).size, 0);
            writeFileSync(historyFile, header, {flag:'r+'});
          }
        }
        const entry = {type:'message',id:`synthetic-${history.length}`,parentId:history.at(-1)?.id ?? null,message};
        appendFileSync(historyFile, JSON.stringify(entry) + '\n');
        history.push(entry);
        emit({type:'message_start', message:{role:'assistant',content:[]}});
        emit({type:'message_update', assistantMessageEvent:{type:'text_start',contentIndex:0}});
        emit({type:'message_update', assistantMessageEvent:{type:'text_delta',contentIndex:0,delta:'Tea 茶\u2028second\u2029third'}});
        emit({type:'message_end', message});
        emit({type:'agent_end',willRetry:true,messages:[]});
        emit({type:'compaction_start',reason:'overflow'});
        settled(false);
        break;
      case 'steer': delayedSteer = record; break;
      case 'follow_up':
        response(record,{disposition:'queued'});
        if (delayedSteer) {response(delayedSteer,{disposition:'queued'}); delayedSteer=undefined;}
        break;
      case 'set_model': model={provider:record.provider,id:record.modelId}; response(record,model); break;
      case 'set_thinking_level': thinkingLevel=record.level === 'high' ? 'low' : record.level; response(record); break;
      case 'clear_queue': if (mode === 'approval-crash') process.exit(17); response(record,{steering:[],followUp:[]}); if (mode === 'settle-on-clear') settled(false); break;
      case 'abort': settled(true); response(record); break;
      case 'extension_ui_response':
        assert.equal(record.id,dialog);
        assert.equal(typeof record.confirmed,'boolean');
        dialog=undefined;
        if (args.includes('--fake-command-file')) appendFileSync(value('--fake-command-file'), 'confirmed=' + record.confirmed + '\n');
        setTimeout(() => settled(false), 30);
        break;
      default: assert.fail('unsupported synthetic command');
    }
  }
});
process.stdin.on('end', () => process.exit(0));
