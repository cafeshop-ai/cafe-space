import test from 'node:test';
import assert from 'node:assert/strict';
globalThis.window={};
const {transportStats}=await import('./browser.mjs');
delete globalThis.window;

test('transport diagnostics select the actual route and never return IDs, URLs or secrets',async()=>{
  const stats=new Map([
    ['t',{type:'transport',selectedCandidatePairId:'pair'}],
    ['pair',{type:'candidate-pair',localCandidateId:'local',remoteCandidateId:'remote',bytesSent:262416,bytesReceived:33,currentRoundTripTime:0.01}],
    ['local',{type:'local-candidate',candidateType:'relay',protocol:'udp',address:'PRIVATE_ADDRESS',url:'PRIVATE_URL'}],
    ['remote',{type:'remote-candidate',candidateType:'relay',protocol:'udp',address:'PRIVATE_ADDRESS'}],
    ['dc',{type:'data-channel',label:'PRIVATE_LABEL',dataChannelIdentifier:42,messagesSent:18,messagesReceived:1,bytesSent:262449,bytesReceived:33}],
  ]);
  const result=await transportStats({sctp:{maxMessageSize:16384},getStats:async()=>stats});
  assert.equal(result.localType,'relay');assert.equal(result.remoteType,'relay');assert.equal(result.protocol,'udp');
  assert.equal(result.maxMessageSize,16384);assert.equal(result.pair.bytesSent,262416);assert.equal(result.pair.responsesReceived,null);
  assert.equal(result.channels[0].messagesSent,18);assert(!JSON.stringify(result).includes('PRIVATE_'));
  assert(!JSON.stringify(result).includes('localCandidateId'));
});
test('Firefox nominated-pair fallback keeps missing and nonnumeric counters unknown',async()=>{
  const stats=new Map([
    ['pair',{type:'candidate-pair',state:'succeeded',nominated:true,localCandidateId:'l',remoteCandidateId:'r',bytesSent:Infinity}],
    ['l',{candidateType:'host',protocol:'udp'}],['r',{candidateType:'host',protocol:'udp'}],
  ]);
  const result=await transportStats({getStats:async()=>stats});
  assert.equal(result.localType,'host');assert.equal(result.maxMessageSize,null);assert.equal(result.pair.bytesSent,null);
  assert.deepEqual(result.channels,[]);
});
