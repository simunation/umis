// Generates bmt-js conformance vectors for the Rust port (umis-core::bmt, ::seg).
//
//   cargo run -q -p umis-core --example dump-seg-samples > scripts/seg-samples.json
//   node scripts/bmt-vectors.mjs scripts/seg-samples.json > crates/umis-core/tests/vectors/bmt-vectors.json
//
// Uses @nugaon/bmt-js@3.0.2 (scripts/package.json). Payloads follow a fixed
// byte pattern that the Rust test regenerates, so only sizes, addresses and
// proofs are stored.
import { readFileSync } from 'node:fs';
import { makeChunk, makeChunkedFile, fileInclusionProofBottomUp, makeSpan } from '@nugaon/bmt-js';

const hex = (u8) => Buffer.from(u8).toString('hex');
const fromHex = (s) => new Uint8Array(Buffer.from(s, 'hex'));

// Must match `pattern` in crates/umis-core/tests/bmt_conformance.rs.
function pattern(n) {
  const out = new Uint8Array(n);
  for (let i = 0; i < n; i++) out[i] = (i * 7 + (i >>> 8) * 13 + 5) & 0xff;
  return out;
}

const chunk = {
  addr_1_2_3: hex(makeChunk(new Uint8Array([1, 2, 3])).address()),
  addr_empty: hex(makeChunk(new Uint8Array()).address()),
  hello_world: [0, 64, 127].map((index) => ({
    index,
    sisters: makeChunk(new TextEncoder().encode('hello world')).inclusionProof(index).map(hex),
  })),
};

const span = [0, 1, 4096, 128 * 4096, 2 ** 32 - 1].map((value) => ({ value, hex: hex(makeSpan(value)) }));

const sizes = [
  0, 1, 31, 32, 33, 4095, 4096, 4097, 8192, 12345, 100000,
  128 * 4096, 128 * 4096 + 2, 129 * 4096 + 77, 300000, 1000003,
  128 * 128 * 4096 + 4096,
];
const files = [];
for (const size of sizes) {
  const t0 = Date.now();
  const payload = pattern(size);
  const file = makeChunkedFile(payload);
  const address = hex(file.address());
  const segs = Math.ceil(size / 32);
  const idx = new Set();
  if (segs > 0) {
    idx.add(0);
    idx.add(segs - 1);
    if (size <= 2_000_000) {
      idx.add(Math.floor(segs / 2));
      idx.add(Math.min(1, segs - 1));
      // first segment of the last leaf chunk (carrier-chunk edge case)
      idx.add(Math.floor((size - 1) / 4096) * 128);
    }
  }
  const proofs = [...idx].sort((a, b) => a - b).map((index) => {
    const seg = new Uint8Array(32);
    seg.set(payload.slice(index * 32, index * 32 + 32));
    return {
      index,
      segment: hex(seg),
      proof: fileInclusionProofBottomUp(file, index).map((p) => ({ span: hex(p.span), sisters: p.sisterSegments.map(hex) })),
    };
  });
  files.push({ size, address, proofs });
  console.error(`size ${size}: ${address.slice(0, 16)}… ${proofs.length} proofs in ${Date.now() - t0} ms`);
}

const seg = [];
const samplesPath = process.argv[2];
if (samplesPath) {
  for (const r of JSON.parse(readFileSync(samplesPath, 'utf8'))) {
    const bytes = fromHex(r.hex);
    const c = makeChunk(bytes);
    const segments = [...new Set(r.fields.map((f) => f.segment))];
    seg.push({
      record: r.record,
      hex: r.hex,
      address: hex(c.address()),
      span: hex(c.span()),
      proofs: segments.map((s) => ({ segment: s, sisters: c.inclusionProof(s).map(hex) })),
    });
  }
}

process.stdout.write(JSON.stringify({ generator: '@nugaon/bmt-js@3.0.2', chunk, span, files, seg }, null, 1));
