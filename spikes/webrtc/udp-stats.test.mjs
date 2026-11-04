import test from 'node:test';
import assert from 'node:assert/strict';
import {macReceiveBufferDrops} from './udp-stats.mjs';

test('Apple netstat full-socket counter has no datagram prefix',()=>{
  assert.equal(macReceiveBufferDrops('udp:\n\t\t0 dropped due to full socket buffers\n'),0);
  assert.equal(macReceiveBufferDrops('\t\t27 dropped due to full socket buffers\n'),27);
  assert.equal(macReceiveBufferDrops('  1 datagram dropped due to full socket buffers\n'),1);
  assert.equal(macReceiveBufferDrops('  3 datagrams dropped due to full socket buffers\n'),3);
});
test('unavailable kernel counters are never reported as zero',()=>{
  assert.throws(()=>macReceiveBufferDrops('statistics unavailable'),/expose/);
});
