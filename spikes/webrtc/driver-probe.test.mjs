import test from 'node:test';
import assert from 'node:assert/strict';
import {patchDriver} from './driver-probe.mjs';
const turn=`        // 1.b turn_relayer poll_write()
        while let Some(msg) = self.turn_relayer.poll_write() {
            let four_tuple: FourTuple = FourTuple::from(&msg.transport);
            if let Err(err) = self.handle_write(msg).await {
                error!("preserve socket failure diagnostic: {}", err);
                if let Err(err) = self.turn_relayer.handle_event(RTCTurnRelayEventIn::SocketWriteFailure(four_tuple)) {
                    error!("preserve relay failure diagnostic: {}", err);
                }
            }
        }

`;
const original=`    async fn poll_writes(&mut self) -> Result<()> {
        // Keep STUN gathering unchanged.
${turn}        // 1.c peer_connection poll_write()
        let writes = Self::drain_core_writes(self.inner.clone()).await;
        self.flush_writes(writes).await;

        Ok(())
    }
`;
test('driver comparison retains early drain and all error handling, and drains again after core output',()=>{
  const result=patchDriver(original);
  assert.equal(result.split('self.flush_turn_writes().await;').length-1,2);
  assert.equal(result.split('async fn flush_turn_writes').length-1,1);
  assert(result.includes(turn.trimEnd()));
  assert(result.indexOf('self.flush_turn_writes().await;')<result.indexOf('drain_core_writes'));
  assert(result.includes('self.flush_writes(writes).await;\n        self.flush_turn_writes().await;'));
  assert(result.includes('// Keep STUN gathering unchanged.'));
  assert.throws(()=>patchDriver(result),'already patched input must not be patched again');
});
test('unexpected or ambiguous upstream layout is rejected rather than guessed',()=>{
  assert.throws(()=>patchDriver(''));
  assert.throws(()=>patchDriver(original+turn));
  assert.throws(()=>patchDriver(original.replace('        Ok(())','        Ok(42)')));
  assert.throws(()=>patchDriver(original+original));
});
