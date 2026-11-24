// Proving worker: loads the wasm module and proves the page nodes'
// `ProveJob`s (borsh bytes in, proof bytes out). Workers never receive a node's
// secret key: STARK proofs do not depend on it.

import init, { proveJob } from './pkg/umis_web.js';

export interface ProveRequest {
  id: number;
  genesis: string;
  job: Uint8Array;
}

export type ProveReply = { id: number; proof: Uint8Array } | { id: number; error: string };

const ready = init();
const post = (self as unknown as { postMessage(m: ProveReply, t?: Transferable[]): void }).postMessage.bind(self);

self.onmessage = async (ev: MessageEvent<ProveRequest>) => {
  const { id, genesis, job } = ev.data;
  await ready;
  try {
    const proof = proveJob(genesis, job);
    post({ id, proof }, [proof.buffer]);
  } catch (e) {
    post({ id, error: String(e) });
  }
};
