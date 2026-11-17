# Folding, sumcheck/GKR proving, and client-side proving: notes for umis

Status: research notes, 2026-11-03. Repo state `nu` @ `4c26c42`.
Scope: whether lattice folding (LatticeFold+, Neo/SuperNeo) or a zkCNN-style sumcheck/GKR prover could replace or complement the current water3d STARK (Plonky3 0.8 batch-stark over KoalaBear), and what FibRace's mobile-proving data implies for umis' browser provers.

Context documents: `docs/zk/research/REPORT.md` (§2–3, §5–8) and `docs/zk/research/research-literature.md` (§3–4). Stencil: `crates/task-water3d/src/stencil.rs`. AIR: `crates/task-water3d/src/air.rs`, spec in `docs/zk/air-spec.md`.

Page numbers are **PDF page indices** of the local copies, which are not always the printed page numbers. For FibRace, PDF page = printed page + 2.
Tags: **[P]** stated in the paper at the cited page; **[W]** web or repository check done on 2026-11-03; **[E]** my estimate, with the reasoning given; **[U]** unverified or open.

---

## 0. Summary

1. **Neither folding nor a sumcheck prover is a drop-in improvement today.** STARK + `p3-recursion` (REPORT §8, phases 2–3) is still the right main path.
2. **A sumcheck/GKR formulation fits the stencil very well, but not through zkCNN's FFT trick.**
   - The umis stencil has **per-cell weights** `w_k(x)`. It is a 7-diagonal sparse matrix–vector product, not a translation-invariant convolution, so FFT convolution does not apply.
   - Even for true convolutions, zkCNN's own measurements show FFT barely helps with tiny kernels (§1.3).
   - What does carry over is the GKR machinery: fan-in-k gates, wiring from arbitrary layers, and the "rescale by bit decomposition" pattern.
   - Expected effect [E]: roughly 10× fewer committed field elements per slot (only remainders and lookup multiplicities). That plausibly makes the prover 3–10× faster.
   - Proof size would be about the same as today, because the PCS opening dominates.
   - The blocker is recursion support, not the prover.
3. **Folding (SuperNeo is the best fit; LatticeFold+ less so) suits the zone IVC conceptually, but has three problems for umis:**
   - **Field.** Published parameters are only for 64-bit fields (Goldilocks, Almost-Goldilocks, M61) for SuperNeo, and a 128-bit q for LatticeFold+. There is no KoalaBear parameter set.
   - **Maturity.** No Neo/SuperNeo implementation was found. LatticeFold has a Nethermind proof of concept; LatticeFold+ has only a hackathon prototype.
   - **Per-step cost.** Pay-per-bit commitment of about 2^21 witness bits per slot is plausibly no cheaper than today's ~90 ms STARK. The win is aggregation, not leaf proving.
4. **FibRace** (native mobile apps, Stwo/Cairo-M, M31) shows:
   - phone proving is practical when traces are small and memory stays bounded;
   - memory, not CPU, causes failures;
   - sustained load causes throttling and leaks.

   For umis browser provers:
   - keep each wasm job at ≤1–2 s on a median device and ≤256 MB per worker;
   - recycle workers;
   - adapt the worker count to measured throughput;
   - don't send phones `n_z = 16` multi-slot traces.
5. **The experiments that would decide this** are in §4: E1 (a sumcheck stencil prover on Plonky3 0.9 crates), E2 (a lattice-folding step measured with an existing prototype), and E3 (the `p3-recursion` aggregation-node baseline, which is needed anyway).

---

## 1. Papers

### 1.1 LatticeFold+ (Boneh–Chen)

- **Citation.** D. Boneh, B. Chen, *LatticeFold+: Faster, Simpler, Shorter Lattice-Based Folding for Succinct Proof Systems*, CRYPTO 2025 (LNCS 16006, pp. 327–361); IACR ePrint 2025/247, version of 2025-08-09.
- **Local copy.** `tmp/zk-research/papers/eprint-2025-247.pdf` (`.txt`), 54 pages.

**Main results**

- **Pipeline** (p. 5) [P]:
  1. A sumcheck "linearization" turns R1CS/CCS into a linear relation `R_lin,B`: an Ajtai commitment `cm` to `f ∈ R_q^n` with ‖f‖∞ < B, plus evaluation claims (Def. 3.1, p. 15).
  2. L > 2 instances of `R_lin,B` fold into one `R_lin,B²`.
  3. A decomposition step splits that back into two `R_lin,B` instances.
- **New algebraic range proof** (§4.3; Theorem 4.2, p. 24). To show f_i ∈ (−d/2, d/2):
  - the prover commits to monomials m_i = X^{f_i};
  - Lemma 2.2 (p. 10) ties m_i to f_i through ψ = Σ i·(X^{−i} + X^i);
  - a monomial-set check (§4.2, Corollary 4.1, p. 18) completes the proof.

  No bit-decomposed commitments are needed. Committing to monomials needs only additions (Remark 4.3, p. 19).
- **Double commitments and commitment transformation** (§4.1 and §4.4; Theorem 4.3, p. 30). A commitment to the vector of d commitments shortens proofs; a sumcheck then turns double-commitment claims back into linear ones so they can be folded.
- **Folding.** Theorem 5.1 (p. 37), Theorem 5.2 (p. 40), and the efficiency result Theorem 5.3 (p. 41).
- **R1CS to linear relation:** Appendix A, Fig. 1 (pp. 52–53).
- **Small moduli:** Appendix B (pp. 52–54) uses a "tensor-of-rings" view. Sumchecks run over an extension F_{q^t}, and folding stays over R_q. The authors say this is equivalent to Neo's approach (p. 6, p. 52).

**Cost model** (Theorem 5.3, p. 41) [P]. Folding L instances of dimension n:

| Item | Cost |
|---|---|
| Prover | ≈ L·n·κ R_q multiplications + O(L·n·κ·d·k) R_q additions |
| Verifier (excluding hashing) | O(L·d·k) R_q multiplications |
| Folding proof | about L(5κ+6)+10 R_q elements + L(dk+5) S̄-elements + L(2dk+d+1) C-elements |
| Proof size, asymptotic | O_λ(κd + log n) bits, versus O_λ(κd log B + d log n) for LatticeFold (p. 7) |
| Prover vs LatticeFold | estimated ~5× faster, from Nethermind's LatticeFold numbers (p. 6) |

Concrete candidate parameters (p. 42) [P]:
- L = 3, q 128-bit, d = 64, n = 2^21, S̄ = {−1,0,1,2}^d, B = 2^10, k = 2, κ = 9;
- this proves R1CS over F_{q^4} of size 2^25 (p. 43);
- folding proof < 200 KB, or ≲ 100 KB with Remark 5.3;
- ≲ 100 hashes in the Fiat–Shamir verifier circuit.

The paper has **no implementation**: "leave concrete implementation for future work" (p. 42).

**Field requirements**
- Analysed over `R_q = Z_q[X]/(X^d+1)` with d a power of two.
- Small q is handled only through Appendix B, with no small-field parameter set given.
- Neo/SuperNeo point out the problem with this (Remark 5, p. 34 of 2026/242): over Goldilocks, X^d+1 with power-of-two d splits completely, which hurts security.
- The same holds for **KoalaBear**: p − 1 = 2^24·127, so 2d | p − 1 for every d ≤ 2^23, and X^64+1 splits into linear factors [E, checked: p ≡ 1 mod 128].
- So LatticeFold+ over KoalaBear needs the Appendix B extension-field machinery and new parameters. None are published.

**Security assumptions**
- Module-SIS in ℓ∞: MSIS^∞_{q,κ,m,β} (p. 13).
- Binding: the commitment is (2B², S̄−S̄)-binding (Theorem 5.2, p. 40).
- The p. 42 parameters target 128-bit conjectured M-SIS hardness from the lattice estimator.
- Plausibly post-quantum. Uses the random oracle via Fiat–Shamir.

**Maturity**
- No reference implementation from the authors.
- An ETHGlobal hackathon project describes an experimental Rust LatticeFold+ that reuses Nethermind's Ajtai code [W].
- Nethermind's `NethermindEth/latticefold` implements the *original* LatticeFold [W]:
  - Rust proof of concept, pinned to a nightly toolchain;
  - README says not production-ready;
  - R1CS/CCS front end;
  - no releases.

### 1.2 Neo and SuperNeo (Nguyen–Setty)

- **Citation.** W. Nguyen, S. Setty (Microsoft Research), *Neo and SuperNeo: Post-quantum folding with pay-per-bit costs over small fields*, IACR ePrint 2026/242. It supersedes Neo, ePrint 2025/294.
- **Local copy.** `tmp/zk-research/papers/eprint-2026-242.pdf` (`.txt`), 60 pages.

**Main results** [P]

- **Six desiderata** (pp. 3–5, Fig. 1 p. 5): post-quantum, pay-per-bit commitments, field-native sumcheck and norm checks, general (non-SIMD) CCS, small-field support, and low recursion overhead.
  - SuperNeo is claimed to be the first scheme to meet all six.
  - Neo needs a SIMD constraint system.
  - The same table marks LatticeFold+ ✗ on pay-per-bit, field-native, general CS, small field, and low recursion overhead.
- **Neo embedding.** Field vectors go into the coefficient slots of ring vectors. This preserves norm, and evaluation claims fold homomorphically.
  - Informal Theorem 1 (p. 9); §2.2 (p. 14).
  - The "inner product trick" is Theorem 8 (p. 24).
- **SuperNeo embedding** (§2.3, p. 14; Theorem 2, p. 11). One field vector of length d·n is packed into n ring elements, which is optimal packing.
  - The formal version is Theorems 9–11 (pp. 25–26); Theorem 11 is the evaluation homomorphism.
  - CCS can be over an arbitrary extension L of the commitment field without an NTT embedding.
- **Folding scheme** (§7, pp. 28–33):
  - Π_CCS: a single sumcheck that checks the K new CCS instances, k carried evaluation claims, and all norm bounds (pp. 30–31; Lemma 7, p. 31);
  - Π_RLC: random linear combination with low-norm ring challenges (p. 32);
  - Π_DEC: base-b decomposition from bound B down to b (p. 33; Theorem 13, p. 33).
- **Security of the composition.** "Strong" and "weak" interactive reductions, with the composition Theorem 12 (p. 28). Theorem 3 (p. 17) shows Π_DEC ∘ Π_RLC ∘ Π_CCS is a reduction of knowledge.
- **Multi-folding.** K instances fold at once, which amortizes decomposition (p. 5).
- **Compression.** Compress with Spartan + FRI over Goldilocks without non-native arithmetic (p. 6, p. 11).
- **Related work** (p. 12): Cyclo is equivalent to Neo and has an "initial benchmark". It also notes that if only a bounded number of steps is needed, the accumulator norm check can be dropped; SuperNeo is compatible with that.

**Cost model** [P, with E where marked]

- **Prover.** The prover is dominated by committing to the witness, like HyperNova (p. 3), plus one sumcheck over K = F_{q^2}.
  - Pay-per-bit means commit cost scales with the bit-width of witness values (p. 4).
  - In the concrete parameters, every fresh CCS witness coordinate must have **‖·‖∞ < b = 2** (Def. 20, p. 29; §8, p. 34). In practice, witness values enter as **signed binary digits**, and the CCS matrices recombine them [E, from the definitions].
- **Recursion overhead.** The goal is logarithmic recursion overhead "analogous to HyperNova" (p. 5). Constant-size lattice verifier circuits are called an open problem (p. 5).
- **No benchmarks or proof-size tables** for Neo/SuperNeo are in this paper.
- A ring multiplication costs ≈ 213 ns, versus sub-nanosecond M61 field multiplications (footnote 2, p. 4, citing [68]).

**Field requirements**

All three concrete parameter sets (§8, p. 34) use τ = 1, K = F_{q^2}, and b = 2. Hardness comes from the lattice-estimator scripts in Appendices B.5–B.6 (pp. 56–60).

| Field | Φ, d | κ | Size | Other parameters | M-SIS |
|---|---|---|---|---|---|
| Almost-Goldilocks, q = (2^64−2^32+1)−32 | X^64+1, d = 64 | 15 | n_F = 2^33 | k = 13, B = 2^13, T = 128 | ≈ 129 bits |
| Goldilocks | X^54+X^27+1, d = 54 | 18 | m = 2^30 | k = 14, B = 2^14, T = 216, b_inv ≈ 2.5·10^9 | ≈ 129 bits |
| M61 | X^54+X^27+1, d = 54 | 18 | m = 2^28 | | ≈ 129 bits |

- **KoalaBear, BabyBear and M31 are not covered.** I checked how Φ_81 = X^54+X^27+1 factors [E, computed]:
  - **KoalaBear: Φ_81 is irreducible.** ord_81(p) = 54, so R_F ≅ F_{p^54} is a field and every nonzero challenge difference is invertible. That is favourable for the challenge set.
  - Goldilocks splits into 2 factors, BabyBear into 2, and M31 into 6.
- **But Module-SIS with q ≈ 2^31 is the open question [U].**
  - The paper's script uses an ℓ2 bound of √(n_R·d)·8TB (p. 59).
  - For a 2^21-element witness with T = 216 and B = 2^14 that is ≈ 2^35, which exceeds q ≈ 2^31.
  - So KoalaBear would need smaller B and T, or a larger κ. Rerun Appendix B.6 before believing any 31-bit instantiation.
  - Sumcheck soundness over 31 bits needs a degree-4 or 5 extension (about 124 bits at degree 4, the same ceiling as the STARK).

**Security assumptions**
- Module-SIS (Ajtai commitment, Def. 8 / Theorem 6, p. 20).
- Binding: 4TB-binding needs MSIS^{∞,κ,q}_{n_R,8TB} hard (p. 34).
- Low-norm invertibility (Theorem 4, p. 19) depends on a conjecture from [66] ("Theorem 1.1, Conjecture 2.6") for some parameter regimes.
- Expansion factor T (Theorem 5, p. 20).
- Random oracle for Fiat–Shamir.
- Plausibly post-quantum.

**Maturity**
- No public Neo/SuperNeo implementation found [W]. Searches turned up only the educational `coset-io/baby-lattice-folding`, which targets SALSAA, and Sonobe, which does Nova and HyperNova.
- The ePrint page lists a 2026-08-14 revision [W].
- Research-grade. Not usable for umis without writing the scheme ourselves.

### 1.3 zkCNN (Liu–Xie–Zhang)

- **Citation.** T. Liu, X. Xie, Y. Zhang, *zkCNN: Zero Knowledge Proofs for Convolutional Neural Network Predictions and Accuracy*, ACM CCS 2021; IACR ePrint 2021/673.
- **Local copy.** `tmp/zk-research/papers/eprint-2021-673.pdf` (`.txt`), 18 pages, two-column. Use `pdftotext` without `-layout` for readable text.

**Main results** [P]

- **FFT sumcheck** (§3.1, pp. 5–7). It proves a = F·c with linear prover time O(M+N).
  - Algorithm 2 (p. 7) builds the bookkeeping table F̃(u, x) for all x in O(M+N) by reusing the FFT's root-of-unity structure.
  - Algorithm 1 (p. 6) is the standard linear-time sumcheck.
  - Proof size and verifier time are O(log N), given oracle access to c̃ and F̃.
  - Verifier work can be delegated through log N extra sumchecks, giving an O(log² N) verifier and proof (p. 7).
- **2-D convolution** (§3.2, pp. 7–8). It is flattened to a 1-D convolution, then verified as FFT → Hadamard → IFFT with three sumchecks.
  - Prover O(n²), proof and verifier O(log² n), given oracle access to the input and output MLEs (p. 8).
  - Multi-channel layers use FFT linearity so only ch_out IFFTs are needed (§4.2.3, pp. 9–10).
- **GKR generalizations** (§4.2.1–4.2.2, pp. 8–9): fan-in > 2 additions and inner products in a single sumcheck, and gates that take inputs from any earlier layer at no prover overhead.
- **Quantization and rescaling** (p. 10). Fixed-point rescaling by 2^{−E} is done with "bit decomposition and shift".
  - ReLU: the prover supplies Q bits plus a sign bit, checked by booleanity and recomposition, and keeps the Q′ most significant bits (pp. 10–11).
  - This is the closest analogue to umis' `⌊num/2^s⌋`.
- **Soundness:** Theorem 4.1 (p. 11), proof sketch in Appendix C.
- **Benchmarks** (§5, pp. 11–13; single core, AMD EPYC 7R32):
  - FFT sumcheck: 0.6 ms at 2^10, 0.1 s at 2^18 (p. 12).
  - Single convolution, 32×32 input with a 4×4 kernel: 4.7 ms, versus 7.7 ms naive GKR. The speedup reaches 291× only for 256×256 with a 64×64 kernel (p. 12).
  - Convolution proof size 5.6–8.4 KB, versus 3.9–7.1 KB naive; verifier time 0.1–0.3 ms (p. 12).
  - Table 1 (p. 13): LeNet 0.44 s prove, 71.3 KB, 5.8 ms verify. VGG16: 88 s, 341 KB, 59.3 ms, 24 GB peak memory.

**Cost model**
- Prover O(Σ_i (n_i·ch_in·ch_out + n_i·ch_out·Q)).
- Proof and verifier logarithmic per layer, plus a PCS over S_in (weights plus bit-decomposition advice) (p. 11).
- The Hyrax-style PCS costs O(√N) proof and verify (p. 11).

**Field and security**
- BLS12-381 scalar field, because it is FFT-friendly. Curve25519's order lacks a large power-of-two root of unity (p. 11).
- Discrete log for the PCS, so **not post-quantum**.
- Fiat–Shamir in the ROM.
- The sumcheck/GKR part is field-agnostic. Over KoalaBear it needs extension-field challenges, and KoalaBear's 2-adicity of 24 gives roots of unity up to 2^24.

**Maturity.** `TAMUCrypto/zkCNN`: C++, MIT licence, Hyrax over BLS12-381 as a submodule, last pushed around 2023 [W]. Treat it as a reference design, not a dependency.

**Relevance to umis:**
- the FFT trick does **not** apply (§2.2);
- the layered GKR structure, generalized gates, and rescale-by-advice pattern do apply.

### 1.4 FibRace (Malatrait–Sirac)

- **Citation.** S. Malatrait (KKRT Labs), A. Sirac (Hyli), *FibRace: a large-scale benchmark of client-side proving on mobile devices*, arXiv:2510.14693, 2025-10-09.
- **Local copy.** `tmp/zk-research/papers/arxiv-2510.14693.pdf` (`.txt`), 14 pages.
- **Dataset.** `github.com/kkrtlabs/cairo-m/tree/main/docs/fibrace` (PDF p. 6).

**Setup** [P]
- A mobile game ran from 2025-09-11 to 09-30.
- Each proof was a Cairo M (KKRT's mobile zkVM) proof of F_n over **M31**, with n ∈ [1, 100000], at **96 security bits**.
- Traces were 8 to 800,000 Cairo-M cycles (PDF p. 12).
- Proving was on-device; there were no remote provers or TEEs (PDF p. 5).
- Proofs were verified natively on the Hyli chain.

**Results** [P]

| Measure | Value | PDF page |
|---|---|---|
| Proofs | 2,195,488 | p. 1, 6 |
| Device models | 1,420 | p. 1, 6 |
| Players with at least one proof | 5,156 | p. 6 |
| Countries | 99 | p. 1, 6 |
| Proof duration | right-skewed; most devices 3–10 s; "most modern smartphones" under 5 s | p. 7 |
| Prover frequency | mean ≈ 30 kHz, median ≈ 10 kHz (heavy slow tail) | p. 7 |
| Fastest | Apple A19 Pro and M1–M4 (≤ 3 s), then high-end Snapdragon, MediaTek, Kirin and Exynos (< 3 s) | p. 7, 9 |
| Mid-range Android | 5–10 s | p. 7 |
| Old devices (> 10 s average) | more crashes and throttling | p. 7 |
| RAM | ≥ 3 GB required; 3–4 GB crashes most; stability plateaus above 6 GB | p. 8, 10, 12 |
| Crash rate | 0.72% overall (15,762 crash events over successful proofs) | p. 10 |
| Main cause of crashes | memory pressure, not CPU | p. 10 |
| 3–4 GB buckets | "4.2 and 4.4 crashes per generated proof", from OOM on the largest witnesses | p. 12 |
| Thermal | crash frequency rose with sustained load; heat and battery drain reported but not measured | p. 10, 12 |
| Leak | after more than ~100 back-to-back proofs, high-RAM devices crashed with a suspected un-killed Rust thread leading to OOM | p. 10 |
| Verifier side | Hyli absorbed peaks of 260,000 proofs per hour | p. 11 |

The "4.2 and 4.4 crashes per generated proof" figure is inconsistent with the 0.72% total. It is probably per 100 or per 1,000; check the dataset before quoting it.

**Not reported:** proof sizes, memory per proof, the split between iOS and Android, and **any browser or wasm numbers**. The paper calls the provers mobile apps with a spawned Rust thread, so these are native builds.

**Maturity.** Cairo M is built on Stwo (Circle STARK over M31), with native NEON. This is a production-scale field trial, not a paper prototype.

---

## 2. Design sketches for umis

### 2.0 What one slot proves

Per zone slot: 4 substeps × 4096 cells × 4 values = **65,536 outputs**.

```
num  = Σ_{k=0..6} w_{k,f}(x)·q_t(x+e_k) + tide·F_f(x)
2^s·q_{t+1} + rem = num
rem ∈ [0, 2^s)
|q_{t+1}| ≤ 2^15
```

- s = 12, so |num| < 2^28.
- `w_{k,f}(x)` and `F_f(x)` are **per cell** and fixed per zone by genesis (`Coeffs::compute`).
- Velocity components share `wv`; dye uses `wc`.
- Halos replace out-of-zone neighbours.

Today's STARK (REPORT §3.2, §5.1):
- 2^14 rows; 46 main, 56 LogUp-equivalent and 32 preprocessed columns;
- about 1.67M committed base elements per slot (before blowup);
- 261 KB proof;
- prove times: 89–92 ms native (2 threads, AVX2) and 682 ms wasm (1 thread, scalar).
- 24 of the 46 main columns are neighbour copies that exist only because AIR constraints are row-local.

### 2.1 (a) The stencil as a CCS step for SuperNeo, Neo or LatticeFold+

**Witness, per slot, for pay-per-bit commitments with b = 2** [E]:

| Part | Size |
|---|---|
| `q_{t+1}` for t = 0..3 | 65,536 values × 16 signed digits ∈ {−1,0,1} = 1,048,576 entries |
| `rem` | 65,536 values × 12 digits = 786,432 entries |
| Public x: `X_in` (16,384), `X_out` (16,384), halos (6×1024×4 = 24,576 at n_loc = 16; the AIR uses one halo face per substep), `tide` | |
| **Total witness** | ≈ 1.84M ≈ 2^21 digit entries |

- `X_out` coincides with `q_4` and can be tied to it by linear rows.
- The digits are the committed vector. Every q or rem used in a constraint is a fixed linear combination Σ 2^i·d_i folded into the CCS matrices M_j. This is why pay-per-bit fits: the CCS sees integers, and the commitment sees bits.

**Constraints:**

1. **Stencil rows.** One linear row per (t, x, f):

   `Σ_k w·(Σ_i 2^i·d^{q_t}_{x+e_k,i}) + tide·F − 2^s·Σ_i 2^i·d^{q_{t+1}}_{x,i} − Σ_i 2^i·d^{rem}_{x,i} = 0`

   - That gives **65,536 rows** with about 7×16 + 16 + 12 ≈ 140 nonzeros each.
   - **Neighbour access is free.** It is just sparsity in M, so there are no LogUp buses, no neighbour columns, and no address columns.
   - t = 0 reads public `X_in` and halos. Halos enter as public inputs at the right column indices.
2. **Range checks.**
   - |q| < 2^16 and |rem| < 2^12 come **for free** from the norm check, since every digit is in {−1,0,1}.
   - The norm check is part of the single Π_CCS sumcheck (p. 30 of 2026/242).
   - That is enough for no-wraparound in a 64-bit field: |2^s·q + rem − num| < 2^29 ≪ p.
   - It is **not** enough for `rem ≥ 0`. Signed digits allow negative remainders, which would make the floor non-deterministic. Add booleanity `d(d−1) = 0` on the 12 rem digits: **786,432 degree-2 rows**.
   - Booleanity rows add only sumcheck work, because the same witness is already committed. Alternatively, find an offset digit encoding that maps exactly onto [0, 2^s) [U].
   - The bound |q| ≤ 2^15 does not need its own check: given rem ∈ [0, 2^s) and no wrap, q_{t+1} = ⌊num/2^s⌋ exactly, and the honest bound follows from Σw ≤ 2^s (`stencil.rs` doc comment).
3. **Size.** m ≈ 852k rows (2^20 padded), n ≈ 2^21, CCS degree 2. That is well inside SuperNeo's Goldilocks set (m = 2^30).

**IVC step function.** One slot, with running instance = (zone, slot, state commitment). The step circuit also checks:
- the previous fold (the folding verifier, logarithmic in SuperNeo, ≲ 100 ring-element hashes in LatticeFold+, p. 42–43);
- that the step's `X_in` equals the previous `X_out`.

The second check is free *inside* the CCS if consecutive slots are batched in one step. Across steps, `X_in`/`X_out` would have to be bound through the instance, either as hash or commitment openings or as "committed public IO". This needs design [U].

**Cost estimates** [E, rough: ±3×]

- **Commitment**, SuperNeo Goldilocks parameters:
  - n_R ≈ 2^21/54 ≈ 39k ring elements; κ = 18.
  - That is about 700k ring×ring products with ternary right-hand sides.
  - At ≈ 200 ns each (paper footnote 2) that is ≈ 0.15 s; schoolbook ternary multiplication would be up to ~1 s.
  - Per slot, native, single thread.
- **Sumcheck** over F_{q^2}, 20–21 rounds, about 2^21 terms with degree ≤ 3 from norm checks: about 10^8 base operations, ≈ 0.1 s.
- **Total:** about 0.2–1 s per slot single thread, versus the current STARK at about 0.18 s single-thread-equivalent with AVX2. **No leaf-proving win.**
- **Decomposition** (Π_DEC, k = 14): transient witnesses of 14 × 2^21 entries. In 64-bit words that is ≈ 235 MB; packed as trits ≈ 7 MB. Memory matters for browsers (§3).
- **Where folding would win:** recursion overhead. The per-fold verifier circuit is small relative to a 2^20-row step (a few percent [E]), whereas a STARK recursion node verifies a whole FRI proof in-circuit.
- **Final compression.** A Spartan+FRI-over-Goldilocks SNARK of the accumulator: CCS satisfaction plus Ajtai-opening and norm checks over a 2^21 witness.
  - The Ajtai check alone is about 700k ring products in-circuit (about 4×10^7 field multiplications).
  - That is seconds per beacon slot on a server, and done once [E].

**Distribution is the catch for umis.**
- Folding needs the folder to hold the *witness* of every instance it folds; the running accumulator is not succinct.
- For umis the witness is recomputable from public data (prev state + halos + params): re-executing a slot costs about 460k multiply-adds, under 1 ms.
- So the natural topology is: **an aggregator re-executes every zone slot and folds everything itself.** Browsers would not prove at all.
- At the lattice bound (128 zones × about 4 slots per 4 s): 512 folds × 0.2–1 s ≈ 100–500 core-seconds per 4 s, i.e. 25–125 cores. Multi-folding with K up to 50 (Goldilocks parameters) amortizes Π_DEC.
- This is feasible for a dedicated node but abandons the "browsers prove leaves" model (REPORT §9 Q3).

**Field mismatch**
- Going lattice means moving the zone proof to Goldilocks. That is arithmetically fine, since no-wrap is trivial at 64 bits.
- It breaks reuse of the KoalaBear STARK/`p3-recursion` stack.
- Wasm has no 64×64→128 multiply, so Goldilocks is slower there. Plonky3 does ship a Goldilocks wasm-SIMD path (research-literature §0.5).

**LatticeFold+ variant**
- Same CCS. The range proof (Theorem 4.2) natively proves f_i ∈ (−d/2, d/2) = (−32, 32) for d = 64.
- So a 16-bit q would need 3 limbs of 6 bits (base 64 ≥ 2^16 over 3 limbs; this needs care with d′ = 32) instead of 16 bits. That gives fewer committed entries, though LatticeFold+ lacks pay-per-bit for small values (Fig. 1 of 2026/242).
- Power-of-two cyclotomics split completely over KoalaBear and Goldilocks, so it needs Appendix B extension-field challenges.
- **SuperNeo is the better fit** on fields, packing, and general CCS.

### 2.2 (b) The stencil as a sumcheck/GKR circuit, zkCNN-style

**Why not FFT.**
- zkCNN's speedup comes from translation-invariant kernels: Ũ = IFFT(FFT(X̄) ⊙ FFT(W̄)) (p. 8).
- umis' `w_k(x)` varies per cell: advection is upwind along Ū(x), and the flux limiter is per cell. So the operator is `Σ_k diag(w_k)·S_k`, with shift matrices S_k.
- One could split off a constant diffusion kernel, but the advection part remains variable.
- Even for real convolutions with tiny kernels, zkCNN's own data shows only a 1.6× gain over naive GKR (4.7 ms vs 7.7 ms for 32×32 with a 4×4 kernel, p. 12). The stencil kernel is 7 points.
- Use the **direct sparse-wiring sumcheck** instead (Thaler/Libra-style linear-time prover, which zkCNN uses as its base: Algorithm 1, p. 6).

**Layered circuit** (one layer per substep, 4 layers per slot, or 4·n_z for a whole commitment):

- **Indexing.** Index outputs by (f, x) ∈ {0,1}^2 × {0,1}^12, with x = (x0..3, y0..3, z0..3). That is 14 variables per substep, or 16 with t.
- **Layer t → t+1 relation, as polynomials:**

  `2^s·Q̃_{t+1}(ρ) + R̃_t(ρ) = Σ_{f,x} eq(ρ, (f,x)) · [ Σ_k W̃_k(f,x) · Σ_y S̃_k(x, y)·Q̃_t(f, y) + B̃_k(f,x)·H̃_{t,k}(f, face(x)) + tide·F̃(f,x) ]`

  - S_k(x, y) = 1 iff y = x + e_k inside the zone.
  - B_k is the boundary mask, and H_{t,k} is the halo face.
- **Two-phase sumcheck** (Libra / zkCNN §4.2.1 generalized fan-in gates):
  - phase 1 over x, phase 2 over y;
  - prover O(7·2^14) per layer;
  - each round degree ≤ 3.
- **Verifier-side wiring.** The verifier evaluates S̃_k and B̃_k in closed form.
  - For a 16³ grid with power-of-two sides, "x + 1 within 4 bits, else 0" has an O(log n) MLE (carry-chain formula, as in HyperPlonk/Spartan "next" polynomials) [E].
  - Halo terms are 6 small (2^8 per face) side sums, using zkCNN §4.2.2 "inputs from arbitrary layers".
- **Weights and forcing.** W̃_k and F̃ are preprocessed: 14 + 4 tables of 4096.
  - A native verifier can evaluate them directly in O(18·4096) ≈ 74k multiplications (< 1 ms). It already rebuilds coefficients from genesis to form the verifying key.
  - Inside recursion they would be committed once per zone.
- **Ends of the chain.**
  - The bottom layer Q̃_0 = MLE of public `X_in`; the verifier computes it in O(16k).
  - The top claim is on Q̃_4 = MLE of public `X_out`.
  - Today's data flow (verifier holds states) is assumed. With in-circuit state commitments (REPORT phase 2), states would instead be PCS-committed polynomials, and the claims become openings. That is a neat fit: **the state root becomes a PCS commitment**, though it is not keccak BMT.
- **Floor division and range checks:**
  - `rem` is prover advice and must be **committed**: 65,536 × 12-bit values.
  - Range: rem ∈ [0, 2^12) and q_{t+1} ∈ [−2^15, 2^15) by **LogUp-GKR** (fractional-sum GKR, already in Plonky3 as `p3-gkr` and `multi-stark/fractional_gkr`).
  - The leaves of the q-lookup need Q̃_t at the GKR's final point. That is a *derived* claim, reduced through the stencil layers like any other, so **intermediate states are never committed**.
  - Only lookup multiplicities are committed: 2^12 for rem, 2^16 for q, or 2×2^8 with limbs (but then the limbs are committed).
  - `q_4 = X_out` is range-checked natively by the verifier (REPORT §3.3 already recommends this).
  - zkCNN's alternative (bit-decompose and shift, p. 10–11) would commit 28 or more bits per value and is strictly worse here.
- **Committed per slot:**
  - rem: 65,536
  - m_rem: 4,096
  - m_q: 65,536
  - total ≈ **135k base elements**, versus about 1.67M today (**~12× less**).
  - Per n_z-slot commitment: about 135k·n_z elements, while sumcheck proof size grows only by layers × log.

**Cost estimates** [E, ±3×, KoalaBear⁴ challenges]

- **Prover:**
  - stencil layers: 4 × (2 phases × 14 rounds; ≈ 7·2^14·c operations, mostly base-field in early rounds with small-value optimizations) ≈ 5–10 M base-equivalent operations;
  - LogUp-GKR on 2^16 + 2^16 leaves ≈ 10–20 M;
  - PCS commit of about 135k elements at rate 1/2–1/4 ≈ 0.3–0.5M hashed elements, versus ≈ 3.3M today;
  - expected total **15–40 ms native single-thread, about 100–250 ms scalar wasm**, versus 682 ms.
  - The uncertainty is the extension-field share and how optimized the sumcheck is.
- **Proof size:**
  - stencil sumchecks: 4 × 28 rounds × 4 coefficients × 16 B ≈ 7 KB;
  - LogUp-GKR: about 2 × Σ_{i≤16} i × 4 × 16 B ≈ 17 KB;
  - PCS opening for 2^17 elements at ≥ 100 proven bits: about 60–120 KB with WHIR, or 150–250 KB with FRI.
  - **Total ≈ 90–270 KB.** That is no big change: the PCS dominates, and it is logarithmic in committed size.
  - Gains in size come from WHIR and from multi-slot amortization, not from GKR itself.
- **Verifier:** O(log² N) sumcheck checks, plus O(N) MLE evaluations of public states and weights (< 1 ms native), plus the PCS. About 1–3 ms native [E], versus 8–11 ms today.
- **Soundness:**
  - sumcheck/GKR error ≈ (Σ rounds × degree)/|F^4| ≈ 2^−114 or better;
  - LogUp-GKR adds the usual fingerprint terms;
  - the overall level is still set by the PCS proximity regime, the same issue as today (REPORT §4.1).

**Whole-world variant.** All zones can be proven in one GKR circuit per beacon slot. Width is 128 × 2^14 = 2^21 per layer, depth is 4 × max S_z, and halos between zones are just wiring.
- One server prover, no recursion, one proof per 4 s.
- Committed: about 135k × (zone-slots) ≈ 2^26 elements. That is seconds of multi-threaded PCS work [E].
- This is the sumcheck analogue of "aggregator re-executes everything" from §2.1.

**Practical routes:**
1. **Port the AIR to `p3-multi-stark`** (Plonky3 0.9; SuperSpartan zerocheck + WHIR + LogUp-GKR buses). This is lower effort, but the neighbour columns stay, because AIR rows still see only the current and next row. The gain comes mostly from removing committed LogUp columns.
2. **A custom layered-GKR prover** on `p3-sumcheck` + `p3-gkr` + `p3-whir`. This is the full gain described above, but it is new code and new soundness review.

Recursion: `p3-recursion` targets uni/batch-STARK. In-circuit verification of a custom GKR+WHIR proof would need new recursive gadgets [U]. **This, not prover speed, is the main blocker.**

---

## 3. Client-side proving lessons (FibRace) for umis browser provers

FibRace was **native mobile**, using Stwo/M31 with NEON. umis provers are **wasm in Web Workers**, with no KoalaBear simd128 and one thread per worker (REPORT §5.1, §6). Treat FibRace numbers as an upper bound on what browsers achieve on the same hardware.

1. **Device spread is about 3× at the median and about 10× at the tail.**
   - FibRace mean 30 kHz vs median 10 kHz; flagship ≤ 3 s, mid-range 5–10 s, old devices > 10 s.
   - umis' 682 ms single-thread wasm on a desktop core suggests about 2–3 s on a median phone browser and over 7 s on the tail [E].
   - Implication: per-job budgets must be set by the slow tail. Phase 2's `n_z = 16` multi-slot proofs (2^18 rows) are about 16× more work: tens of seconds on phones, so **desktop or server only**.
2. **Memory is the failure mode.**
   - FibRace needed ≥ 3 GB device RAM; failures were OOM at the largest witnesses.
   - Today's umis job is small. Estimate: about 134 columns × 2^15 LDE rows × 4 B ≈ 18 MB of trace, plus Merkle trees and quotient, for tens of MB per worker [E]. Measure it.
   - Recommendations:
     - set an explicit per-worker memory budget (e.g. ≤ 256 MB);
     - refuse jobs whose trace height exceeds a device-class cap;
     - remember that the lattice-folding decomposition (§2.1: up to ~235 MB transient in 64-bit words) would be over budget on low-end phones.
3. **Sustained load causes throttling and leaks.**
   - FibRace saw more crashes at peak activity and OOM after more than ~100 consecutive proofs from a suspected thread leak.
   - umis' `ProverPool` (`web/src/prover.ts`) keeps workers forever, uses up to min(4, cores/2) workers, and has no per-job timeout. Recommendations:
     - recycle each worker after N jobs or after its wasm memory grows (`WebAssembly.Memory.buffer.byteLength`);
     - add a per-job timeout tied to the proof deadline, so a stuck worker is killed rather than holding the job;
     - adapt the worker count from measured ms per job: shrink when per-job time rises (throttling), and do not count hyperthreads.
4. **Deadlines.**
   - Proof commitments are due within L = 1 beacon slot (4 s), and the delegate proves missing shares itself at B/8 (REPORT §3.4).
   - A browser job should finish well before the delegate's fallback. Practical rule [E]: accept a job only if the device's measured p90 job time is ≤ ~1–1.5 s; otherwise run as verifier or relay only.
   - FibRace's ≤ 5 s "smooth UX" target is a user-experience target, not a consensus deadline. umis needs tighter, p90-based admission.
5. **Proof size and bandwidth.** FibRace does not report proof sizes. For umis, mobile uplink matters: a 261 KB proof per slot, gossiped about 4 times (REPORT §5.2), is significant on cellular. Prefer submitting once to the delegate and moving to smaller (WHIR) proofs.
6. **Platform skew.** Apple M/A-series dominate the fast tier. Safari's wasm threads need cross-origin isolation like other browsers. Plan the `wasm-bindgen-rayon` path (REPORT phase 2.5) with COOP/COEP and keep the multi-worker fallback.
7. **Telemetry.** FibRace's most useful output is its per-device dataset. umis should log (opt-in, anonymous) device class, `hardwareConcurrency`, `deviceMemory`, job height, prove ms, peak wasm memory, and failure cause. That calibrates the admission rule in item 4.

---

## 4. Recommendation and deciding experiments

**Do not switch now.** The STARK + `p3-recursion` path (REPORT §8, phases 1–3) has working code, the same field and hash throughout, and recursion support. Neither alternative fixes umis' actual bottleneck: proof count × size at the beacon, and shipping states (REPORT §5.2).

**When a sumcheck/GKR prover would beat it.** All of these must hold:
- (i) browser leaf proving stays a design goal, and the wasm prover is the bottleneck after SIMD and threading work;
- (ii) a recursion path exists for sumcheck+WHIR proofs (either `p3-recursion` gains WHIR-based multilinear verification, or aggregation moves to a server-side "whole-world GKR");
- (iii) measured prover time is ≥ 3× better at equal proven security.

The most likely trigger is Plonky3 0.9 multi-stark maturing with recursion support.

**When folding would beat it.** All of these must hold:
- (i) a maintained Rust SuperNeo (or equivalent) exists, with audited Module-SIS parameters;
- (ii) umis accepts a 64-bit field for zone proofs, or someone produces vetted 31-bit parameters;
- (iii) aggregation volume is high enough that recursion-node cost dominates (thousands of leaves per 4 s);
- (iv) an aggregator-centric model is acceptable: the aggregator re-executes and folds, and browsers do not prove leaves.

Today none of (i)–(ii) hold. Revisit in about 12 months.

**Experiments**

- **E1: sumcheck stencil prover (2–3 weeks).**
  - Build the §2.2 layered GKR for one slot and for n_z ∈ {4, 16} on Plonky3 0.9 crates (`p3-sumcheck`, `p3-gkr` for LogUp, `p3-whir`), over KoalaBear⁴.
  - In parallel, port the existing AIR to `p3-multi-stark` as the low-effort baseline.
  - Measure native (AVX2, 1 and 2 threads) and wasm (1 thread) prove time, peak memory, proof size, and verify time at a matched ≥ 100-bit proven configuration (`p3-security`).
  - **Decision:** proceed if wasm prove time is ≥ 3× better than batch-stark at ≤ 1.2× proof size **and** a recursive-verifier cost estimate for the proof (in `p3-recursion` terms) is within 2× of a batch-stark leaf.
- **E2: lattice-folding feasibility (2–4 weeks, research).**
  - Encode the §2.1 CCS (about 2^20 rows, 2^21 digit witness) in Nethermind's `latticefold` PoC (R1CS/CCS front end) or the LatticeFold+ hackathon code.
  - Measure fold time per step, memory, accumulator size, and the size of the fold-verifier circuit.
  - Separately, run the 2026/242 Appendix B.5/B.6 scripts for q = KoalaBear with Φ_81 to see whether any ≥ 100-bit M-SIS parameter set exists for n_F ≈ 2^21.
  - **Decision:** worth a prototype only if fold plus in-circuit verifier is ≤ 1/3 of a measured `p3-recursion` 2-to-1 node, final compression is ≤ 5 s per beacon slot on one server, and a parameter set exists.
- **E3: baseline (needed for phase 3 anyway).**
  - Measure the `p3-recursion` aggregation node (2-to-1, FRI and WHIR inner) on umis leaf proofs: time, memory, output size.
  - E1 and E2 are judged against this.
- **E4: browser telemetry (1 week).**
  - Add the §3 item 7 logging and worker recycling.
  - Run the existing prover on a phone and desktop browser matrix (iOS Safari, Android Chrome mid-range, desktop) to set the admission rule.

---

## 5. Open questions and uncertainties

- Every cost in §2 is a back-of-envelope estimate [E]. None are measured. Constants for ring multiplication and extension-field sumcheck could move results by 3× either way.
- Whether `rem ≥ 0` can be enforced in SuperNeo without booleanity rows (an offset digit encoding) is open [U].
- How to bind `X_in`/`X_out` across folding steps without a hash in the step circuit is open [U].
- Whether `p3-multi-stark` proofs can be recursively verified by `p3-recursion` was not verified [U].
- The FibRace per-bucket crash metric appears mis-stated in the paper (see §1.4).
