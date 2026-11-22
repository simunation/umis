# Which simulations fit our proofs, intuitively

Side reading for choosing toy and real simulations (owner request, 2026-11-22). It covers what "constraint degree"
means, what actually makes physics expensive to prove, and which simulations fit our current settings
(P100: blowup 4, so constraints up to degree 5).

## 1. What a proof checks

A STARK proof turns a computation into a **table**: one row per step (or per group of cells), one column per
value. The rule ("next state = f(neighbours)") becomes a set of **equations that must equal zero on every row**.
The verifier never reruns the simulation. It checks, at a few random points, that the table satisfies the
equations. This works because the prover encodes each column as a polynomial and the equations as polynomial
identities.

## 2. Degree, intuitively

The **degree** of an equation is how many table values get multiplied together in its biggest term:

| equation | degree |
|---|---|
| `a + b − c = 0` | 1 |
| `a · b − c = 0` | 2 |
| `a · b · c − d = 0`, or `a³` | 3 |

Why it matters: the prover stretches every column to several times its length (the **blowup**) to check the
equations at random points. A degree-`d` equation produces a polynomial about `d` times longer, so it needs more
stretch. More stretch means a slower prover and bigger proofs ([Winterfell: "keep this degree as low as possible,
e.g. under 4 or 8"](https://docs.rs/crate/winter-air/latest)). Our P100 preset stretches 4×, which allows degree up
to 5. The Game of Life rule is degree 3.

**Degree never limits what you can simulate.** Any high-degree formula splits into steps, each stored in a
**helper column**:

```
x⁸  →  t = x²,  s = t²,  y = s²      (three degree-2 equations, two helper columns)
```

So the real trade-off is **more columns** (more data to commit) against **more stretch** (larger blowup). It is
never "possible against impossible".

## 3. What actually makes physics expensive

| cost driver | why | how it is proved |
|---|---|---|
| **Fixed-point arithmetic** (the main cost) | Proofs work over integers mod a prime `p` (here `p ≈ 2³¹`), not real numbers. A real value is stored as an integer times `2^-f`; after every multiplication you divide by `2^f` and round | Prover supplies the quotient `q` and remainder `r`; check `a·b = q·2^f + r` and that `0 ≤ r < 2^f`. The **range check** on `r` costs one column per bit, or one lookup per value |
| Division, square root | Not polynomial | Prove backwards: supply the answer `q` and check `b·q + r = a` (or `q² ≤ a < (q+1)²`) plus range checks |
| `exp`, `log`, `sin`, … | Not polynomial | Lookup tables, or a short polynomial approximation (a few multiplications) |
| Floating point | Exponent alignment and normalisation per operation | Very expensive: avoid; use fixed point |
| Overflow | Values must stay far below `p` (about 2³¹) | Range checks, or a wider representation (several limbs) |
| **Non-local steps** | A zone sees only its own cells plus a halo | Global solves (for example the pressure Poisson equation in incompressible Navier–Stokes) need many local sweeps or a different method |
| Moving particles | Cells no longer have fixed neighbours | Memory or permutation arguments (lookups / LogUp) |
| Many values per cell | More columns per cell | Fine: columns scale linearly |
| Bigger stencils (radius `r`) | More neighbours per equation (more terms, not higher degree) | Fine for small `r` |

**Rule of thumb:** cost ≈ cells × (values per cell + helper columns + range-check bits) × blowup. Degree only picks
the blowup.

## 4. The fit table

Explicit, local update rules suit our framework (zones, halos, one proof per slot). "Degree" is per equation after
a few natural helper columns; every row also pays fixed-point rounding range checks.

| simulation | what it models | degree | fit | notes |
|---|---|---|---|---|
| Game of Life / cellular automata | discrete patterns | 3 | done (toy 1) | bits only, no rounding |
| Heat / diffusion | temperature, concentration spreading | 1 + rounding | **easiest next toy** | weighted average of neighbours |
| Wave equation | sound, vibrating membrane | 1 + rounding | easy | two values per cell (height, velocity) |
| Advection, upwind | stuff carried by a flow | 2 (a direction-selector bit) | easy | the selector is a boolean helper column |
| Burgers equation | simplest nonlinear flow, shocks | 2 (`u·∂u/∂x`) | easy | first nonlinear fluid toy |
| Reaction–diffusion (Gray–Scott) | chemical patterns (spots, stripes) | 3 (`u·v²`) | easy, visually rich | two values per cell |
| **Lattice Boltzmann (D2Q9, BGK)** | fluids (incompressible flow, vortices) | 3–4: the equilibrium is quadratic in velocity times density ([1](https://arxiv.org/pdf/1501.00515), [2](https://arxiv.org/pdf/2212.06124), [Dellar](https://people.maths.ox.ac.uk/dellar/papers/DSFD04reprint.pdf)); `u = m/ρ` is one division via a helper | **best fluid candidate** | purely local (stream + collide), 9 values per cell, standard method; reproduces Navier–Stokes at second order ([2](https://arxiv.org/pdf/2212.06124)) |
| Shallow water | waves, floods, tides | 3 (division by depth `h` via a helper) | good | 3 values per cell |
| Compressible Euler (finite volume) | gas dynamics, shocks | 3–4, plus sqrt (sound speed) and min/max (limiters) | medium | sqrt and limiters need range checks |
| Incompressible Navier–Stokes (projection) | water, smoke | low degree, but a **global pressure solve** | poor | prefer lattice Boltzmann or artificial compressibility (local) |
| Particles: molecular dynamics, SPH | atoms, granular, splashes | 2–3 with helpers (use `r²`, inverses) | hard | particles move between cells, needing lookups |
| Monte Carlo (Ising, ...) | magnets, statistical physics | low, but needs randomness and `exp` | medium | public random seed; `exp` by lookup table |

## 5. Suggested toy ladder

1. **Life** (done, `umis_stencil::life`): bits, degree 3; the framework acceptance test.
2. **Heat diffusion** (done, `heat`): one fixed-point value per cell; rounding and range checks.
3. **Gray–Scott reaction–diffusion** (not yet): two values, degree 3, still simple. A good speed benchmark.
4. **Lattice Boltzmann D2Q9** (done, `lbm`): 9 values per cell, divisions by constants (staged so every
   integer stays far below `p`), exact mass conservation; the first real fluid. Shear layers roll up into
   vortices.

How to run them and what they cost: `docs/stencil.md`.

Each step adds exactly one new difficulty (fixed point; nonlinearity; many values per cell plus division). When
range checks dominate the columns, switch them to lookups (ROADMAP §0 S5).

## 6. What "degree ≤ 5" means for us

- Everything in the table fits at degree ≤ 5 with a few helper columns: no change to P100 is needed.
- A rule with a naturally higher degree (for example a degree-8 polynomial approximation) is split by helper
  columns. Alternatively we switch to blowup 8, which allows degree 9 and needs fewer queries; recompute the
  security with p3-security.
- The stencil framework (ROADMAP §0 S1) should let a rule declare helper columns, so splitting is routine.
