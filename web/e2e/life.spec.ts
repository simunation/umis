// Life genesis only:
//   MEMBERS=6 GENESIS_ARGS="--task life --n-loc 64 --gens 1 --region 3,2 --slot-ticks 4,4,8 --proof-backend plonky3" \
//     scripts/e2e.sh life.spec.ts
// Devnet validators are full nodes, so zones get members only from joiners; a
// proposal needs members in every adjacent zone (diagonals included) and the
// founder must not empty its own zone. Native joiners (MEMBERS) give every
// zone a member; then one page node joins and founds a zone on a free in-plane
// point next to its own zone. The new zone is born, advances with proofs,
// and adjacent zones stay within k generations of each other (the globally
// exact rule of task_life2d). Zones with a recent rollback are left out of
// that check: right after one, the bootstrap's views mix old and new branches.
import { expect, test } from '@playwright/test';

interface LifeZone {
  zone: number;
  point: [number, number, number];
  born: boolean;
  gen: number;
  k: number;
  status: string | null;
  nextSlot: number;
  slotTicks: number;
  rolledBack: boolean;
}
interface Snap { kind: string | null; zone: number | null }
interface BootZone { zone: number; point: [number, number, number]; members: number }
type PageState = { ready: boolean; nodes: { index: number; snap: Snap | null }[]; bootstrap: { zones: BootZone[] } | null; life?: LifeZone[] };

const adjacent = (a: number[], b: number[]) => Math.max(Math.abs(a[0] - b[0]), Math.abs(a[1] - b[1])) === 1;

/** Adjacent born zones (no recent rollback) more than k generations apart. */
function violations(zs: LifeZone[]): string[] {
  const born = zs.filter((z) => z.born && !z.rolledBack);
  const bad: string[] = [];
  for (const a of born)
    for (const b of born) if (a.zone < b.zone && adjacent(a.point, b.point) && Math.abs(a.gen - b.gen) > a.k) bad.push(`z${a.zone}@${a.gen} z${b.zone}@${b.gen}`);
  return bad;
}

test('a browser-founded life zone is born and advances, neighbours within k', async ({ page }) => {
  test.setTimeout(560_000);
  page.on('pageerror', (e) => { if (!e.message.includes('closure invoked')) console.log('pageerror', e.message); });
  await page.goto('/');
  const state = () => page.evaluate(() => (window as unknown as { simu: PageState }).simu);
  await expect.poll(async () => (await state()).ready, { timeout: 60_000 }).toBe(true);
  const isLife = await page.evaluate(() => document.getElementById('view2d')?.hidden === false);
  test.skip(!isLife, 'needs a life genesis');
  await expect.poll(async () => (await state()).life !== undefined, { timeout: 30_000 }).toBe(true);
  const seen: string[] = [];
  const check = (s: PageState) => {
    const v = violations(s.life ?? []);
    if (v.length) {
      seen.push(...v);
      console.log('spread', v.join(' '), '|', (s.life ?? []).map((z) => `z${z.zone} gen ${z.gen} slot ${z.nextSlot}×${z.slotTicks}`).join(', '));
    }
  };

  // every zone has a (native) member first, then the page node joins one of them
  await expect
    .poll(async () => { const z = (await state()).bootstrap?.zones ?? []; return z.length > 0 && z.every((x) => x.members >= 1); }, { timeout: 200_000, intervals: [2000] })
    .toBe(true);
  await page.evaluate(() => (window as unknown as { simuApi: { nodes: { add: (autoJoin: boolean) => Promise<unknown> } } }).simuApi.nodes.add(true));

  // the page node a member of a zone with another member
  let pick: { founder: number; free: [number, number, number] } | null = null;
  let polls = 0;
  await expect
    .poll(
      async () => {
        const s = await state();
        check(s);
        const me = s.nodes[0];
        const zones = s.bootstrap?.zones ?? [];
        if (++polls % 10 === 0) console.log('waiting: page node', me?.snap?.kind, 'zone', me?.snap?.zone, '| members', zones.map((z) => `z${z.zone}:${z.members}`).join(' '));
        if (me?.snap?.kind !== 'Member' || !zones.length || zones.some((z) => z.members < 1)) return false;
        const own = zones.find((z) => z.zone === me.snap!.zone);
        if (!own || own.members < 2) return false;
        const taken = new Set(zones.map((z) => z.point.join()));
        for (const [dx, dy] of [[1, 0], [0, 1], [1, 1], [-1, 1], [1, -1], [-1, 0], [0, -1], [-1, -1]]) {
          const p: [number, number, number] = [own.point[0] + dx, own.point[1] + dy, 0];
          if (p[0] >= 0 && p[1] >= 0 && !taken.has(p.join())) {
            pick = { founder: me.index, free: p };
            return true;
          }
        }
        return false;
      },
      { timeout: 200_000, intervals: [2000] },
    )
    .toBe(true);
  const { founder, free } = pick!;
  const id = await page.evaluate(
    ([f, p]) => (window as unknown as { simuApi: { propose: (a: number, b: number[], c: number[], d: number) => string } }).simuApi.propose(f as number, [f as number], p as number[], 8),
    [founder, free],
  );
  console.log('proposal', id, 'at', free, 'founder', founder);

  // born, then at least two advancing slots, proven or settled
  let born: LifeZone | undefined;
  await expect
    .poll(
      async () => {
        const s = await state();
        check(s);
        const z = (s.life ?? []).find((x) => x.point.join() === free.join());
        if (z?.born && !born) born = z;
        return !!born && !!z && z.gen >= born.gen + 2 * z.k && (z.status === 'proven' || z.status === 'settled');
      },
      { timeout: 240_000, intervals: [2000] },
    )
    .toBe(true);
  const end = await state();
  console.log('life zones', (end.life ?? []).map((z) => `z${z.zone}(${z.point.slice(0, 2)}) gen ${z.gen} ${z.status}`).join(', '));
  expect(seen, 'adjacent zones never more than k generations apart').toEqual([]);
});
