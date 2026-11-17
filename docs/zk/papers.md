# Paper catalogue

The PDFs and their text extractions (`pdftotext -layout`) are in `tmp/zk-research/papers/`. They are not tracked: the papers are copyrighted, so keep them out of git. Re-fetch them from the links if the folder is lost.

**Priority:**
- **★★★** read before changing that area;
- **★★** background needed for design decisions;
- **★** reference.

The **Notes** column points to the note under `docs/zk/notes/` that extracts theorem numbers, pages and formulas.

## A. FRI / Reed–Solomon soundness (what "N bits of security" means)

| ID | Paper | Year | Pri | What it gives umis | Notes |
|---|---|---|---|---|---|
| `eprint-2020-654` | Ben-Sasson, Carmon, Ishai, Kopparty, Saraf, *Proximity Gaps for Reed–Solomon Codes* | 2020 | ★★★ | Proven proximity gaps and correlated agreement in the unique-decoding and Johnson regimes. The basis of every *proven* FRI bound, including p3-security's `low-degree-test` and `batch-combination` terms | `fri-soundness-foundations.md` |
| `eprint-2021-582` | StarkWare, *ethSTARK Documentation v1.2* | 2021 | ★★★ | The STARK/FRI protocol and its soundness analysis, plus the old "conjectured security" formula (queries × log blowup + PoW). That formula is the source of the obsolete "~116 bits" comment in `config.rs` | `fri-soundness-foundations.md` |
| `eprint-2022-1216` | Haböck, *A summary on the FRI low degree test* | 2022 | ★★★ | A compact, self-contained FRI soundness statement (batched FRI, DEEP, query and commit-phase errors). The most readable reference for the parameter recipe | `fri-soundness-foundations.md` |
| `eprint-2025-2110` | Haböck, *A note on mutual correlated agreement for RS codes* | 2025 | ★★ | Proves mutual correlated agreement up to the Johnson bound, which WHIR and p3-security need for proven numbers | `fri-soundness-foundations.md`, `pcs-stir-whir-circle.md` |
| `eprint-2025-2010` | Diamond, Gruen, *On the Distribution of the Distances of Random Words* | 2025 | ★★ | Refutes the up-to-capacity conjectures (rate → 0, any field). Its "random words" heuristic is what p3-security calls *conjectured*; it is a heuristic, not a theorem | `proximity-gaps-status.md` |
| `eprint-2025-2046` | Crites, Stewart, *On RS Proximity Gaps Conjectures* | 2025 | ★★ | Shows failure beyond list-decoding capacity. With the domain in the base field, the **base-field** entropy (log₂p ≈ 31) governs, so EF4 doesn't rescue it | `proximity-gaps-status.md` |
| `eprint-2025-2055` | Ben-Sasson, Carmon, Haböck, Kopparty, Saraf, *On Proximity Gaps for RS Codes* | 2025 | ★★ | Improved Johnson-regime bound O(n/η⁵) (Thm 1.5) and a characteristic-2 counterexample (Thm 1.6). Has an explicit M31⁴ instance | `proximity-gaps-status.md` |
| `eprint-2026-782` | Krachun, Kazanin, Haböck, *Failure of proximity gaps close to capacity* | 2026 | ★★★ | A counterexample on **2-power multiplicative subgroups of prime fields**, the kind of domain umis uses. The gap is Θ(1/log n). Costs about 6–9 bits of margin versus capacity at umis' rates | `proximity-gaps-status.md` |
| `eprint-2026-2056` | Dao, Kominers, Thaler, *RS Codes Beyond Johnson* (107 pp) | 2026 | ★★ | Proven first-order beyond-Johnson results and a refined Johnson count (Thm 4.4, already used by p3-security). Not usable for EF4 parameters (caps near 90 bits); a ≈3% gain with EF6 plus a per-code certificate | `proximity-gaps-status.md` |
| `arxiv-2604.09724` | Kambiré, *Proximity Gaps Conjecture Fails Near Capacity over Prime Fields* | 2026 | ★ | An independent prime-field counterexample (Thm 1) | `proximity-gaps-status.md` |
| `arxiv-2609.05870` | Jeronimo, *Algorithmic List Decoding at Capacity and Optimal Proximity Gaps for RS* | 2026 | ★ | Asymptotic list decoding and MCA at any fixed gap from capacity. The constants are not effective, so it can't set parameters | `proximity-gaps-status.md` |
| `arxiv-2609.08005` | Brakensiek, Chen, Putterman, Zhang, Zheng, *Algorithmic List Decoding of RS up to Capacity* | 2026 | ★ | Asymptotic list decoding to capacity, list decoding only (no MCA) | `proximity-gaps-status.md` |

## B. Fiat–Shamir and lookups (what the transcript must bind; how buses are sound)

| ID | Paper | Year | Pri | What it gives umis | Notes |
|---|---|---|---|---|---|
| `eprint-2023-691` | Dao, Miller, Wright, Grubbs, *Weak Fiat–Shamir Attacks on Modern Proof Systems* | 2023 | ★★★ | Why every statement element must be absorbed before the challenges. It justifies umis' keccak digest of periodic data (INV-FS-1) | `fiat-shamir-and-lookups.md` |
| `eprint-2025-118` | Khovratovich, Rothblum, Soukhanov, *How to Prove False Statements: Practical Attacks on Fiat–Shamir* | 2025 | ★★ | An attack on GKR-style protocols when the circuit can compute the transcript hash. Relevant if umis adopts GKR for the stencil; doesn't apply to LogUp-GKR over a fixed circuit | `fiat-shamir-and-lookups.md` |
| `eprint-2022-1530` | Haböck, *Multivariate lookups based on logarithmic derivatives* (LogUp) | 2022 | ★★★ | The lookup argument behind every umis bus. Error ≈ N(W+2)/\|EF\| ≈ 2^-102.6 for umis. Multiplicity sums must stay below p | `fiat-shamir-and-lookups.md` |
| `eprint-2023-1284` | Papini, Haböck, *Improving logarithmic derivative lookups using GKR* (LogUp-GKR) | 2023 | ★★ | Removes about 56 committed LogUp columns for roughly a 10 KB GKR proof. Available via p3 `multi-stark` / `gkr` on main | `fiat-shamir-and-lookups.md` |

## C. Polynomial commitments and IOPPs (upgrade options)

| ID | Paper | Year | Pri | What it gives umis | Notes |
|---|---|---|---|---|---|
| `eprint-2024-390` | Arnon, Chiesa, Fenzi, Yogev, *STIR* | 2024 | ★★ | Fewer queries than FRI. At umis' wide trace it is only about 0.65–0.75× FRI's size; the headline numbers assume the refuted capacity conjecture. No in-circuit verifier yet | `pcs-stir-whir-circle.md` |
| `eprint-2024-1586` | Arnon, Chiesa, Fenzi, Yogev, *WHIR* | 2024 | ★★★ (phase 3) | Very fast verification, 3.6–4.6× faster than FRI. The recommended PCS for inner and outer aggregation layers, at Johnson-bound parameters. Multilinear only | `pcs-stir-whir-circle.md` |
| `eprint-2024-278` | Haböck, Levit, Papini, *Circle STARKs* | 2024 | ★ | The M31/Stwo route. Johnson-regime analysis in the oracle model only; not recommended for umis now | `pcs-stir-whir-circle.md` |

## D. Folding, sumcheck provers and client-side proving (research tracks)

| ID | Paper | Year | Pri | What it gives umis | Notes |
|---|---|---|---|---|---|
| `eprint-2025-247` | Boneh, Chen, *LatticeFold+* | 2025 | ★ | Post-quantum folding. No production implementation, and no KoalaBear parameters (X^d+1 splits) | `folding-sumcheck-client.md` |
| `eprint-2026-242` | Nguyen, Setty, *Neo and SuperNeo* | 2026 | ★★ | Small-field folding. A natural fit for zone IVC, but parameters exist only for Goldilocks-class fields and there is no public code. Sketch: about 852k CCS rows per slot | `folding-sumcheck-client.md` |
| `eprint-2021-673` | Liu, Xie, Zhang, *zkCNN* | 2021 | ★ | Sumcheck/GKR for convolutions. Its FFT trick doesn't apply (umis' weights vary per cell), but a layered GKR stencil prover could commit about 12× less data | `folding-sumcheck-client.md` |
| `arxiv-2510.14693` | Malatrait, Sirac, *FibRace* | 2025 | ★★ | Lessons from 2.2M client proofs on phones: memory, throttling, worker leaks. Basis for the browser prover admission and recycling rules | `folding-sumcheck-client.md` |

## E. Not downloaded (cited in notes; fetch if needed)

- Nova 2021/370, HyperNova 2023/573, ProtoStar 2023/620, LatticeFold 2024/257, Arc 2024/1731, CCS/SuperSpartan 2023/552, Jolt 2023/1217, Binius 2023/1784, HVZK-WHIR 2026/391, Fenzi–Sanso 2025/2197, Arnon–Boneh–Fenzi 2026/680, S-two whitepaper 2026/532.

## Suggested reading order before implementation work

1. **Phase 1** (parameters, invariant fixes): `air-spec.md`, then 2022-1216, then 2023-691, then 2022-1530.
2. **Phase 2** (multi-slot proofs, Poseidon2 commitment, rc.2): `notes/plonky3-implementation-guide.md` §2–3, then 2023-1284.
3. **Phase 3** (recursion/aggregation): guide §4, then 2024-1586 (WHIR), then 2025-2110.
4. **Security sign-off:** 2020-654, 2026-782, 2025-2046, and `notes/proximity-gaps-status.md`.
