import { chromium } from 'playwright-core';
const browser = await chromium.launch({ executablePath: '/usr/bin/chromium', headless: true, args: ['--no-sandbox'] });
const page = await browser.newPage();
page.on('console', (m) => { if (m.type() !== 'log') console.log('[console]', m.type(), m.text().slice(0, 200)); });
await page.goto('http://127.0.0.1:8080/?n=5');
let pick = null;
for (let t = 0; t < 200; t += 3) {
  await new Promise((r) => setTimeout(r, 3000));
  const s = await page.evaluate(() => window.simu);
  const b = s.bootstrap;
  if (!pick && s.nodes.length === 5 && s.nodes.every((n) => n.snap?.kind === 'Member')) {
    const byZone = new Map();
    for (const n of s.nodes) byZone.set(n.snap.zone, [...(byZone.get(n.snap.zone) ?? []), n.index]);
    for (const [zone, idx] of byZone) if (idx.length >= 2) pick = { founder: idx[0], zone };
    if (pick) {
      const p = s.nodes[0].snap.zones.find((z) => z.zone === pick.zone).point;
      const above = [p[0], p[1], p[2] + 1];
      const id = await page.evaluate(([f, a]) => window.simuApi.propose(f, [f], a, 16), [pick.founder, above]);
      console.log(`t=${t} proposed ${id} at ${above} by #${pick.founder}`);
    }
  }
  console.log(`t=${t} head ${b?.head_slot} zones ${b?.zones.length} proposals ${JSON.stringify(b?.proposals.map((p) => [p.status, p.tally]))} members ${JSON.stringify(s.nodes.map((n) => n.snap?.zone))}`);
  if (pick && b?.zones.length > 4) { const f = s.nodes.find((n) => n.index === pick.founder).snap; console.log('founder zone', f.zone, 'minted', f.minted); }
}
await browser.close();
