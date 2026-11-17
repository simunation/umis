// simunation web UI: browser nodes (several per tab), lattice explorer,
// 3D current view, zone creation and proposals.
//
// URL parameters: `boot=<bootstrap http url>` (default: this origin),
// `rv=<multiaddr>[,<multiaddr>]` (override the bootstrap/rendezvous
// multiaddrs), `n=<count>` (start that many nodes right away),
// `transport=webrtc` (dial native nodes over WebRTC-direct instead of WebSocket).

import { Bootstrap, LatticeView, RenderGrid, Snapshot, short } from './api';
import { renderLattice } from './lattice';
import { Nodes, loadWasm } from './nodes';
import { genesisId } from './pkg/umis_web.js';
import { View2d, ZoneTile } from './view2d';
import { View3d } from './view3d';

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;
const params = new URLSearchParams(location.search);

interface LifeZone {
  zone: number;
  point: [number, number, number];
  born: boolean;
  gen: number;
  k: number;
  status: string | null;
  /** Slot the shown state is at, and the zone's slot length in ticks. */
  nextSlot: number;
  slotTicks: number;
  rolledBack: boolean;
}
interface PageState {
  ready: boolean;
  nodes: { index: number; nodeId: string; snap: Snapshot | null }[];
  bootstrap: Snapshot | null;
  /** Life genesis only: every followed zone's generation and commit status (test hook). */
  life?: LifeZone[];
  error?: string;
}
declare global {
  interface Window {
    simu: PageState;
    simuApi: { nodes: Nodes; propose: (proposer: number, founders: number[], point: [number, number, number], slotTicks: number) => string };
  }
}
window.simu = { ready: false, nodes: [], bootstrap: null };

async function main() {
  const bootUrl = params.get('boot') ?? location.origin;
  $<HTMLInputElement>('boot').value = bootUrl;
  const boot = new Bootstrap(bootUrl);
  const [genesisJson, info] = await Promise.all([boot.genesis(), boot.info()]);
  const addrs = params.get('rv')?.split(',') ?? info.addrs;
  const genesis = JSON.parse(genesisJson);
  await loadWasm();
  const nodes = new Nodes(boot, genesisJson, addrs, $('nodes'));
  const layers: string[] = genesis.layers ?? ['water'];
  const life = layers[0] === 'life';
  $<HTMLSelectElement>('layer').innerHTML = layers.map((l) => `<option value="${l}">${l}</option>`).join('');
  document.querySelectorAll<HTMLElement>('.water-only').forEach((e) => (e.hidden = life));
  document.querySelectorAll<HTMLElement>('.life-only').forEach((e) => (e.hidden = !life));
  const view = life ? null : new View3d($('view3d'));
  const view2 = life ? new View2d($('view2d')) : null;
  if (view) {
    view.nLoc = genesis.zones[0]?.params?.n_loc ?? 16;
    $<HTMLSelectElement>('channel').onchange = (e) => (view.channel = (e.target as HTMLSelectElement).value as View3d['channel']);
  }
  if (view2) {
    view2.zoneSize = genesis.zones[0]?.params?.n ?? 64;
    $<HTMLInputElement>('trails').onchange = (e) => ((view2.showTrails = (e.target as HTMLInputElement).checked), view2.draw());
    $<HTMLInputElement>('labels').onchange = (e) => ((view2.showLabels = (e.target as HTMLInputElement).checked), view2.draw());
    $('fit').onclick = () => view2.fit();
  }

  const autoJoin = () => $<HTMLInputElement>('auto-join').checked;
  $('add-node').onclick = () => nodes.add(autoJoin());
  for (let i = 0; i < Number(params.get('n') ?? 0); i++) await nodes.add(true);

  // Create-zone dialog, opened by clicking a ghost block.
  const dialog = $<HTMLDialogElement>('create-zone');
  let target: [number, number, number] = [0, 0, 0];
  const openCreate = (p: [number, number, number]) => {
    target = p;
    $('cz-point').textContent = `(${p.join(', ')}) ${layers[0]}`;
    $('cz-founders').innerHTML =
      '<legend>founders (nodes in this page)</legend>' +
      nodes.list
        .filter((n) => n.snap?.kind === 'Member')
        .map((n) => `<label><input type="checkbox" value="${n.index}"> #${n.index} zone ${n.snap?.zone}</label><br>`)
        .join('');
    dialog.showModal();
  };
  if (view) view.onGhostClick = openCreate;
  if (view2) {
    view2.onGhostClick = openCreate;
    view2.onZoneClick = (z) => {
      const tile = lastTiles.find((t) => t.zone === z);
      const zv = window.simu.bootstrap?.zones.find((x) => x.zone === z);
      const m = (tile?.grid?.meta ?? {}) as Record<string, unknown>;
      $('details').textContent = [
        `life zone ${z} at (${zv?.point.join(', ')})`,
        `S_z           ${zv?.slot_ticks} ticks · ${zv?.members} members`,
        `generation    ${m.gen ?? '—'}  (k = ${m.k ?? '—'} per slot, born at ${m.birth ?? '—'})`,
        `population    ${m.population ?? '—'}`,
        `next slot     ${m.next_slot ?? '—'}`,
        `tip           ${zv?.tip ? `slot ${zv.tip[0]} · ${zv.tip[2]}` : '—'}`,
        `last commit   ${zv?.last_commit ? `slot ${zv.last_commit[0]} · ${zv.last_commit[2]}` : '—'}`,
        `status        ${tile?.status ?? '—'}${tile?.rolledBack ? ' (rolled back recently)' : ''}`,
      ].join('\n');
    };
  }
  $('cz-submit').onclick = () => {
    const picked = [...$('cz-founders').querySelectorAll<HTMLInputElement>('input:checked')].map((i) => Number(i.value));
    const founders = nodes.list.filter((n) => picked.includes(n.index));
    const proposer = founders[0] ?? nodes.list.find((n) => n.snap?.kind);
    if (!proposer || founders.length === 0) {
      alert('pick at least one founder (a joined node of this page)');
      return;
    }
    const slot = Number($<HTMLSelectElement>('cz-slot').value);
    const id = proposer.node.proposeZone(new Int32Array(target), slot, founders.map((f) => f.node.secretHex));
    $('details').textContent = `proposal ${id}\nat (${target.join(', ')}) ${layers[0]}, S_new = ${slot}`;
  };

  // Test hook (and console helper): propose a zone from nodes of this page.
  window.simuApi = {
    nodes,
    propose: (proposer, founders, point, slotTicks) => {
      const p = nodes.list.find((n) => n.index === proposer)!;
      const f = nodes.list.filter((n) => founders.includes(n.index)).map((n) => n.node.secretHex);
      return p.node.proposeZone(new Int32Array(point), slotTicks, f);
    },
  };

  let lattice: LatticeView | null = null;
  let lastTiles: ZoneTile[] = [];
  const refresh = async () => {
    await nodes.tick();
    try {
      const s = await boot.status();
      window.simu.bootstrap = s;
      $('genesis').textContent = `genesis ${genesisId(genesisJson)} · beacon ${s.head_slot ?? '—'} · final ${s.finalized_slot ?? '—'} · zones ${s.zones.length}`;
      const from = Math.max(0, (s.head_slot ?? 0) - 14);
      lattice = await boot.lattice(from, s.head_slot ?? 0);
      renderLattice($('lattice'), lattice, s.zones.map((z) => z.zone), $('details'));
      renderProposals(s, nodes);
      $('global').textContent = `${s.slashes} slashes · ${s.rollbacks} rollbacks`;
    } catch (e) {
      $('global').textContent = String(e);
    }
    window.simu.nodes = nodes.list.map((n) => ({ index: n.index, nodeId: n.node.nodeId, snap: n.snap }));
    window.simu.ready = true;
  };
  const redraw = async () => {
    const s = window.simu.bootstrap;
    if (!s) return;
    if (view2) {
      const zs = s.zones.filter((z) => z.layer === 'life');
      const grids = await Promise.all(zs.map((z) => boot.zone(z.zone).catch(() => null)));
      const status = new Map<number, string>();
      for (const b of lattice?.blocks ?? []) for (const c of b.commits) status.set(c.zone, c.status);
      const head = s.head_slot ?? 0;
      const rolled = new Set((lattice?.rollbacks ?? []).filter((r) => r.e_record + 3 >= head).map((r) => r.zone));
      lastTiles = zs.map((z, i) => ({ zone: z.zone, point: z.point, grid: grids[i], status: status.get(z.zone) ?? null, rolledBack: rolled.has(z.zone) }));
      view2.setGhosts(lattice?.free_points ?? []);
      view2.setTiles(lastTiles);
      renderLifeStats($('life-stats'), lastTiles);
      window.simu.life = lastTiles.map((t, i) => {
        const m = (t.grid?.meta ?? {}) as { born?: boolean; gen?: number; k?: number; next_slot?: number };
        return {
          zone: t.zone,
          point: t.point,
          born: !!m.born,
          gen: m.gen ?? 0,
          k: m.k ?? 0,
          status: t.status,
          nextSlot: m.next_slot ?? 0,
          slotTicks: zs[i].slot_ticks,
          rolledBack: t.rolledBack,
        };
      });
      return;
    }
    if (!view) return;
    const grids = (await Promise.all(s.zones.filter((z) => z.layer === $<HTMLSelectElement>('layer').value).map((z) => boot.zone(z.zone).catch(() => null)))).filter(
      (g): g is RenderGrid => !!g,
    );
    view.setZones(grids);
    if (lattice) view.setGhosts(lattice.free_points);
  };
  setInterval(refresh, 1500);
  setInterval(redraw, 2500);
  await refresh();
  await redraw();
}

/** Generation spread and the neighbour invariant (adjacent born zones are at
 * most k generations apart, see `task_life2d`). */
function renderLifeStats(el: HTMLElement, tiles: ZoneTile[]) {
  type M = { born: boolean; gen: number; k: number; population: number };
  const born = tiles.filter((t) => (t.grid?.meta as M | undefined)?.born);
  const meta = (t: ZoneTile) => t.grid!.meta as M;
  if (!born.length) {
    el.innerHTML = '<span class="muted">waiting for zone states…</span>';
    return;
  }
  const gens = born.map((t) => meta(t).gen);
  const k = meta(born[0]).k;
  let pairs = 0;
  let bad = 0;
  // Right after a rollback the bootstrap's zone views mix old and new
  // branches, so recently rolled-back zones are left out of the check.
  const settled = born.filter((t) => !t.rolledBack);
  for (const a of settled)
    for (const b of settled)
      if (a.zone < b.zone && Math.max(Math.abs(a.point[0] - b.point[0]), Math.abs(a.point[1] - b.point[1])) === 1) {
        pairs++;
        if (Math.abs(meta(a).gen - meta(b).gen) > k) bad++;
      }
  const pop = born.reduce((s, t) => s + meta(t).population, 0);
  const counts = new Map<string, number>();
  for (const t of tiles) counts.set(t.status ?? 'none', (counts.get(t.status ?? 'none') ?? 0) + 1);
  el.innerHTML = [
    `<span><b>gen</b> ${Math.min(...gens)}–${Math.max(...gens)}</span>`,
    `<span><b>k</b> ${k}</span>`,
    `<span><b>population</b> ${pop}</span>`,
    `<span class="${bad ? 'bad' : 'good'}" title="adjacent zones never more than k generations apart">${bad ? '✗' : '✓'} neighbours within k (${pairs} pairs)</span>`,
    ...[...counts].map(([s, n]) => `<span class="chip st-${s}">${n} ${s}</span>`),
  ].join('');
}

function renderProposals(s: Snapshot, nodes: Nodes) {
  const el = $('proposals');
  if (!s.proposals.length) {
    el.innerHTML = `<div class="muted">none yet — click a ${document.getElementById('view2d')?.hidden === false ? 'dashed tile in the board' : 'ghost block in the 3D view'}</div>`;
    return;
  }
  el.innerHTML = s.proposals
    .map(
      (p) => `<div class="card"><h3>${short(p.id, 12)} (${p.point.join(',')}) ${p.layer}</h3>
      <div>${p.status} · deadline ${p.deadline}</div>
      ${p.tally.map(([z, yes, no, n]) => `<div class="muted">zone ${z}: ${yes} yes / ${no} no of ${n}</div>`).join('')}
      ${p.status === 'Voting' ? `<div class="actions"><button data-id="${p.id}" data-v="1">Accept</button><button class="secondary" data-id="${p.id}" data-v="0">Reject</button></div>` : ''}
      </div>`,
    )
    .join('');
  el.querySelectorAll<HTMLButtonElement>('button[data-id]').forEach((b) => {
    b.onclick = () => {
      for (const n of nodes.list) if (n.snap?.kind === 'Member') n.node.vote(b.dataset.id!, b.dataset.v === '1');
    };
  });
}

main().catch((e) => {
  window.simu.error = String(e);
  document.body.insertAdjacentHTML('afterbegin', `<pre style="color:red">${String(e)}</pre>`);
});
