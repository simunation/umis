// Browser STARK proving (Plonky3 genesis only):
//   GENESIS_ARGS="--proof-backend plonky3" scripts/e2e.sh stark.spec.ts
// Two browser nodes join as zone members, prove their priority slots in Web
// Workers, and the network stays free of rollbacks.
import { expect, test } from '@playwright/test';

interface Snap {
  kind: string | null;
  minted: number;
  proved: number;
  rollbacks: number;
}
type PageState = { ready: boolean; error?: string; nodes: { snap: Snap | null }[]; bootstrap: Snap | null };

test('browser members prove in workers', async ({ browser, request }) => {
  const genesis = await (await request.get('/genesis.json')).json();
  test.skip(genesis.proof_backend !== 'plonky3', 'needs a plonky3 genesis');

  const page = await (await browser.newContext()).newPage();
  page.on('pageerror', (e) => console.log('pageerror', e.message));
  await page.goto('/?n=2');
  const state = () => page.evaluate(() => (window as unknown as { simu: PageState }).simu);

  await expect
    .poll(
      async () => {
        const nodes = (await state()).nodes;
        return (
          nodes.length === 2 &&
          nodes.every((n) => n.snap?.kind === 'Member' && n.snap.minted > 0) &&
          nodes.some((n) => (n.snap?.proved ?? 0) > 0)
        );
      },
      { timeout: 200_000, intervals: [2000] },
    )
    .toBe(true);
  const s = await state();
  expect(s.error).toBeUndefined();
  expect(s.bootstrap?.rollbacks).toBe(0);
});
