// Screenshot of the UI against a running devnet: node scripts/web-shot.mjs out.png [wait_s] [n]
import { chromium } from 'playwright-core';
const [out, wait = '40', n = '2'] = process.argv.slice(2);
const browser = await chromium.launch({ executablePath: process.env.CHROME || '/usr/bin/chromium', headless: true, args: ['--no-sandbox', '--use-gl=swiftshader', '--enable-unsafe-swiftshader'] });
const page = await browser.newPage({ viewport: { width: 1500, height: 1000 } });
page.on('pageerror', (e) => { if (!e.message.includes('closure invoked')) console.log('[pageerror]', e.message); });
await page.goto(`http://127.0.0.1:8080/?n=${n}`);
await page.waitForTimeout(Number(wait) * 1000);
await page.screenshot({ path: out, fullPage: true });
await browser.close();
