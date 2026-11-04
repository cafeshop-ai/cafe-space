import test from 'node:test';
import assert from 'node:assert/strict';
import {startLab} from './lab.mjs';

test('owned research daemon acknowledges explicit reset repeatedly and serves the same browser module',async()=>{
  let censusReply={available:true,sessions:2,private:'PRIVATE_SECRET'};
  const lab=await startLab({turnCensus:async()=>censusReply});
  try {
    await lab.reset();await lab.reset();
    assert.deepEqual(lab.diagnostics(),[]);
    const snapshot=await lab.snapshot();
    assert.equal(snapshot.dataChunksReceived,0);assert.equal(snapshot.sacksBuilt,0);
    assert.equal(snapshot.lastLocalWindow,null);
    assert.deepEqual(await lab.snapshot(),snapshot);
    const response=await fetch(lab.url+'/browser.mjs');
    assert.equal(response.status,200);assert((await response.text()).includes("peer = 'rust'"));
    assert.deepEqual(await (await fetch(lab.url+'/turn-census')).json(),{available:true,sessions:2});
    for(const value of [{available:true,sessions:-1},{available:true,sessions:'2'},null]) {
      censusReply=value;assert.deepEqual(await (await fetch(lab.url+'/turn-census')).json(),{available:false});
    }
    const foreign=await fetch(lab.url+'/turn-census',{headers:{Origin:'https://example.invalid'}});
    assert.equal(foreign.status,403);
    const rejected=await fetch(lab.url+'/offer',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({mode:'production'})});
    assert.equal(rejected.status,400);
    await lab.reset();
  } finally {await lab.stop();}
});
