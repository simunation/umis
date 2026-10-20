import { chromium } from 'playwright-core';
const browser = await chromium.launch({ executablePath: '/usr/bin/chromium', headless: true, args: ['--no-sandbox'] });
const page = await browser.newPage();
page.on('console', (m) => { const t = m.text(); if (t.includes('WARN') && !t.includes('already been published')) console.log('[console]', t.replace(/%c/g, '').slice(0, 200)); });
await page.goto('http://127.0.0.1:8080/?n=5');
await new Promise((r) => setTimeout(r, Number(process.argv[2] ?? 45) * 1000));
const out = await page.evaluate(async () => {
  const res = [];
  for (const n of window.simuApi.nodes.list) {
    const st = await n.node.netStatus();
    const s = n.snap;
    res.push({ i: n.index, zone: s?.zone, kind: s?.kind, pool: s?.tx_kinds, minted: s?.minted, gin: s?.gossip_in?.['beacon/tx'], peers: st.peers.length });
  }
  return res;
});
for (const o of out) console.log(JSON.stringify(o));
await browser.close();
