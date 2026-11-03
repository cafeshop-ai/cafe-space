// v0 pi-packages 3294487: 16 KiB packets, 16-byte PCD1 header, 256 KiB frames.
export const PACKET_BYTES = 16 * 1024;
export const HEADER_BYTES = 16;
export const MAX_FRAME_BYTES = 256 * 1024;
export function packet(id, raw, offset) {
  if (!Number.isInteger(id) || id < 1 || id > 0xffffffff || raw.length < 1 || raw.length > MAX_FRAME_BYTES || !Number.isInteger(offset) || offset < 0 || offset >= raw.length) throw Error("FRAME_BOUNDS");
  const body = raw.subarray(offset, offset + PACKET_BYTES - HEADER_BYTES);
  const out = new Uint8Array(HEADER_BYTES + body.length);
  const view = new DataView(out.buffer);
  [0x50434431, id, raw.length, offset].forEach((n, i) => view.setUint32(i * 4, n));
  out.set(body, HEADER_BYTES);
  return out;
}
export class Decoder {
  last = 0;
  pending = null;
  push(raw, now = performance.now()) {
    if (raw.byteLength <= HEADER_BYTES || raw.byteLength > PACKET_BYTES) throw Error("PACKET_SIZE");
    const view = new DataView(raw.buffer, raw.byteOffset, raw.byteLength);
    const [magic, id, total, offset] = [0, 4, 8, 12].map(at => view.getUint32(at));
    const body = raw.subarray(HEADER_BYTES);
    if (magic !== 0x50434431 || id === 0 || total < 1 || total > MAX_FRAME_BYTES || offset + body.length > total) throw Error("PACKET_BOUNDS");
    if (offset === 0) {
      if (this.pending || id !== (((this.last + 1) >>> 0) || 1)) throw Error("PACKET_REPLAY_OR_OVERLAP");
      this.pending = { id, total, offset: 0, bytes: new Uint8Array(total), deadline: now + 10000 };
    }
    const p = this.pending;
    if (!p || now > p.deadline || p.id !== id || p.total !== total || p.offset !== offset) throw Error("PACKET_SEQUENCE_OR_TIMEOUT");
    p.bytes.set(body, offset);
    p.offset += body.length;
    if (p.offset !== total) return null;
    this.pending = null;
    this.last = id;
    return p.bytes;
  }
}
