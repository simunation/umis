# umis zero-knowledge / STARK knowledge base

Start here before changing anything that touches proofs: `crates/task-water3d/src/air.rs`, `crates/umis-stark`,
`crates/umis-proof`, the proof paths in `crates/umis-protocol` (`beacon/state.rs` `apply_proof_commitment`,
`zone/engine.rs` shares, `node/duties.rs` jobs), the provers in `umis-node` and `umis-web`, and genesis proof settings.

This was built on 2026-11-03 against commit `4c26c42` (Plonky3 0.8.0). File:line references are a snapshot of that commit.

## Map

| Document | Use it for |
|---|---|
| [`air-spec.md`](air-spec.md) | **The rules.** What a proof claims, the arithmetic bounds, the trace layout, and every invariant (`INV-*`) with its current status. Cite these IDs in commits and tests |
| [`aggregation.md`](aggregation.md) | Recursive aggregation spike (`umis-agg`): measured cost and the ~93-bit security blocker |
| [`life2d-air-spec.md`](life2d-air-spec.md) | The Game of Life AIR (`task-life2d/src/air.rs`): claim split, light-cone layout, `INV-LIFE-*` invariants, measured security |
| [`research/REPORT.md`](research/REPORT.md) | The overall analysis: how the simulation and proofs work, measured performance, scaling limits, and the phased roadmap |
| [`notes/plonky3-implementation-guide.md`](notes/plonky3-implementation-guide.md) | **How to code it.** The Plonky3 0.8 API as used; migration to 0.9.0-rc.2; multi-AIR batches; a compile-checked Poseidon2 state commitment; Plonky3-recursion; security knobs and a CI test; per-phase checklists |
| [`notes/fri-soundness-foundations.md`](notes/fri-soundness-foundations.md) | **How to pick parameters.** Where p3-security's formulas come from (paper, theorem, page), the parameter recipe, and the 100/128-bit presets |
| [`notes/proximity-gaps-status.md`](notes/proximity-gaps-status.md) | Which soundness conjectures are refuted, proven or open; what number to quote |
| [`notes/fiat-shamir-and-lookups.md`](notes/fiat-shamir-and-lookups.md) | Transcript binding rules, LogUp bus rules, how to add or merge buses, LogUp-GKR, missing negative tests |
| [`notes/pcs-stir-whir-circle.md`](notes/pcs-stir-whir-circle.md) | Which PCS to use at which layer (leaf FRI, aggregation WHIR); STIR/Circle pitfalls |
| [`notes/folding-sumcheck-client.md`](notes/folding-sumcheck-client.md) | Research tracks (folding, GKR stencil prover) and browser-prover rules from FibRace |
| [`research/protocol-proof-flow.md`](research/protocol-proof-flow.md) | End-to-end proof flow through the protocol, and the touch list for changing the proof system |
| [`research/plonky3-internals.md`](research/plonky3-internals.md) | Plonky3 0.8 source audit: transcript order, LogUp compilation, profiling |
| [`research/research-literature.md`](research/research-literature.md) | Ecosystem survey, 45 citations |
| [`papers.md`](papers.md) | Catalogue of the 23 local papers (`tmp/zk-research/papers/`), with priority and reading order |

Local, untracked resources are in `tmp/zk-research/`:
- `papers/`;
- `src/`: upstream clones of Plonky3 main, Plonky3-recursion, Stwo and soundcalc. These are untrusted; **read only, never build or run**;
- `secprobe/`: runs the real p3-security calculator on the umis shape;
- `sketches/`: compiled Plonky3 0.8 and rc.2 sketches;
- `repro-aliasing/`: exploit reproducer;
- `profprobe/`, `scripts/`.

## Current status (2026-11-03)

**Environment:** Rust 1.99.1 with wasm32, wasm-bindgen-cli 0.2.108, and npm dependencies. All of these pass: the workspace tests, the STARK tests, the wasm golden proof (equal to native), SimNet `stark`, and the web build.

**Open issues, by severity** (status 2026-11-19):

| # | Issue | Invariant | Status |
|---|---|---|---|
| 1 | Non-canonical state/halo blobs (`v ± p`) got valid proofs | INV-CANON-1 | **fixed** 2026-11-19 (`decode_cells` range check; `non_canonical_blobs_are_rejected`) |
| 2 | `shift` 15–16 accepted, so the stencil wraps mod p | INV-ARITH-1 | **fixed** (`shift ≤ 14`) |
| 3 | Security ≈ 66 proven bits; `koalabear-test` selectable by any genesis | INV-PARAM-1/3 | **fixed**: `koalabear*` = P100, ≥ 101 proven bits (UDR) asserted per task; test ids need `insecure_proofs` + `--allow-insecure-proofs` |
| 4 | Junk shares or proof commitments triggered a global `MissingProof` rollback (DoS) | — (protocol) | **fixed**: proof commitments verified on receipt against head + pending commitments, candidates per signer, eviction; untrusted shares never block self-proving (`junk-proofs`, `life-stark-junk` SimNet scenarios) |
| 5 | Tamper tests reach the prover's debug checker in debug builds | air-spec §8 | run them with `--release` (CLAUDE.md); life2d also with `LIFE_PRESET=p100` |
| 6 | Native builds lacked AVX2 | — | fixed 2026-11-14 (`.cargo/config.toml`) |
| 7 | Scaling: per-slot proofs plus full states per leaf on the beacon chain | — | roadmap phases 2–3 (recursion, INV-PARAM-5/6, INV-LIFE-FS-2) |
| 9 | `L = 0` with plonky3 | INV-PARAM-1 | **kept, by decision**: valid (proofs at inclusion); tested (`life-stark-l0`, `junk-proofs-l0`) |
| 8 | Batch STARK security falls with the member count | INV-LIFE-BATCH-4 | **fixed**: at most 32 members, bound asserted at 32 |

**Facts verified in this pass:**
- Moving to Plonky3 `=0.9.0-rc.2` is a pure version bump: same proof bytes, tests green.
- AVX2 doesn't change proofs.
- Proofs with PoW and more than one thread are not byte-reproducible (0.8 and rc.2).

## Decisions

**Settled by the evidence:**
- Stay on Plonky3 batch-stark with FRI at the leaves. Use Plonky3-recursion with WHIR (Johnson-bound parameters) for aggregation.
- No Stwo/Circle, folding or zkVM for now (see the notes for when to revisit).
- Target **0.9.0-rc.2**: pin Plonky3 to whatever version Plonky3-recursion pins, and pin Plonky3-recursion to a git commit.
- Keep the keccak digest of periodic data until per-claim data leaves the periodic columns (INV-FS-1, INV-PARAM-6).
- Quote **proven (Johnson)** security. Label any conjectured number as heuristic.

**Tension to resolve when choosing the preset.** These rules pull against each other:
- In the Johnson regime, cheap ≥100-bit presets rely on PoW at the batch, OOD and lookup steps (`fri-soundness-foundations.md` §3, e.g. P100-lb3: lb 3, 58 queries, ≈153 KB).
- The Plonky3-recursion verifier rejects non-zero PoW at those steps (INV-PARAM-5).
- The recursion-compatible alternative is lb 2, 128 queries, commit and query PoW 16, which gives ≈100.6 bits at ≈230–285 KB.

Choose per layer: non-recursed final proofs can use PoW everywhere; recursed leaves cannot.

**Caveat on proven numbers:** p3-security's Johnson-regime commit and batch term uses the 2026 DKT count. Under the classic BCI+20 count alone, the P100-lb3 preset would be about 83 bits, not 101.7. If you want security that rests only on long-established results, use the unique-decoding presets (UDR-only) in `fri-soundness-foundations.md`.

**Decided 2026-11-19:** 100 proven bits, unique-decoding regime (preset P100, `umis_stark::security`). 128 would
need EF5, a digest of at least 9 elements and Poseidon2 width 24: with EF4 the ceiling is about 113 bits.

**Recursion vs P100 (measured 2026-11-19, `secprobe RECUR=1`).** Plonky3-recursion's in-circuit verifier rejects
grinding at the OOD, batch and lookup sites (INV-PARAM-5); P100 uses all three. Without batch grinding the
unique-decoding batch-combination term (`123 − log2|LDE| − log2 B`) caps leaves at: 101.8 bits for one 256×256
life proof at the default shape (126 queries at lb 2), 96.8 for a batch of 32, 91–98 for the largest shapes of the
envelope; lb 3 is worse (bigger LDE). So aggregated leaves at 100 UDR bits need one of: (a) single-instance
leaves of small shapes only (aggregation then replaces batching); (b) the Johnson regime (depends on DKT26,
§3.6 of `notes/fri-soundness-foundations.md`); (c) batch grinding supported in-circuit upstream; (d) EF5, which
adds about 31 bits to every field-size term. The Plonky3 0.9.0-rc.2 bump P3R needs is done (commit 0954b2f).

**Waiting on the owner:**
1. (decided: 100, see above)
2. Canonical state roots: keccak/Swarm BMT (dual roots) or Poseidon2.
3. Browser role: leaf prover only, or also verifier.
4. Nonlinear physics: it changes the constraint degree, so FRI `log_blowup` has to be at least 2.
5. Plonky3 version policy (recommended: follow Plonky3-recursion's pin).

## Golden rules for proof changes

1. Read `air-spec.md` first, and cite the `INV-*` IDs you touch.
2. Every new soundness-relevant check gets a **negative test that runs the verifier in release mode**. A panic in the prover is not a rejection.
3. Compute security with p3-security (`tmp/zk-research/secprobe`) for the exact shape. Never hand-wave with `queries × log_blowup + PoW`.
4. Anything the verifier doesn't recompute or bind into the transcript is not part of the statement. Check with `notes/fiat-shamir-and-lookups.md` §2.
5. Proof-format changes re-pin `crates/task-water3d/tests/golden-proof-4.txt` and must keep native and wasm byte-identical (test preset, no PoW).
6. Don't build or run code from `tmp/zk-research/src`. It's reference material.
