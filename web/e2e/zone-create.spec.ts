// Zone creation from the browser: five page nodes join; a member of a zone
// with two members founds a new zone on the free point above it; the
// adjacent zone's members (auto policy) accept; the zone activates and its
// founder mints there.
import { expect, test } from '@playwright/test';

interface Snap {
  kind: string | null;
  zone: number | null;
  minted: number;
  zones: { zone: number; point: [number, number, number]; members: number; last_commit: unknown }[];
  proposals: { id: string; status: string }[];
}
type PageState = { ready: boolean; nodes: { index: number; snap: Snap | null }[] };

test('a browser-founded zone activates and runs', async ({ page }) => {
  page.on('pageerror', (e) => { if (!e.message.includes('closure invoked')) console.log('pageerror', e.message); });
  await page.goto('/?n=5');
  const state = () => page.evaluate(() => (window as unknown as { simu: PageState }).simu);
  // wait until every node is a member, then pick a zone with two of them
  let pick: { founder: number; zone: number } | null = null;
  await expect
    .poll(
      async () => {
        const s = await state();
        if (s.nodes.length < 5 || !s.nodes.every((n) => n.snap?.kind === 'Member')) return false;
        const byZone = new Map<number, number[]>();
        for (const n of s.nodes) byZone.set(n.snap!.zone!, [...(byZone.get(n.snap!.zone!) ?? []), n.index]);
        for (const [zone, idx] of byZone) if (idx.length >= 2) pick = { founder: idx[0], zone };
        return pick !== null;
      },
      { timeout: 150_000, intervals: [2000] },
    )
    .toBe(true);
  const s0 = await state();
  const point = s0.nodes[0].snap!.zones.find((z) => z.zone === pick!.zone)!.point;
  const above: [number, number, number] = [point[0], point[1], point[2] + 1];
  const id = await page.evaluate(
    ([f, p]) => (window as unknown as { simuApi: { propose: (a: number, b: number[], c: number[], d: number) => string } }).simuApi.propose(f as number, [f as number], p as number[], 16),
    [pick!.founder, above],
  );
  console.log('proposal', id, 'at', above, 'founder', pick);
  await expect
    .poll(
      async () => {
        const s = await state();
        const f = s.nodes.find((n) => n.index === pick!.founder)!.snap!;
        const created = f.zones.find((z) => z.point.join() === above.join());
        return !!created && f.zone === created.zone && created.last_commit !== null;
      },
      { timeout: 150_000, intervals: [2000] },
    )
    .toBe(true);
});
