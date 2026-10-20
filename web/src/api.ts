// Bootstrap HTTP API and the JSON shapes shared with the Rust side.

export interface ZoneView {
  zone: number;
  point: [number, number, number];
  layer: string;
  slot_ticks: number;
  members: number;
  following: boolean;
  tip: [number, string, string] | null;
  last_commit: [number, string, string] | null;
  my_rank: number | null;
}

export interface ProposalView {
  id: string;
  point: [number, number, number];
  layer: string;
  status: string;
  tally: [number, number, number, number][];
  deadline: number;
}

export interface Snapshot {
  node_id: string;
  now_slot: number | null;
  head_slot: number | null;
  head_hash: string;
  head_height: number;
  finalized_slot: number | null;
  finalized_hash: string;
  kind: string | null;
  zone: number | null;
  stake: number;
  balance: number;
  peers: number;
  zones: ZoneView[];
  proposals: ProposalView[];
  slashes: number;
  rollbacks: number;
  minted: number;
  proved: number;
  proposed: number;
  faults_reported: number;
  tx_pool: number;
}

export interface LeafView { from_slot: number; to_slot: number; state_root: string; proof_address: string }
export interface CommitView {
  zone: number; e: number; address: string; state_root: string; outputs_root: string; inputs_root: string;
  tip: string; blocks: number; status: string; proof_root: string | null; leaves: LeafView[];
}
export interface BeaconView { slot: number; hash: string; proposer: string; txs: string[]; commits: CommitView[]; zones_root: string; proofs_root: string }
export interface LatticeView {
  blocks: BeaconView[];
  slashes: { e: number; minter: string; zone: number; zone_slot: number; kind: number; slashed: number }[];
  rollbacks: { zone: number; checkpoint_e: number | null; j_r: number; e_record: number; reason: string }[];
  free_points: [number, number, number][];
  bounds: [number, number, number];
}

export interface RenderGrid { dims: [number, number, number]; origin: [number, number, number]; channels: number; data: number[] | Float32Array }

export class Bootstrap {
  constructor(public base: string) {}

  async json<T>(path: string, init?: RequestInit): Promise<T> {
    const r = await fetch(this.base + path, init);
    if (!r.ok) throw new Error(`${path}: ${r.status}`);
    return r.json() as Promise<T>;
  }

  genesis(): Promise<string> {
    return fetch(this.base + '/genesis.json').then((r) => r.text());
  }

  info(): Promise<{ peer_id: string; addrs: string[] }> {
    return this.json('/bootstrap.json');
  }

  status(): Promise<Snapshot> {
    return this.json('/status');
  }

  lattice(from: number, to: number): Promise<LatticeView> {
    return this.json(`/lattice?from=${from}&to=${to}`);
  }

  zone(z: number): Promise<RenderGrid | null> {
    return this.json(`/zone/${z}`);
  }

  faucet(nodeId: string): Promise<{ ok: boolean; error?: string }> {
    return this.json('/faucet', {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ node_id: nodeId }),
    });
  }
}

export const short = (h: string | null | undefined, n = 8) => (h ? h.slice(0, n) : '—');
