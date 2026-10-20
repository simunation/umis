// M1 connectivity spike, end to end:
//   bootstrap (QUIC + WebRTC-direct + WS, relay, rendezvous) and
//   two browser tabs, one with 2 SpikeNodes and one with 1.
// Passes when every browser node received gossip from both other browser
// nodes. The bootstrap does not subscribe to the spike topic, so delivery
// proves browser↔browser paths through the relay.
//
//   node scripts/spike-e2e.mjs   (after scripts/build-spike.sh and cargo build -p umis-cli)
import { spawn } from 'node:child_process';
import { setTimeout as sleep } from 'node:timers/promises';
import { chromium } from 'playwright-core';
import { fileURLToPath } from 'node:url';
import path from 'node:path';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const http = process.env.SPIKE_HTTP || '127.0.0.1:8180';
const chrome = process.env.CHROME || '/usr/bin/chromium';

const boot = spawn(
  path.join(root, 'target/debug/umis-cli'),
  ['bootstrap', '--data-dir', path.join(root, '.devnet/spike-boot'), '--http', http, '--serve-web', path.join(root, 'web'),
    '--listen', '/ip4/127.0.0.1/udp/9190/quic-v1,/ip4/127.0.0.1/udp/9191/webrtc-direct,/ip4/127.0.0.1/tcp/9192/ws'],
  { stdio: ['ignore', 'inherit', 'inherit'], env: { ...process.env, RUST_LOG: process.env.RUST_LOG || 'warn' } },
);

let browser;
let ok = false;
try {
  let info;
  for (let i = 0; i < 100; i++) {
    try {
      info = await (await fetch(`http://${http}/bootstrap.json`)).json();
      if (info.addrs.length >= 3) break;
    } catch { /* not up yet */ }
    await sleep(100);
  }
  console.log('bootstrap', info.peer_id, info.addrs);

  browser = await chromium.launch({ executablePath: chrome, headless: true, args: ['--no-sandbox'] });
  const ctx = await browser.newContext();
  const tabs = [];
  for (const n of [2, 1]) {
    const page = await ctx.newPage();
    page.on('console', (m) => { if (m.type() === 'error') console.log(`[tab n=${n}]`, m.text()); });
    page.on('pageerror', (e) => console.log(`[tab n=${n}] pageerror`, e.message));
    await page.goto(`http://${http}/spike/index.html?n=${n}`);
    tabs.push(page);
  }

  const deadline = Date.now() + 90_000;
  let nodes = [];
  while (Date.now() < deadline) {
    nodes = [];
    for (const t of tabs) {
      const s = await t.evaluate(() => window.spike);
      if (s?.ready) nodes.push(...s.nodes);
    }
    const ids = nodes.map((x) => x.peerId);
    const done = nodes.length === 3 && nodes.every((x) => ids.filter((p) => p !== x.peerId).every((p) => (x.received[p] || 0) > 0));
    if (done) { ok = true; break; }
    await sleep(1000);
  }
  for (const x of nodes) {
    const conns = Object.entries(x.connections).map(([p, c]) => `${p.slice(-6)}${c.relayed ? '(relay)' : ''} ${c.addr.split('/').slice(0, 6).join('/')}`);
    console.log(`${x.peerId.slice(-6)} reservation=${x.reservation} registered=${x.registered}`);
    console.log(`   conns: ${conns.join(' | ')}`);
    if (process.env.SPIKE_LOG) console.log(x.log.join('\n'));
    console.log(`   received: ${JSON.stringify(Object.fromEntries(Object.entries(x.received).map(([p, c]) => [p.slice(-6), c])))}`);
  }
  const relayed = nodes.some((x) => Object.values(x.connections).some((c) => c.relayed));
  const webrtc = nodes.every((x) => Object.values(x.connections).some((c) => c.addr.includes('webrtc-direct')));
  console.log(`browser↔browser relayed connection: ${relayed}; browser→native over webrtc-direct: ${webrtc}`);
  ok = ok && relayed && webrtc;
} finally {
  if (browser) await browser.close();
  boot.kill('SIGINT');
}
console.log(ok ? 'SPIKE OK' : 'SPIKE FAILED');
process.exit(ok ? 0 : 1);
