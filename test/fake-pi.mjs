// Offline fixture only. Does not import Pi, read credentials, use tools or call a model/network.
import assert from 'node:assert/strict';

const args = process.argv.slice(2);
const value = flag => args[args.indexOf(flag) + 1];
for (const flag of ['--mode', '--offline', '--no-extensions', '--no-approve', '--no-themes', '--session-dir', '--session-id']) assert(args.includes(flag));
assert.equal(value('--mode'), 'rpc');
assert.equal(process.env.CAFE_SPACE_TEST_SECRET, undefined);
const nativeId = value('--session-id');
const mode = args.includes('--fake-mode') ? value('--fake-mode') : 'normal';
if (mode === 'no-read') setInterval(() => {}, 1000);
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
    switch (record.type) {
      case 'get_state':
        response(record, {sessionId:nativeId, sessionFile:'/private/must-not-forward.jsonl', isStreaming:false, isCompacting:false, pendingMessageCount:0});
        if (mode === 'no-read') process.stdin.pause();
        break;
      case 'prompt':
        if (mode === 'stderr-flood') process.stderr.write('synthetic-private-diagnostics'.repeat(100000));
        if (mode === 'no-response') break;
        if (mode === 'crash') process.exit(17);
        if (mode === 'malformed') { process.stdout.write('{synthetic invalid record}\n'); break; }
        if (record.message === '/handled') {response(record, {disposition:'handled'}); break;}
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
        if (record.message === 'approval') {
          dialog = 'private-confirm';
          emit({type:'extension_ui_request', id:dialog, method:'confirm', title:'Synthetic confirmation'});
          break;
        }
        emit({type:'message_start', message:{role:'assistant',content:[]}});
        emit({type:'message_update', assistantMessageEvent:{type:'text_start',contentIndex:0}});
        emit({type:'message_update', assistantMessageEvent:{type:'text_delta',contentIndex:0,delta:'Tea 茶\u2028second\u2029third'}});
        emit({type:'message_end', message:{role:'assistant',content:[{type:'text',text:'Final synthetic 茶'}],stopReason:'stop'}});
        emit({type:'agent_end',willRetry:true,messages:[]});
        emit({type:'compaction_start',reason:'overflow'});
        settled(false);
        break;
      case 'steer': delayedSteer = record; break;
      case 'follow_up':
        response(record,{disposition:'queued'});
        if (delayedSteer) {response(delayedSteer,{disposition:'queued'}); delayedSteer=undefined;}
        break;
      case 'clear_queue': response(record,{steering:[],followUp:[]}); break;
      case 'abort': settled(true); response(record); break;
      case 'extension_ui_response':
        assert.equal(record.id,dialog);
        assert.equal(typeof record.confirmed,'boolean');
        dialog=undefined;
        settled(false);
        break;
      default: assert.fail('unsupported synthetic command');
    }
  }
});
process.stdin.on('end', () => process.exit(0));
