import test from 'node:test';
import assert from 'node:assert/strict';
import {turnStats} from './turn-stats.mjs';

test('coturn lifecycle counts distinct sessions, waits for final close and redacts private fields',()=>{
  const prefix='session 000000000000000001: realm <PRIVATE_REALM> user <PRIVATE_USER>: incoming packet ';
  const log=prefix+'ALLOCATE processed, success\n'+prefix+'ALLOCATE processed, success\n'+prefix+'CREATE_PERMISSION processed, success\n'+prefix+'CHANNEL_BIND processed, success\n'+prefix+'ALLOCATE processed, error 486: PRIVATE_ERROR\n';
  assert.deepEqual(turnStats(log),{allocations:1,observedActiveAllocations:1,closedAllocations:0,permissionSuccesses:1,channelBindSuccesses:1,quotaErrors:1});
  const stage1=log+'session 000000000000000001: closed (1st stage), PRIVATE_ADDRESS\n';
  assert.equal(turnStats(stage1).observedActiveAllocations,1);
  const closed=turnStats(stage1+'session 000000000000000001: closed (2nd stage), PRIVATE_ADDRESS\n');
  assert.equal(closed.observedActiveAllocations,0);assert.equal(closed.closedAllocations,1);
  assert(!JSON.stringify(closed).includes('PRIVATE_'));
});
test('newer coturn suffix session IDs work; unknown allocation format is not reported as zero active',()=>{
  const log='realm <PRIVATE_REALM> user <PRIVATE_USER>: incoming packet ALLOCATE processed, success (session 123)\n';
  assert.equal(turnStats(log).observedActiveAllocations,1);
  assert.equal(turnStats(log+'closed (2nd stage), PRIVATE_ADDRESS (session 123)\n').observedActiveAllocations,0);
  assert.equal(turnStats('ALLOCATE processed, success\n').observedActiveAllocations,null);
  assert.equal(turnStats('ALLOCATE processed, success\n').allocations,null);
  assert.equal(turnStats('').observedActiveAllocations,0);
});
