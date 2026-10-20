// Opens the UI against a running devnet and prints browser node states.
//   node scripts/web-debug.mjs [seconds=90] [n=2]
import { chromium } from 'playwright-core';
const secs = Number(process.argv[2] ?? 90);
const n = Number(process.argv[3] ?? 2);
const browser = await chromium.launch({ executablePath: process.env.CHROME || '/usr/bin/chromium', headless: true, args: ['--no-sandbox'] });
const page = await browser.newPage();
page.on('console', (m) => { const t = m.text(); if (['error', 'warning'].includes(m.type()) || t.includes('WARN') || (process.env.DBG && t.includes(process.env.DBG))) console.log('[console]', m.type(), t.slice(0, 300)); });
page.on('pageerror', (e) => console.log('[pageerror]', e.message));
await page.goto(`http://127.0.0.1:8080/?n=${n}`);
for (let t = 0; t < secs; t += 5) {
  await new Promise((r) => setTimeout(r, 5000));
  const s = await page.evaluate(() => window.simu);
  const b = s.bootstrap;
  console.log(`t=${t + 5}s boot head ${b?.head_slot} fin ${b?.finalized_slot} err=${s.error ?? ''}`);
  for (const x of s.nodes) {
    const q = x.snap;
    console.log(`   #${x.index} kind=${q?.kind} zone=${q?.zone} head=${q?.head_slot} fin=${q?.finalized_slot} peers=${q?.peers} bal=${q?.balance} stake=${q?.stake} minted=${q?.minted} pool=${q?.tx_pool} acc=${q?.zone_blocks_accepted} inv=${q?.invalid_blocks} rejB=${q?.rejected_beacon} sync=${JSON.stringify(q?.syncing)} req=${q?.pending_requests} tip=${JSON.stringify(q?.zones?.find(z=>z.zone===q?.zone)?.tip)} rank=${q?.zones?.find(z=>z.zone===q?.zone)?.my_rank}`);
  }
}
await browser.close();
