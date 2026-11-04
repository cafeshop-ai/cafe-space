import test from 'node:test';
import assert from 'node:assert/strict';
import {startLab} from './lab.mjs';

test('owned research daemon acknowledges explicit reset repeatedly and serves the same browser module',async()=>{
  const lab=await startLab();
  try {
    await lab.reset();await lab.reset();
    assert.deepEqual(lab.diagnostics(),[]);
    const response=await fetch(lab.url+'/browser.mjs');
    assert.equal(response.status,200);assert((await response.text()).includes("peer = 'rust'"));
    const rejected=await fetch(lab.url+'/offer',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({mode:'production'})});
    assert.equal(rejected.status,400);
    await lab.reset();
  } finally {await lab.stop();}
});
