// Proving workers shared by the page's nodes. Workers start on the first
// job (mock genesis: never) and grow up to half the cores, at most 4.

import type { ProveReply, ProveRequest } from './prover.worker';

class ProverWorker {
  busy = 0;
  private next = 1;
  private pending = new Map<number, { resolve: (p: Uint8Array) => void; reject: (e: Error) => void }>();
  private worker = new Worker(new URL('./prover.worker.ts', import.meta.url), { type: 'module' });

  constructor() {
    this.worker.onmessage = (ev: MessageEvent<ProveReply>) => {
      const p = this.pending.get(ev.data.id);
      if (!p) return;
      this.pending.delete(ev.data.id);
      this.busy--;
      if ('proof' in ev.data) p.resolve(ev.data.proof);
      else p.reject(new Error(ev.data.error));
    };
  }

  prove(genesis: string, secret: string, job: Uint8Array): Promise<Uint8Array> {
    const id = this.next++;
    this.busy++;
    return new Promise((resolve, reject) => {
      this.pending.set(id, { resolve, reject });
      const req: ProveRequest = { id, genesis, secret, job };
      this.worker.postMessage(req, [job.buffer]);
    });
  }
}

export class ProverPool {
  private workers: ProverWorker[] = [];
  private max = Math.max(1, Math.min(4, Math.floor((navigator.hardwareConcurrency ?? 2) / 2)));

  prove(genesis: string, secret: string, job: Uint8Array): Promise<Uint8Array> {
    let w = this.workers.find((x) => x.busy === 0);
    if (!w && this.workers.length < this.max) {
      w = new ProverWorker();
      this.workers.push(w);
    }
    w ??= this.workers.reduce((a, b) => (b.busy < a.busy ? b : a));
    return w.prove(genesis, secret, job);
  }
}

let pool: ProverPool | null = null;
export const provers = () => (pool ??= new ProverPool());
