// Lattice explorer: beacon slots × zones, coloured by commitment status,
// with click-through to the commitment and its proof BMT leaves.

import { LatticeView, short } from './api';

export function renderLattice(el: HTMLElement, l: LatticeView, zones: number[], details: HTMLElement) {
  const rows = [...l.blocks].reverse().slice(0, 14);
  const slashes = new Map(l.slashes.map((s) => [s.e, s]));
  const rollbacks = new Map<number, string[]>();
  for (const r of l.rollbacks) {
    const v = rollbacks.get(r.e_record) ?? [];
    v.push(`z${r.zone}:${r.reason}`);
    rollbacks.set(r.e_record, v);
  }
  let html = `<div class="legend">${['optimistic', 'proven', 'settled', 'invalid', 'pruned']
    .map((s) => `<span class="st-${s}">${s}</span>`)
    .join('')}</div><table><tr><th>beacon slot</th><th>block</th>${zones.map((z) => `<th>zone ${z}</th>`).join('')}<th>events</th></tr>`;
  for (const b of rows) {
    html += `<tr><td>${b.slot}</td><td title="${b.hash}">${short(b.hash, 6)}</td>`;
    for (const z of zones) {
      const c = b.commits.filter((x) => x.zone === z);
      html += c.length
        ? `<td class="c st-${c[0].status}" data-slot="${b.slot}" data-zone="${z}" title="commit for slot ${c[0].e}">${short(c[0].state_root, 6)}${c[0].proof_root ? ' ✓' : ''}</td>`
        : '<td></td>';
    }
    const ev: string[] = [];
    const s = slashes.get(b.slot);
    if (s) ev.push(`slash ${short(s.minter, 6)} (z${s.zone})`);
    for (const r of rollbacks.get(b.slot) ?? []) ev.push(`rollback ${r}`);
    if (b.txs.length) ev.push(`${b.txs.length} txs`);
    html += `<td class="muted">${ev.join(', ')}</td></tr>`;
  }
  el.innerHTML = html + '</table>';
  el.querySelectorAll<HTMLElement>('td.c').forEach((td) => {
    td.onclick = () => {
      const b = l.blocks.find((x) => x.slot === Number(td.dataset.slot))!;
      const c = b.commits.find((x) => x.zone === Number(td.dataset.zone))!;
      details.textContent = [
        `zone ${c.zone} commitment for beacon slot ${c.e} (in block ${b.slot})`,
        `status        ${c.status}`,
        `address       ${c.address}`,
        `state_root    ${c.state_root}`,
        `outputs_root  ${c.outputs_root}`,
        `inputs_root   ${c.inputs_root}`,
        `tip block     ${c.tip}`,
        `blocks        ${c.blocks}`,
        `proof BMT     ${c.proof_root ?? '— (proof follows later)'}`,
        `zones_root    ${b.zones_root}`,
        '',
        'proof leaves:',
        ...c.leaves.map((x) => `  slot ${x.from_slot}→${x.to_slot}  state ${short(x.state_root, 16)}  proof ${short(x.proof_address, 16)}`),
      ].join('\n');
    };
  });
}
