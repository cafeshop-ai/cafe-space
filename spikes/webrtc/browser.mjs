import { Decoder, packet, MAX_FRAME_BYTES, PACKET_BYTES, HEADER_BYTES } from './pcd1.mjs';
const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));
async function until(check, ms, failure) {
  const deadline = performance.now() + ms;
  while (!check()) {
    if (performance.now() >= deadline) throw Error(failure);
    await sleep(10);
  }
}
export async function transportStats(pc) {
  const stats=await pc.getStats();
  let pair;
  for(const entry of stats.values())if(entry.type==='transport' && entry.selectedCandidatePairId)pair=stats.get(entry.selectedCandidatePairId);
  if(!pair)for(const entry of stats.values())if(entry.type==='candidate-pair' && entry.state==='succeeded' && (entry.nominated || entry.selected))pair=entry;
  const local=pair && stats.get(pair.localCandidateId),remote=pair && stats.get(pair.remoteCandidateId);
  const numbers=(value,keys)=>Object.fromEntries(keys.map(key=>[key,Number.isFinite(value?.[key])?value[key]:null]));
  return {maxMessageSize:pc.sctp?.maxMessageSize ?? null,localType:local?.candidateType,remoteType:remote?.candidateType,protocol:local?.protocol,remoteProtocol:remote?.protocol,pair:numbers(pair,['bytesSent','bytesReceived','requestsSent','responsesReceived','currentRoundTripTime']),channels:[...stats.values()].filter(value=>value.type==='data-channel').map(value=>numbers(value,['messagesSent','messagesReceived','bytesSent','bytesReceived']))};
}
export async function runCase({mode, servers = [], blocked = false}) {
  let phase = 'setup';
  const pc = new RTCPeerConnection({iceServers: servers, iceTransportPolicy: mode === 'relay' ? 'relay' : 'all'});
  const candidateTypes={host:0,srflx:0,prflx:0,relay:0}, iceErrors=new Set();
  pc.addEventListener('icecandidate',event=>{if(event.candidate && Object.hasOwn(candidateTypes,event.candidate.type))candidateTypes[event.candidate.type]++;});
  pc.addEventListener('icecandidateerror',event=>{if(Number.isInteger(event.errorCode))iceErrors.add(event.errorCode);});
  const dc = pc.createDataChannel('cafe-r4', {ordered: true});
  dc.binaryType = 'arraybuffer';
  const decoder = new Decoder(), replies = [];
  let decodeFailed = false;
  dc.onmessage = event => {
    try { const result = decoder.push(new Uint8Array(event.data)); if (result) replies.push(result); }
    catch { decodeFailed = true; }
  };
  try {
    phase = 'browser_gather';
    await pc.setLocalDescription(await pc.createOffer());
    await until(() => pc.iceGatheringState === 'complete', 15000, 'BROWSER_GATHER_TIMEOUT');
    phase = 'rust_answer';
    const response = await fetch('/offer', {method:'POST', headers:{'Content-Type':'application/json'},body:JSON.stringify({mode, offer:pc.localDescription})});
    if (!response.ok) throw Error('ANSWER_FAILED');
    const answer = await response.json();
    await pc.setRemoteDescription(answer.answer);
    phase = 'channel_open';
    try { await until(() => dc.readyState === 'open', 20000, 'OPEN_TIMEOUT'); }
    catch (error) {
      if (blocked && answer.relayCandidates === 0) return {ok:true,blocked:true,opened:false,rustRelayCandidates:0,rustGathered:answer.gathered,iceState:pc.iceConnectionState};
      throw error;
    }
    if (blocked) throw Error('UDP_BLOCK_DID_NOT_BLOCK');
    phase = 'maximum_frame_echo';
    const frames = [new TextEncoder().encode('中文 café 😀'),Uint8Array.from({length:MAX_FRAME_BYTES},(_,i)=>i%251)];
    for (let index=0;index<frames.length;index++) {
      phase=index===0?'unicode_echo':'maximum_frame_echo';
      const raw = frames[index];
      for (let offset=0;offset<raw.length;offset+=PACKET_BYTES-HEADER_BYTES) {
        const bytes=packet(index+1,raw,offset);
        await until(() => dc.bufferedAmount+bytes.byteLength <= 4*PACKET_BYTES,5000,'SEND_BUDGET');
        dc.send(bytes);
      }
      await until(() => decodeFailed || replies.length > index,10000,'ECHO_TIMEOUT');
      if (decodeFailed || replies[index].length !== raw.length || !raw.every((byte,at)=>byte===replies[index][at])) throw Error('ECHO_MISMATCH');
    }
    phase = 'selected_pair';
    const route=await transportStats(pc);
    if (!route.localType || !route.remoteType || route.protocol !== 'udp' || route.remoteProtocol !== 'udp') throw Error('NO_SELECTED_UDP_PAIR');
    if (mode==='relay' && (route.localType!=='relay' || route.remoteType!=='relay' || answer.relayCandidates < 1)) throw Error('RELAY_NOT_PROVEN');
    if (mode==='direct' && (route.localType==='relay' || route.remoteType==='relay')) throw Error('NOT_DIRECT');
    const hash = [...new Uint8Array(await crypto.subtle.digest('SHA-256',replies[1]))].map(byte=>byte.toString(16).padStart(2,'0')).join('');
    return {ok:true,mode,bytes:replies[1].length,packets:Math.ceil(MAX_FRAME_BYTES/(PACKET_BYTES-HEADER_BYTES)),sha256:hash,unicodeEcho:true,...route,rustRelayCandidates:answer.relayCandidates,rustGathered:answer.gathered};
  } catch (error) {
    let route;try{route=await transportStats(pc);}catch{route={statsAvailable:false};}
    return {ok:false,phase,reason:error instanceof Error ? error.message.replace(/[^A-Z_]/g,'').slice(0,64) : 'CASE_FAILED',browserOnline:navigator.onLine,gatherState:pc.iceGatheringState,channelState:dc.readyState,bufferedAmount:dc.bufferedAmount,candidateTypes,iceErrorCodes:[...iceErrors],...route};
  } finally { dc.close(); pc.close(); }
}
window.runR4Case = runCase;
