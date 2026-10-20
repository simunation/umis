// Browser nodes in this page: creation (keys persisted in localStorage as
// `simu.node.<n>`), auto join, and their status cards.

import init, { SimNode } from './pkg/umis_web.js';
import { Bootstrap, Snapshot, short } from './api';
import { provers } from './prover';

export interface PageNode {
  index: number;
  node: SimNode;
  snap: Snapshot | null;
  card: HTMLElement;
  autoJoin: boolean;
  lastFaucet: number;
  joinSent: number;
}

let wasmReady: Promise<unknown> | null = null;
export const loadWasm = () => (wasmReady ??= init());

const keyName = (genesis8: string, n: number) => `simu.node.${genesis8}.${n}`;

function store(): Storage | null {
  try {
    return window.localStorage;
  } catch {
    return null;
  }
}

export class Nodes {
  list: PageNode[] = [];

  constructor(
    private boot: Bootstrap,
    private genesisJson: string,
    private addrs: string[],
    private container: HTMLElement,
  ) {}

  /** Index for the next node: the first index of this page without a running node. */
  private nextIndex(genesis8: string): number {
    const used = new Set(this.list.map((n) => n.index));
    // Tabs share localStorage; claim an index no other tab is using right now.
    for (let i = 0; ; i++) {
      if (used.has(i)) continue;
      const lock = `simu.lock.${genesis8}.${i}`;
      const s = store();
      const t = Number(s?.getItem(lock) ?? 0);
      if (Date.now() - t < 15_000) continue;
      s?.setItem(lock, String(Date.now()));
      return i;
    }
  }

  async add(autoJoin: boolean): Promise<PageNode> {
    await loadWasm();
    const probe = new SimNode(this.genesisJson, undefined);
    const g8 = probe.genesis8;
    probe.free();
    const index = this.nextIndex(g8);
    const saved = store()?.getItem(keyName(g8, index)) ?? undefined;
    const node = new SimNode(this.genesisJson, saved);
    store()?.setItem(keyName(g8, index), node.secretHex);
    const card = document.createElement('div');
    card.className = 'card';
    this.container.appendChild(card);
    const pn: PageNode = { index, node, snap: null, card, autoJoin, lastFaucet: 0, joinSent: 0 };
    node.onEvent((ev: { kind: string }) => {
      if (ev.kind === 'Error') console.error('node', index, ev);
    });
    const params = new URLSearchParams(location.search);
    if (params.get('transport') === 'webrtc') node.preferWebrtc(true);
    // STARK jobs run in workers (`?prover=inline` proves on the page thread).
    if (params.get('prover') !== 'inline') {
      const secret = node.secretHex;
      node.setProver((job: Uint8Array) => provers().prove(this.genesisJson, secret, job));
    }
    node.start(this.addrs);
    this.list.push(pn);
    // keep the tab lock fresh
    setInterval(() => store()?.setItem(`simu.lock.${g8}.${index}`, String(Date.now())), 5000);
    return pn;
  }

  /** Polls every node, drives auto join, re-renders cards. */
  async tick(): Promise<void> {
    for (const pn of this.list) {
      if (!pn.node.ready) {
        pn.card.innerHTML = `<h3>${short(pn.node.peerId, 16)}…</h3><div class="muted">starting…</div>`;
        continue;
      }
      try {
        pn.snap = (await pn.node.status()) as Snapshot;
      } catch {
        continue;
      }
      if (pn.autoJoin) await this.autoJoin(pn);
      this.render(pn);
    }
  }

  private async autoJoin(pn: PageNode) {
    const s = pn.snap!;
    if (s.kind || s.head_slot === null) return;
    const now = Date.now();
    if (s.balance >= 200) {
      if (now - pn.joinSent > 12_000) {
        pn.node.join(200n);
        pn.joinSent = now;
      }
    } else if (now - pn.lastFaucet > 12_000) {
      pn.lastFaucet = now;
      await this.boot.faucet(s.node_id).catch(() => undefined);
    }
  }

  private render(pn: PageNode) {
    const s = pn.snap!;
    const myZone = s.zones.find((z) => z.zone === s.zone);
    const role =
      s.zone === null
        ? s.kind
          ? s.kind
          : 'not joined'
        : myZone?.my_rank === null || myZone?.my_rank === undefined
          ? `zone ${s.zone} validator`
          : `zone ${s.zone} minter rank ${myZone.my_rank}`;
    pn.card.innerHTML = `
      <h3 title="${pn.node.peerId}">#${pn.index} ${short(pn.node.peerId.slice(-12), 12)}</h3>
      <div class="row"><span class="role">${role}</span><span class="muted">${s.peers} peers</span></div>
      <div class="row"><span>beacon ${s.head_slot ?? '—'} / final ${s.finalized_slot ?? '—'}</span><span class="muted">${short(s.finalized_hash)}</span></div>
      <div class="row"><span>stake ${s.stake} · balance ${s.balance}</span><span>minted ${s.minted} · proved ${s.proved} · proposed ${s.proposed}</span></div>
      ${myZone?.tip ? `<div class="row muted"><span>zone tip slot ${myZone.tip[0]}</span><span>root ${myZone.tip[2]}</span></div>` : ''}
      <div class="actions">
        <button data-a="faucet" class="secondary">Get test stake</button>
        <button data-a="join" ${s.kind || s.balance < 200 ? 'disabled' : ''}>Join</button>
        <select data-a="policy"><option value="auto">votes: auto</option><option value="manual">votes: manual</option><option value="reject_all">votes: reject</option></select>
        <button data-a="stop" class="secondary">Stop</button>
      </div>`;
    const policy = pn.card.querySelector<HTMLSelectElement>('[data-a=policy]')!;
    policy.value = (pn as unknown as { policy?: string }).policy ?? 'auto';
    policy.onchange = () => {
      (pn as unknown as { policy?: string }).policy = policy.value;
      pn.node.setVotePolicy(policy.value);
    };
    pn.card.querySelector<HTMLButtonElement>('[data-a=faucet]')!.onclick = () => this.boot.faucet(s.node_id);
    pn.card.querySelector<HTMLButtonElement>('[data-a=join]')!.onclick = () => pn.node.join(200n);
    pn.card.querySelector<HTMLButtonElement>('[data-a=stop]')!.onclick = () => {
      pn.node.stop();
      pn.card.remove();
      this.list = this.list.filter((x) => x !== pn);
    };
  }
}
