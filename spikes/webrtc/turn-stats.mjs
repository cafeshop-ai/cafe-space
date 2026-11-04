// Private coturn logs stay local; only aggregate lifecycle/method counters leave this module.
export async function observeTurnRelease(snapshot,ms) {
  const started=performance.now();
  let turn=await snapshot();
  while(turn.observedActiveAllocations!==0 && performance.now()-started<ms) {
    await new Promise(resolve=>setTimeout(resolve,Math.min(250,Math.max(0,ms-(performance.now()-started)))));
    turn=await snapshot();
  }
  return {released:turn.observedActiveAllocations===0,elapsedMs:Math.round(performance.now()-started),turn};
}
export function turnStats(log) {
  const active=new Set(),allocated=new Set();
  let closedAllocations=0,permissionSuccesses=0,channelBindSuccesses=0,quotaErrors=0,unparsedAllocations=0;
  for(const line of log.split(/\r?\n/)) {
    const id=line.match(/\bsession (\d+):/)?.[1] ?? line.match(/\(session (\d+)\)/)?.[1];
    if(/\bALLOCATE processed,\s*success\b/.test(line)) {
      if(id){active.add(id);allocated.add(id);}else unparsedAllocations++;
    }
    if(id && /closed \(2nd stage\)/.test(line) && active.delete(id))closedAllocations++;
    if(/\bCREATE_PERMISSION processed,\s*success\b/.test(line))permissionSuccesses++;
    if(/\bCHANNEL_BIND processed,\s*success\b/.test(line))channelBindSuccesses++;
    if(/processed,\s*error 486\b/.test(line))quotaErrors++;
  }
  return {allocations:unparsedAllocations?null:allocated.size,observedActiveAllocations:unparsedAllocations?null:active.size,closedAllocations,permissionSuccesses,channelBindSuccesses,quotaErrors};
}
