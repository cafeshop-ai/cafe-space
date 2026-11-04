import assert from 'node:assert/strict';

export function macReceiveBufferDrops(output){
  const match=output.match(/^\s*(\d+)\s+(?:datagrams?\s+)?dropped due to full socket buffers\s*$/m);
  assert(match,'kernel must expose the receive-buffer drop counter');
  return Number(match[1]);
}
