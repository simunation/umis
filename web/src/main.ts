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
import { View3d } from './view3d';

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;
const params = new URLSearchParams(location.search);

interface PageState {
  ready: boolean;
  nodes: { index: number; nodeId: string; snap: Snapshot | null }[];
  bootstrap: Snapshot | null;
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
  const view = new View3d($('view3d'));
  view.nLoc = genesis.zones[0]?.params?.n_loc ?? 16;
  $<HTMLSelectElement>('channel').onchange = (e) => (view.channel = (e.target as HTMLSelectElement).value as View3d['channel']);

  const autoJoin = () => $<HTMLInputElement>('auto-join').checked;
  $('add-node').onclick = () => nodes.add(autoJoin());
  for (let i = 0; i < Number(params.get('n') ?? 0); i++) await nodes.add(true);

  // Create-zone dialog, opened by clicking a ghost block.
  const dialog = $<HTMLDialogElement>('create-zone');
  let target: [number, number, number] = [0, 0, 0];
  view.onGhostClick = (p) => {
    target = p;
    $('cz-point').textContent = `(${p.join(', ')}) water`;
    $('cz-founders').innerHTML =
      '<legend>founders (nodes in this page)</legend>' +
      nodes.list
        .filter((n) => n.snap?.kind === 'Member')
        .map((n) => `<label><input type="checkbox" value="${n.index}"> #${n.index} zone ${n.snap?.zone}</label><br>`)
        .join('');
    dialog.showModal();
  };
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
    $('details').textContent = `proposal ${id}\nat (${target.join(', ')}) water, S_new = ${slot}`;
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

function renderProposals(s: Snapshot, nodes: Nodes) {
  const el = $('proposals');
  if (!s.proposals.length) {
    el.innerHTML = '<div class="muted">none yet — click a ghost block in the 3D view</div>';
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
