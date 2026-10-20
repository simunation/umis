// Browser end to end: two tabs (2 nodes + 1 node) join the devnet as zone
// members; every browser node mints at least one zone block, and all of them
// (and the bootstrap) agree on the finalized beacon hash.
import { expect, test, type Page } from '@playwright/test';

interface Snap {
  kind: string | null;
  zone: number | null;
  minted: number;
  finalized_slot: number | null;
  finalized_hash: string;
  head_slot: number | null;
}
type PageState = { ready: boolean; error?: string; nodes: { snap: Snap | null }[]; bootstrap: Snap | null };

test('browser nodes mint and agree on finality', async ({ browser }) => {
  const ctx = await browser.newContext();
  const tabs: Page[] = [];
  for (const n of [2, 1]) {
    const page = await ctx.newPage();
    page.on('pageerror', (e) => console.log('pageerror', e.message));
    await page.goto(`/?n=${n}`);
    tabs.push(page);
  }
  const state = async () => Promise.all(tabs.map((t) => t.evaluate(() => (window as unknown as { simu: PageState }).simu)));

  await expect
    .poll(
      async () => {
        const s = await state();
        const nodes = s.flatMap((x) => x.nodes);
        return nodes.length === 3 && nodes.every((n) => n.snap?.kind === 'Member' && n.snap.minted > 0);
      },
      { timeout: 200_000, intervals: [2000] },
    )
    .toBe(true);

  // Agreement: the same hash at the lowest finalized slot everybody reached.
  await expect
    .poll(
      async () => {
        const s = await state();
        const snaps = s.flatMap((x) => x.nodes.map((n: { snap: Snap | null }) => n.snap!)).concat(s[0].bootstrap ? [s[0].bootstrap] : []);
        const slots = new Set(snaps.map((x) => x.finalized_slot));
        const hashes = new Set(snaps.map((x) => x.finalized_hash));
        return slots.size === 1 && hashes.size === 1;
      },
      { timeout: 30_000, intervals: [500] },
    )
    .toBe(true);
  for (const s of await state()) expect(s.error).toBeUndefined();
});
