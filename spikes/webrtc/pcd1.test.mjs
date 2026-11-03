import test from 'node:test';
import assert from 'node:assert/strict';
import { Decoder, packet, PACKET_BYTES, HEADER_BYTES, MAX_FRAME_BYTES } from './pcd1.mjs';
test('v0 golden packet, maximum frame, replay and ID wrap', () => {
  assert.deepEqual([...packet(1, new TextEncoder().encode('ab'), 0)], [80,67,68,49,0,0,0,1,0,0,0,2,0,0,0,0,97,98]);
  const raw = Uint8Array.from({length:MAX_FRAME_BYTES},(_,i)=>i%251);
  const d = new Decoder(); let result;
  for(let at=0;at<raw.length;at+=PACKET_BYTES-HEADER_BYTES) result=d.push(packet(1,raw,at),0);
  assert.deepEqual(result,raw);
  assert.throws(()=>d.push(packet(1,raw,0),0));
  d.last=0xffffffff;
  assert.deepEqual(d.push(packet(1,new Uint8Array([1]),0),0),new Uint8Array([1]));
});
test('bounds, wrong order, overlap and deadline are rejected', () => {
  for (const raw of [new Uint8Array(),new Uint8Array(16),new Uint8Array(PACKET_BYTES+1)]) assert.throws(()=>new Decoder().push(raw,0));
  assert.throws(()=>packet(0,new Uint8Array([1]),0));
  assert.throws(()=>packet(1,new Uint8Array(MAX_FRAME_BYTES+1),0));
  const raw=new Uint8Array(PACKET_BYTES), first=packet(1,raw,0), second=packet(1,raw,PACKET_BYTES-HEADER_BYTES);
  assert.throws(()=>new Decoder().push(second,0));
  const d=new Decoder(); d.push(first,0);
  assert.throws(()=>d.push(first,0));
  assert.throws(()=>d.push(second,10001));
  new DataView(first.buffer).setUint32(8,MAX_FRAME_BYTES+1);
  assert.throws(()=>new Decoder().push(first,0));
});
