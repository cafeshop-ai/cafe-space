import test from 'node:test';
import assert from 'node:assert/strict';
globalThis.window={};
const {transportStats,runCase,offerCandidateCounts}=await import('./browser.mjs');
delete globalThis.window;

test('offer diagnostics count mDNS candidates without returning SDP or addresses',()=>{
  const result=offerCandidateCounts([
    'a=ice-pwd:PRIVATE_SECRET',
    'a=candidate:1 1 udp 123 PRIVATE_NAME.local 10000 typ host',
    'a=candidate:2 1 udp 123 PRIVATE_NAME.LOCAL. 10001 typ host',
    'a=candidate:3 1 udp 123 192.0.2.1 10002 typ srflx',
    'a=candidate:4 1 udp 123 ::1 10003 typ host',
    'a=candidate:incomplete',
  ].join('\r\n'));
  assert.deepEqual(result,{total:5,mdns:2});
  assert(!JSON.stringify(result).includes('PRIVATE_'));
  assert.deepEqual(offerCandidateCounts(),{total:0,mdns:0});
});

test('transport diagnostics select the actual route and never return IDs, URLs or secrets',async()=>{
  const stats=new Map([
    ['t',{type:'transport',selectedCandidatePairId:'pair'}],
    ['pair',{type:'candidate-pair',state:'succeeded',localCandidateId:'local',remoteCandidateId:'remote',bytesSent:262416,bytesReceived:33,currentRoundTripTime:0.01}],
    ['local',{type:'local-candidate',candidateType:'relay',protocol:'udp',address:'PRIVATE_ADDRESS',url:'PRIVATE_URL'}],
    ['remote',{type:'remote-candidate',candidateType:'relay',protocol:'udp',address:'PRIVATE_ADDRESS'}],
    ['dc',{type:'data-channel',label:'PRIVATE_LABEL',dataChannelIdentifier:42,messagesSent:18,messagesReceived:1,bytesSent:262449,bytesReceived:33}],
  ]);
  const result=await transportStats({sctp:{maxMessageSize:16384},getStats:async()=>stats});
  assert.equal(result.localType,'relay');assert.equal(result.remoteType,'relay');assert.equal(result.protocol,'udp');
  assert.equal(result.maxMessageSize,16384);assert.equal(result.pair.bytesSent,262416);assert.equal(result.pair.responsesReceived,null);
  assert.equal(result.channels[0].messagesSent,18);assert(!JSON.stringify(result).includes('PRIVATE_'));
  assert.deepEqual(result.remoteCandidateTypes,{host:0,srflx:0,prflx:0,relay:1,unknown:0});
  assert.equal(result.candidatePairStates.succeeded,1);assert.equal(result.candidatePairStates.unknown,0);
  assert(!JSON.stringify(result).includes('localCandidateId'));
});
test('Firefox nominated-pair fallback keeps missing and nonnumeric counters unknown',async()=>{
  const stats=new Map([
    ['pair',{type:'candidate-pair',state:'succeeded',nominated:true,localCandidateId:'l',remoteCandidateId:'r',bytesSent:Infinity}],
    ['l',{candidateType:'host',protocol:'udp'}],['r',{type:'remote-candidate',candidateType:'host',protocol:'udp'}],
    ['unknown',{type:'remote-candidate',candidateType:'PRIVATE_UNKNOWN'}],
    ['pending',{type:'candidate-pair',state:'in-progress'}],
  ]);
  const result=await transportStats({getStats:async()=>stats});
  assert.equal(result.localType,'host');assert.equal(result.maxMessageSize,null);assert.equal(result.pair.bytesSent,null);
  assert.deepEqual(result.channels,[]);assert.equal(result.remoteCandidateTypes.host,1);
  assert.equal(result.remoteCandidateTypes.unknown,1);assert.equal(result.candidatePairStates['in-progress'],1);
  assert(!JSON.stringify(result).includes('PRIVATE_'));
});
test('all four peer-policy combinations prove the selected route and keep maximum frames within the send high-water',async()=>{
  const previousFetch=globalThis.fetch,previousPeer=globalThis.RTCPeerConnection;
  let peak=0,rustMode,wrongRoute=false;
  const channel={readyState:'open',bufferedAmount:0,close(){},send(bytes){
    if(this.failSend)throw Error('PRIVATE_SECRET');
    this.bufferedAmount+=bytes.byteLength;peak=Math.max(peak,this.bufferedAmount);
    setTimeout(()=>{this.bufferedAmount-=bytes.byteLength;this.onmessage({data:bytes.slice().buffer});},0);
  }};
  class Peer {
    iceGatheringState='complete';localDescription={type:'offer',sdp:'a=candidate:1 1 udp 123 PRIVATE_NAME.local 10000 typ host'};sctp={maxMessageSize:16384};
    constructor(config){this.localType=config.iceTransportPolicy==='relay'?'relay':'host';}
    addEventListener(){} createDataChannel(){return channel;} close(){}
    async createOffer(){return this.localDescription;} async setLocalDescription(){} async setRemoteDescription(){}
    async getStats(){return new Map([
      ['t',{type:'transport',selectedCandidatePairId:'p'}],['p',{localCandidateId:'l',remoteCandidateId:'r'}],
      ['l',{candidateType:this.localType,protocol:'udp'}],['r',{candidateType:wrongRoute?'relay':rustMode==='relay'?'relay':'host',protocol:'udp'}],
    ]);}
  }
  globalThis.RTCPeerConnection=Peer;
  globalThis.fetch=async(_url,options)=>{
    rustMode=JSON.parse(options.body).mode;
    return {ok:true,json:async()=>({answer:{type:'answer'},relayCandidates:rustMode==='relay'?1:0,gathered:true})};
  };
  try{
    for(const mode of ['direct','relay'])for(const remoteMode of ['direct','relay']){
      const result=await runCase({mode,rustMode:remoteMode});
      assert(result.ok,JSON.stringify(result));assert.equal(rustMode,remoteMode);
      assert.equal(result.localType,mode==='relay'?'relay':'host');assert.equal(result.remoteType,remoteMode==='relay'?'relay':'host');
      assert.equal(result.bytes,262144);assert.equal(result.packets,17);
      assert.deepEqual(result.localOffer,{total:1,mdns:1});assert(!JSON.stringify(result).includes('PRIVATE_'));
      assert.equal(result.sha256,'31a1f9dea0169551092d05e8bf4a446228c8c3eb4c9b713c66adcb7fd53c89be');
      assert(peak<=64*1024,'queued packet headers also count: '+peak);
    }
    wrongRoute=true;
    const failed=await runCase({mode:'relay',rustMode:'direct'});
    assert.equal(failed.reason,'NOT_DIRECT');assert.deepEqual(failed.localOffer,{total:1,mdns:1});
    assert.equal(failed.rustRelayCandidates,0);assert.equal(failed.rustGathered,true);
    assert(!JSON.stringify(failed).includes('PRIVATE_'));
    channel.failSend=true;
    const sendFailed=await runCase({mode:'direct'});
    assert.equal(sendFailed.reason,'DATA_CHANNEL_SEND_FAILED');assert(!JSON.stringify(sendFailed).includes('PRIVATE_'));
    assert.equal((await runCase({mode:'invalid'})).reason,'INVALID_MODE');
  }finally{
    globalThis.fetch=previousFetch;
    if(previousPeer===undefined)delete globalThis.RTCPeerConnection;else globalThis.RTCPeerConnection=previousPeer;
  }
});
