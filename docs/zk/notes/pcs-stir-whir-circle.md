# Next-generation PCS / IOPPs for umis: STIR, WHIR, Circle STARKs

Status: research note, 2026-11-03. Scope: whether and where umis should replace FRI.

## Context and conventions

The umis baseline is described in `docs/zk/research/REPORT.md` §3–8. Each zone slot gets its own Plonky3 0.8 `p3-batch-stark` proof with these parameters:

- **Field and code:** KoalaBear, with challenges in the quartic extension EF4 (|EF4| ≈ 2^124). FRI runs at rate ρ = 1/2 (`log_blowup = 1`), arity 2, with 100 queries and 16 query-PoW bits.
- **Trace:** 2^14 rows, LDE 2^15. Committed width is about 46 main + 32 preprocessed + about 56 LogUp base columns, plus 2 EF4 quotient chunks, so roughly 142 base elements per opened row across 4 Merkle trees.
- **Proof and verifier:** 261 KB proof and about 8–11 ms native verify. Native prove is 89–92 ms with AVX2 or 355–419 ms as shipped (2 threads); single-threaded wasm prove is 682 ms (*doc*).
- **Security:** about 66 bits proven at the Johnson bound (JB), 57.5 under unique decoding (UD), and about 101 "conjectured" (`plonky3-internals.md` §4).
- **Plan:** recursive aggregation per 4-second beacon slot (REPORT §8, phase 3).

Conventions used throughout:

- Page numbers are the printed page numbers of the PDFs, which match the PDF page index for all three papers.
- **[P]** means quoted from a paper. **[C]** means read from code in `tmp/zk-research/src`. **[E]** means my estimate, with its assumptions stated next to it.
- "JB" means the Johnson-regime proximity parameter δ = 1 − √ρ − η. "CB" means capacity, δ = 1 − ρ − η. "UD" means δ = (1 − ρ)/2.

**Caveat on "conjectured".** STIR and WHIR advertise their headline numbers under capacity-regime conjectures. Those conjectures were disproven in 2025–26:

- Crites–Stewart 2025/2046 disproved, among others, WHIR's mutual correlated agreement up to capacity.
- Diamond–Gruen 2025/2010 and Krachun–Kazanin–Haböck 2026/782 disproved the related capacity conjectures.

`research-literature.md` §2.1 has the details. **Treat every CB number in this note as unsound for parameter setting.** Use JB or UD.

---

## 1. Papers

### 1.1 STIR: Reed–Solomon Proximity Testing with Fewer Queries

- **Citation:** G. Arnon, A. Chiesa, G. Fenzi, E. Yogev. CRYPTO 2024. ePrint 2024/390, version of 27 Jan 2025.
- **Local copy:** `tmp/zk-research/papers/eprint-2024-390.{pdf,txt}` (59 pp.).

**Protocol** (Construction 5.2, pp. 22–23). STIR is an IOPP for RS[F, L, d] where L is a smooth multiplicative coset and d is a power of 2. Each iteration does the following:

1. Fold f_{i−1} by k_{i−1}, using r^fold.
2. The prover commits g_i, the fold, evaluated on a **new domain L_i of half the size** (L_i = ω·⟨ω²⟩). Because the degree drops by k while the domain only halves, the rate improves from ρ to (2/k)·ρ ("Shift To Improve Rate", p. 7).
3. The verifier sends s out-of-domain (OOD) points. The prover answers with β.
4. The verifier sends t_{i−1} shift queries into L_{i−1}^{k}. The virtual fold of f_{i−1} at those points, together with the OOD answers, defines Ans_i.
5. g_i is quotiented by the set G_i of all these points and degree-corrected (DegCor) to give f_i.

After M = ⌊log_k(d/d_stop)⌋ rounds the prover sends the final polynomial in the clear.

- The folding parameter k must be **≥ 4** and a power of 2 (Thm 5.1, p. 22).
- Rounds: 2M+1. Total proof length: 2Ms + d/∏k_i + Σ(|L_i| + t_{i−1}) (p. 24). Proof query complexity: 2Σt_i over the alphabet F^k.
- **Remark 5.3 (p. 23):** if L_{i−1}^{k} ∩ L_i = ∅, the Fill oracles go away. The recommended L_i = ω⟨ω²⟩ ensures this (footnote 8, p. 28).

**Main theorem** (Thm 1, p. 4; Thm 5.1, p. 22):

- Round-by-round soundness error 2^−λ.
- Rounds O(log_k d), proof length |L| + O_k(log d).
- Queries O(log d + λ·log(log d / log(1/ρ))), compared with FRI's O(λ·log d / log(1/ρ)).
- Required field size: |F| = Ω(λ·2^λ·d²·|L|^3.5 / log(1/ρ)).

**Soundness formula** (Lemma 5.4, p. 24). For δ_i < min{1 − ρ_i − 1/|L_i|, 1 − B⋆(ρ_i)}, with RS[F, L_i, d_i] (δ_i, ℓ_i)-list-decodable:

- ε^fold ≤ err⋆(d_0/k_0, ρ_0, δ_0, k_0)
- ε^out_i ≤ (ℓ_i²/2)·(d_i/(|F| − |L_i|))^s
- ε^shift_i ≤ (1 − δ_{i−1})^{t_{i−1}} + err⋆(d_i, ρ_i, δ_i, t_{i−1}+s) + err⋆(d_i/k_i, ρ_i, δ_i, k_i)
- ε^fin ≤ (1 − δ_M)^{t_M}

Here err⋆ is the BCIKS20 correlated-agreement error (Thm 4.1, p. 16):

- **UD**, δ ≤ (1−ρ)/2: (m−1)·d/(ρ|F|).
- **JB**, δ < 1−√ρ: (m−1)·d² / (|F|·(2·min{1−√ρ−δ, √ρ/20})^7).

The error is linear in d in the UD case and grows as d² times a seventh-power gap term in the JB case.

**Assumptions.** STIR needs plain correlated agreement, which is proven up to Johnson by BCIKS20, and list-decodability up to Johnson. The paper's recommended parameters come in two flavours:

- **Provable, JB** (p. 28): s = 1, t_i = ⌈(λ+1)/(−log(√ρ_i + η_i))⌉.
- **Conjectured, Conjecture 5.6, p. 28:** CA and list decoding up to capacity with c1 = c2 = c3 = 1, and s = 2. **The capacity form is now disproven.**

All benchmarks in the paper use the conjectured setting, for both STIR and FRI (§6.2, p. 31).

**Cost tables** (§6, pp. 30–34):

- **Setup:** 192-bit prime field, λ = 128 with 22-bit PoW, so the IOPP itself targets 106 bits. Merkle trees use SHA3 and Fiat–Shamir uses BLAKE3. Folding is k = 16 for STIR and k = 8 for FRI, which the authors found size-optimal for each (p. 31). Benchmarks were single-threaded on an AWS r6a.
- Table 1 (p. 5) and Table 2 (p. 34) give FRI → STIR:

| d, ρ | Argument size | Verifier hashes | Verifier time | Prover time |
|---|---|---|---|---|
| 2^18, 1/2 | 163 → 114 KiB | 2490 → (1.4k) | 2.0 → 2.9 ms | 1.2 → 2.2 s |
| 2^22, 1/4 | 154 → 94 KiB | 2821 → 1521 | 1.9 → 2.1 ms | 11 → 14 s |
| 2^24, 1/2 | 306 → 160 KiB | 5647 → 2645 | 3.9 → 3.8 ms | 28 → 36 s |
| 2^26, 1/4 | 211 → 114 KiB | — | 2.7 → 2.5 ms | 200 → 250 s |
| 2^28, 1/2 | 430 → 189 KiB | 8479 → 3451 | 5.5 → 4.3 ms | 420 → 640 s |

- Across the grid STIR is **1.25–2.46× smaller** and needs **1.55–2.67× fewer verifier hashes** (p. 32).
- Its native verifier is 0.7× to 1.45× FRI's speed. It is *slower* at small degrees because of the extra algebra.
- The prover is 0.64–0.95× FRI's speed, because STIR runs an FFT per round (p. 33).
- With Poseidon Merkle trees ("algebraic configuration"), the STIR verifier is 1.48–2.34× faster than FRI's (p. 32). **This is the setting that matters for recursion.**
- **Lower rate does not let FRI catch up** (Table 3, p. 33). For d = 2^24, STIR at ρ = 1/4 gives 107 KiB, against 134 KiB for FRI at ρ = 1/8.
- The authors note that when the initial commitment dominates, as in a batched STARK, prover time is "essentially unchanged" (p. 5).

**Relevance at umis' size.** The STIR advantage shrinks at small d: only 1.44× at 2^18 with ρ = 1/2, and umis is at 2^14. The gain is also smaller still once wide trace openings dominate the proof, as they do in umis (§3).

### 1.2 WHIR: Reed–Solomon Proximity Testing with Super-Fast Verification

- **Citation:** G. Arnon, A. Chiesa, G. Fenzi, E. Yogev. EUROCRYPT 2025. ePrint 2024/1586, version of 21 Nov 2024.
- **Local copy:** `tmp/zk-research/papers/eprint-2024-1586.{pdf,txt}` (83 pp.).

**Protocol** (Construction 5.1, pp. 32–33). WHIR is an IOPP for **constrained RS codes**:

CRS[F, L, m, ŵ, σ] = {f ∈ RS[F, L, 2^m] : Σ_{b∈{0,1}^m} ŵ(f̂(b), b) = σ} (Def. 1, p. 5)

RS codewords are read as evaluations of m-variate multilinear polynomials. A multilinear evaluation claim f̂(z) = σ is the weight ŵ = Z·eq(X, z). A univariate evaluation uses z = (z, z², z⁴, …) (p. 7).

Each iteration runs as follows:

1. k sumcheck rounds fold k variables.
2. The prover commits the folded function on a domain of **half size**. This is the STIR rate trick: ρ' = 2^{1−k}·ρ (p. 9).
3. One OOD sample and its answer.
4. t_{i−1} shift queries into L_{i−1}^{(2^k)}.
5. All of these become new eq-constraints that are folded into ŵ with a random γ.

There are no quotients and no degree correction, so the verifier only evaluates eq polynomials. That is the source of its speed.

- Complexity (Table 1, p. 6): queries q = O(λ + (λ/k)·log m), the same as STIR. Verifier field operations O(q·(2^k + m)), compared with STIR's O(q·2^k + λ²·2^k/k). Hash complexity O(q·m).

**Main theorem** (Thm 5.2, p. 34). WHIR assumes the generator (1, α, …, α^{ℓ−1}) has **mutual correlated agreement (MCA)** (Def. 4.9, p. 23) with bound B⋆ and error err⋆. Its errors are:

- ε^fold_{i,s} ≤ d·ℓ_{i,s−1}/|F| + err⋆(C^{(i,s)}_RS, 2, δ_i)
- ε^out_i ≤ 2^{m_i}·ℓ²_{i,0}/(2|F|)
- ε^shift_i ≤ (1 − δ_{i−1})^{t_{i−1}} + ℓ_{i,0}·(t_{i−1}+1)/|F|
- ε^fin ≤ (1 − δ_{M−1})^{t_{M−1}}

**Assumptions.**

- **UD:** MCA is proven in the paper (Lemma 4.10 and Cor. 4.11, p. 23), with B⋆ = (1+ρ)/2 and err⋆ = (ℓ−1)·2^m/(ρ|F|).
- **JB:** MCA is only *conjectured* in the paper (Conjecture 4.12 item 1, p. 25). It has **since been proven**: Haböck ePrint 2025/2110, and Bordage–Chiesa–Guan–Manzur, CCC 2026. Both are cited in `p3/security/src/assumption.rs:12-21` as the justification for `JohnsonBound`. Dao–Kominers–Thaler 2026/2056 (Thm 5.12) tightens the line-MCA count to 8n(m+½)³/(3ρ⁻) exceptional challenges, which p3-security uses.
- **CB:** Conjecture 4.12 item 2 (p. 25). **Disproven** by Crites–Stewart 2025/2046.

**Cost tables** (§6, pp. 43–55). The paper uses k = 4 and m − log ρ − 3 bits of query PoW (p. 44). Merkle trees use BLAKE3 and Fiat–Shamir uses SHA3.

**Table 3 (p. 50): FRI, STIR and WHIR-CB** at λ = 128 over a 192-bit field, single-threaded. Each cell is (FRI, STIR, WHIR):

| d, ρ | Size (KiB) | Verifier time (ms) | Hashes (×10³) | Prover time (s) |
|---|---|---|---|---|
| 2^18, 1/2 | 163, 114, 119 | 2.0, 2.9, 0.7 | 2.5, 1.4, 1.6 | 1.2, 2.2, 0.42 |
| 2^18, 1/16 | 62, 50, 55 | 0.8, 1.0, 0.4 | 1.2, 0.76, 0.89 | 2.7, 2.9, 2.9 |
| 2^24, 1/2 | 306, 160, 157 | 3.9, 3.8, 1.0 | 5.6, 2.6, 2.7 | 28, 36, 34 |
| 2^28, 1/2 | 430, 189, 178 | 5.5, 4.3, 1.2 | 8.5, 3.5, 3.4 | 420, 640, 660 |

- WHIR's verifier is **3.6–4.6× faster** than FRI's and STIR's.
- Its size and hash count match STIR's.
- Its prover is 1.2–1.6× slower than FRI's at large d (p. 49).

**Table 5 (p. 54): UD vs JB vs CB** over Goldilocks³ at λ = 128. This table is the honest one for proven security:

| (m, ρ) | Size UD / JB / CB (KiB) | Verifier time (ms) | Hashes (×10³) |
|---|---|---|---|
| (18, 1/2) | 399 / 228 / 120 | 3.0 / 1.8 / 0.9 | 4.8 / 2.9 / 1.6 |
| (18, 1/16) | 337 / 104 / 56 | 2.4 / 0.8 / 0.4 | 4.9 / 1.6 / 0.9 |
| (24, 1/2) | 621 / 299 / 156 | 4.8 / 2.5 / 1.4 | 10 / 5.1 / 2.7 |
| (24, 1/16) | 535 / 140 / 74 | 4.0 / 1.1 / 0.6 | 10 / 2.7 / 1.4 |

- **Going from CB to JB roughly doubles size and hashes at ρ = 1/2** (p. 53). Lower rate narrows the gap.
- At JB with ρ = 1/16, m = 18, WHIR gives about 104 KiB and 0.8 ms at 128 bits. That is the relevant regime for recursion layers.

**Table 2 (p. 47): λ = 100, Goldilocks²**. Each cell is (UD, CB):

- At m = 18, ρ = 1/2: 249 / 76 KiB and 1.44 / 0.40 ms.
- The abstract's "63 KiB, 360 µs" at 2^22 is the **CB** number (ρ = 1/4, Table 2).

**Prover time** is essentially independent of the regime: 49 / 50 / 47 s single-threaded at (24, 1/2) (p. 53).

### 1.3 Circle STARKs

- **Citation:** U. Haböck, D. Levit, S. Papini. ePrint 2024/278, version of 17 Dec 2024.
- **Local copy:** `tmp/zk-research/papers/eprint-2024-278.{pdf,txt}` (46 pp.).

**Construction.** For a prime p with 2^{n+1} | p+1 ("CFFT-friendly", Def. 1, p. 4), the circle x² + y² = 1 over F_p is a cyclic group of order p+1. For M31, p = 2^31 − 1 and p+1 = 2^31.

- **Domains.** Evaluation domains are *twin-cosets* Q·G_{n−1} ∪ Q^{−1}·G_{n−1}. *Standard-position cosets* are the special case that is itself a coset of G_n (p. 6).
- **Folding.** The squaring map π(x, y) = (2x²−1, 2xy) is 2-to-1 and maps standard-position cosets to standard-position cosets (Lemma 3, p. 7). That gives a circle FFT (§4) and an FRI analogue.
- **Polynomial space.** Polynomials live in L_N(F), bivariate modulo x²+y²−1. The **FFT space L'_N is one dimension smaller** than L_N (p. 3). This dimension gap needs special handling twice:
  - in the quotient decomposition (§5.3);
  - before the low-degree test, where the prover sends λ with f = g + λ·v_n (Protocol 1, step 1a, p. 29).
- **Circle FRI** (Protocol 1, pp. 29–30):
  - The first fold uses the involution J (projection to x) with twiddle y.
  - Later folds use π(x) = 2x² − 1 with twiddle x.
  - The batch version (Protocol 2, p. 30) combines functions with powers λ_0^{k−1}.
- **DEEP.** The DEEP point γ is sampled on C(F) \ D over an extension F (p. 25). The real and imaginary parts of the DEEP quotients are both low-degree tested, which doubles the batch at negligible cost (Remark 23, p. 30).

**Soundness** (Thm 6, p. 38; summary p. 30):

ε_PROX = (L − ⅔)·(m+½)^7/(3·ρ_−^{3/2})·|D|²/|F| + (2m+1)(|D|+1)·r/(√ρ_−·|F|) + α^s, with α = (1 + 1/(2m))·√ρ and m ≥ 3.

For the AIR (Thm 7, p. 42; summary p. 24):

ε_AIR = ℓ⁺·((C−1)/|F| + d(N+2)/(|F| + 1 − 2^{n+B})) + ε_PROX

- **The paper analyses only the Johnson regime.** It uses the BCIKS20 CA theorem, "taken over" to circle codes (App. A, p. 34), and gives no conjectured variant.
- The circle code rate is ρ = 2^{−B}·(1 + 1/N) (p. 28), slightly off 2^{−B}.
- **Soundness is in the oracle model only.** The BCS/Fiat–Shamir treatment is "postponed to a future version" (Remark 19, p. 24), and no round-by-round proof is given. ethSTARK-style RBR arguments are expected to carry over, but that is not written down there.

**Costs.**

- **The paper has no end-to-end STARK benchmark.** It benchmarks the FFT only (App. D, Table 1, pp. 45–46). For a batch of 128 polynomials on an i7 with AVX2:
  - single-threaded, M31 CFFT vs BabyBear FFT: 11.0 vs 17.6 ms at 2^14 (1.6×), and 1.4× at 2^16–2^22;
  - with 8 threads: 1.3× at 2^14 and about 1.0× from 2^16 up (memory-bound).
- Proof size and query count are the same as FRI at equal rate and field. Circle FRI *is* FRI on a generalized RS code, so the gain is prover arithmetic only.

---

## 2. Implementation status

### 2.0 Repository caveat

The vendored `tmp/zk-research/src/p3` has workspace version **0.8.0** (`p3/Cargo.toml`) but already contains `stir`, `whir`, `security`, `multi-stark` and `zk-codes`, which first appear on crates.io in 0.9.0-rc.

- The snapshot comes from this repo's own two commits. Treat it as "Plonky3 main as of Oct 2026".
- `p3r` pins **crates.io 0.9.0-rc.2** (Cargo.lock), not this tree.

### 2.1 Plonky3

| Crate (path) | Entry points [C] | Implements | Wired into | Maturity |
|---|---|---|---|---|
| `p3-fri` (`p3/fri`) | `FriParameters` (`config.rs:34`: log_blowup, log_final_poly_len, max_log_arity, num_queries, batch/commit/query PoW bits); `TwoAdicFriPcs`, `HidingFriPcs` | `Pcs`, `UnivariateStarkPcs` (`two_adic_pcs.rs:526,1019`) | uni-, batch-stark (production path) | 139 tests, 3 TODOs; Least Authority audit Jul 2024 (`p3/audits/`) |
| `p3-stir` (`p3/stir`) | `StirParameters` (`config.rs:63`: log_blowup, log_folding_factor ≥ 2, log_starting_folding_factor ≥ 2, soundness_type, security_level, max_pow_bits, mmcs); `StirOptions` (max_log_final_poly_len, compact_answers); `prove_stir[_multi]` (`prover.rs:44,1284`), `verify_stir[_multi]` (`verifier.rs:1111,1327`); `TwoAdicStirPcs` (`pcs.rs:309`) | `Pcs` (`pcs.rs:561`), `UnivariateStarkPcs` (`pcs.rs:670`) | uni-stark (`uni-stark/tests/stir_fibonacci.rs`). Type-level compatible with batch-stark, which uses the same `StarkGenericConfig`, but untested there | 295 tests, 0 TODOs, **unaudited**. Documented deviations from the paper: a prover-assisted Ans check and a fixed s schedule (`lib.rs`). PR #2100 fixed a STIR case that delivered 86.76 bits against a 100-bit target |
| `p3-whir` (`p3/whir`) | `ProtocolParameters` (`parameters/mod.rs:22`: starting_log_inv_rate, round_log_inv_rates, folding_factor ∈ {Constant, ConstantFromSecondRound, PerRound}, soundness_type, security_level, pow_bits; `DEFAULT_MAX_POW = 16`); `WhirConfig` (`whir.rs:232`: commitment_ood_samples, per-round num_queries / ood_samples / pow_bits / folding_pow_bits) | **`MultilinearPcs`** (`pcs/adapter.rs:67`), `PrescribedPointPcs`; `HidingWhirPcs` (`pcs/zk/adapter.rs`) | multi-stark (dev-dep), binary PCS, p3-recursion. **Not** a `UnivariateStarkPcs`, so not usable with batch-stark | 250 tests, unaudited. Bench `whir/benches/fri_vs_whir.rs` exists, but no numbers are recorded |
| `p3-circle` (`p3/circle`) | `CirclePcs { mmcs, fri_params }` (`pcs.rs:45`), `cfft.rs`, `deep_quotient.rs`, `folding.rs` | `Pcs` (`pcs.rs:243`), `UnivariateStarkPcs` (`pcs.rs:1199`) | uni- and batch-stark tests (`batch-stark/tests/common/config.rs:7,154`) | 83 tests, 2 TODOs. `fri/src/config.rs:36`: `log_final_poly_len` and early stopping are "not yet implemented in `CirclePcs`". The 2024 audit says circle-FRI was "not completed yet" |
| `p3-mersenne-31` | `Mersenne31`, `CIRCLE_TWO_ADICITY = 31` (`complex.rs:53`) | — | — | Packing for NEON, AVX2 and AVX-512. **No wasm simd128** |
| `p3-security` (`p3/security`) | `SecurityAssumption {UniqueDecoding, JohnsonBound, CapacityBound}` (`assumption.rs:39`; JB uses η = √ρ/20, CB uses η = ρ/20); FRI: `fri.rs` (`proven_error_udr`, `proven_error_ldr_m`, `best_ldr_m`, …); WHIR/STIR: `whir/mod.rs` (`determine_ood_samples`, `queries_combination_error`, `folding_pow_bits`); `stark.rs` (`proven_security*`) | — | multi-stark (`prove_with_security`) | 188 tests. **Has no Circle-specific soundness model** |
| `p3-multi-stark` | `prove` / `verify`, `prove_with_security` / `verify_with_security` (`verifier.rs:123,228`) | — | — | 681 tests, unaudited; SuperSpartan-style zerocheck, LogUp-GKR buses |
| `p3-zk-codes` | ZK RS codes (HVZK-WHIR, ePrint 2026/391 §3.2.1) | — | whir | 26 tests |

**How `TwoAdicStirPcs` handles wide traces** (`stir/src/pcs.rs:1-74`). This matters a lot for umis.

- It alpha-batches DEEP quotients per height class and merges classes via §4.5 `Combine`.
- It **commits the combined codeword itself**. Each round-0 query then opens the input matrices at **one uniformly chosen lane of the k_0-fiber, one row each**, not k_0 rows.
- That binding requires *mutual* CA for the round-0 fold. JB provides it. Under CB the PCS assumes more than the regime states (as the code itself notes).
- Without this trick a k_0 = 16 STIR would open 16 rows × 142 columns per query.

**Two-adicity.** KoalaBear has TWO_ADICITY = 24, so a single LDE domain is at most 2^24 (trace ≤ 2^{24−log_blowup}). `stir/src/config.rs:905` rejects `log_starting_domain > F::TWO_ADICITY`. That is ample for a 2^15 leaf and for p3r layers.

**wasm.**

- All four PCS crates are `no_std` with optional `p3-maybe-rayon`, so they compile to `wasm32-unknown-unknown` single-threaded just like FRI.
- STIR and WHIR do a small FFT per round. For a 2^15 initial domain these are 2^14 and below, which costs little compared with the trace LDE.
- The real wasm bottleneck is unchanged: no KoalaBear simd128 packing (REPORT §6).

### 2.2 Plonky3-recursion (`tmp/zk-research/src/p3r`, v0.1.0, unaudited)

**In-circuit verifiers [C]:**

- **FRI:** `recursion/src/pcs/fri/verifier.rs:1662` `verify_fri_circuit`, with `max_log_arity` support.
- **WHIR:** `recursion/src/pcs/whir/verifier.rs:467` `verify_whir_circuit`, plus `whir/sumcheck.rs`, `whir/uni/` (`WhirUniPcs`, a univariate adapter) and binary-field WHIR.
- **STIR: none.** "STIR" in `whir/params.rs` refers only to WHIR's round terminology.
- **Circle: none.**

**STARK verifiers:**

- `verify_p3_uni_proof_circuit` (`verifier/stark.rs:197`).
- `verify_p3_batch_proof_circuit` and `verify_batch_circuit` (`verifier/batch_stark.rs:646,904`).
- Multi-stark WHIR verifiers for binary fields.

**Selection and parameters:**

- Examples pick the PCS with `--pcs fri|whir` (`examples/common/whir.rs:9`).
- Built-in suites live in `recursion/src/builtin_config/{fri,whir}.rs`. The WHIR built-ins are KoalaBear or BabyBear, D4, Poseidon2, Merkle arity 2, constant folding factor, non-hiding (`book/src/user_guide/configuration.md:56-57,115`).

**Security pitfall in the examples [C]:**

- FRI query counts come from `checked_num_queries = (security_level − query_pow)/log_blowup` (`examples/common/mod.rs:179`). That is the **ethSTARK "legacy" conjectured formula**.
- The WHIR example hard-codes `SecurityAssumption::CapacityBound` (`examples/common/whir.rs:321`).
- **Neither is proven security.** The default `--security-level 124` therefore corresponds to roughly 62 proven bits at log_blowup 2 under FRI-JB. Some binary examples use `JohnsonBound` (`native_binary_recursion.rs:332`, `binary_prover.rs:102`).

**Benchmarks** (`book/src/appendix/benchmark.md`): M4 Pro, 14 cores, KoalaBear D4, 124-bit target, default FRI, "early stage", prover time only:

| Workload | Base proof | 1st layer | 2nd+ layers |
|---|---|---|---|
| 2-to-1 aggregation | 13.9 ms | 85 ms | 174 ms |
| 8-to-1 aggregation | 13.9 ms | 229 ms | 641 ms |

The book records no circuit sizes or proof sizes.

### 2.3 Stwo (`tmp/zk-research/src/stwo`, v2.2.0 workspace)

- **Status:** README calls it "production-grade". Development moved to `starkware-libs/proving` from the end of July 2026. It needs toolchain `nightly-2026-01-15`, and the `prover` feature requires `portable_simd` (`crates/stwo/src/lib.rs:8`).
- **Core types:**
  - `CirclePoint` (`core/circle.rs:13`; `M31_CIRCLE_LOG_ORDER = 31` at `:185`);
  - `CanonicCoset` (`core/poly/circle/canonic.rs:39`);
  - `CircleDomain` (`core/poly/circle/domain.rs:19`);
  - FRI in `core/fri.rs` (`FriVerifier`, `fold_circle_into_line`, `fold_line`) and `prover/fri.rs`;
  - PCS in `core/pcs/` and `prover/pcs/`;
  - `prove` at `prover/mod.rs:29` and `verify` at `core/verifier.rs:18`.
- **`PcsConfig`** (`core/pcs/mod.rs:31`): pow_bits plus `FriConfig` (log_blowup 1..=16, log_last_layer_degree_bound 0..=10, n_queries, fold_step).
- **Security estimate.** The only one is `security_bits() = pow_bits + log_blowup·n_queries`, the legacy conjectured formula. **The default config (pow 10, blowup 1, 3 queries) gives about 13 bits and is test-only.**
- **Backends:** `cpu` and `simd`. simd is built on `std::simd` u32x16 and is portable, so it lowers to wasm simd128 when built with `+simd128`. There is no explicit wasm cfg; wasm32 only disables threads and grinding.
- **Recursion:** none in this repo. Recursion goes through the external Cairo verifier (`stwo-cairo`).

---

## 3. Fit for umis

### 3.1 Scorecard

Scores run from 1 (poor) to 5 (best) for umis today.

| | (a) Leaf prover (browser) | (b) Beacon verify | (c) Recursion in-circuit | (d) Proven-security parameters | Integration cost |
|---|---|---|---|---|---|
| **FRI** (`TwoAdicFriPcs`) | **5**: current path; FRI is about 10% of prove time (38 ms of about 400 ms scalar) | 3: about 8–11 ms native, 100+ queries at JB | **4**: `verify_fri_circuit` exists; most hashes per query | 4: p3-security models it fully (UD and JB); ceiling about 100–108 bits on EF4 | none |
| **STIR** (`TwoAdicStirPcs`) | 4: same commit cost plus small per-round FFTs; paper prover 0.64–0.95× FRI speed on the IOPP alone, about unchanged end-to-end [E] | 4: 1.5–2.7× fewer hashes; native time about equal at small d (p. 34) | **1**: **no in-circuit verifier** in p3r | 4: JB proven (CA plus MCA for the PCS lane binding); p3-security plus `stir/soundness.rs` | low: swap the PCS type in the batch-stark config (untested in batch-stark) |
| **WHIR** (`WhirProver`, multilinear) | 2: requires moving the AIR to `p3-multi-stark` (rewrite of config and glue; REPORT §7) | **5**: 3.6–4.6× faster verify than FRI/STIR (p. 49) | **5**: `verify_whir_circuit` exists; fewest field operations and hashes | 4: JB is now proven (MCA, Hab25/BCGM26). **The p3r examples default to CB, which is disproven** | high at the leaf; low inside p3r (`--pcs whir`) |
| **Circle / M31** (`CirclePcs`, Stwo) | 4: 1.3–1.6× faster FFT; Stwo has a portable-SIMD path usable from wasm; but the AIR, field, DSL and all tooling must be ported | 3: FRI-like | 1: no Plonky3 in-circuit verifier; Stwo recursion only via Cairo | **2**: oracle-model JB analysis only, no p3-security model, Stwo's estimate is the legacy formula | very high |

### 3.2 Estimated numbers for the umis leaf at about 100 proven (JB) bits on KoalaBear⁴ [E]

**Assumptions:**

- Same 2^14 trace and about 142 opened base elements per row in 4 trees.
- Pruned multiproofs and 32-byte digests.
- Query PoW of 16–20 bits.
- Per-query JB soundness of −log₂(1.05·√ρ) bits (p3-security's η = √ρ/20), which is 0.43 / 0.93 / 1.43 / 1.93 bits for log_blowup 1 / 2 / 3 / 4.
- Required t_0 = ⌈(100 − 16)/that⌉, giving **196 / 91 / 59 / 44 queries**.
- FRI sizes are anchored on the p3-security proof-size grid in `plonky3-internals.md` §4: lb2 q100 at 231 KB, lb3 q64 at 177 KB, lb4 q50 at 158 KB.

| Leaf config | Queries | Proof | Verify (native) | Prover vs today |
|---|---|---|---|---|
| FRI lb1 (today's rate) | about 196 | about 470–500 KB | about 18 ms | 1.0× |
| FRI lb2, arity 4, final poly 8 | about 91–100 | **about 220–235 KB** | about 9 ms | about 1.6–2× (LDE and hash double) |
| FRI lb3 | about 59–64 | about 175 KB | about 7 ms | about 3× |
| STIR lb2, k_0 = 4, k = 16 | t_0 about 91, then about 35 / 22 | **about 150–170 KB** | about 7–9 ms | about FRI lb2 + 5% |
| STIR lb3 | t_0 about 59, then about 30 / 20 | about 115–130 KB | about 6 ms | about FRI lb3 + 5% |

The STIR saving at umis' size comes only from replacing FRI's fold layers, which are roughly 40–45% of today's proof. The t_0 input-row openings (about 1.3–1.5 KB per query) are identical for every IOPP at the same rate and dominate.

Net, STIR is about **0.65–0.75× FRI** for umis, not the paper's 2× (which holds at 2^24+ and narrow inputs). Measure this with `p3/examples/examples/pcs_comparison.rs` before relying on it.

### 3.3 Recursion and in-circuit cost [E]

The in-circuit verifier is dominated by Poseidon2 permutations: Merkle paths plus leaf hashing.

**Per verified umis-shaped leaf at JB, about 100 bits, lb2:**

- **FRI:**
  - input openings: about 91 queries × (4 trees × about 9 non-pruned levels + about 18 perms to hash a 142-element row);
  - fold layers: about 6 layers × about 10 perms;
  - total **about 13–15k Poseidon2 permutations**, plus about 91 × 6 EF4 fold evaluations.
- **WHIR or STIR:** the same input part (about 7–8k perms), plus about 60 queries over 2–3 rounds × about 12 perms, for **about 8.5–9.5k perms**. That is about 35–40% fewer.
- **WHIR's field work** is O(q·(2^k + m)), roughly 2–4k EF4 multiplications, compared with STIR's quotient and degree-correction algebra.
- The paper's algebraic (Poseidon) setting shows the same direction: STIR verifies 1.48–2.34× faster than FRI (STIR p. 32), and WHIR hashes match STIR's (WHIR p. 49).

**Inner layers.** The proof being verified is the recursion circuit's own proof (p3r tables), not the umis AIR. Here lower rate pays off directly, because the prover can afford it natively. At lb3–4 with JB:

- WHIR needs about 45–60 initial queries and about 15–25 per later round.
- Expect about 100–130 KB per inner proof and **sub-millisecond to about 2 ms native verify** [E, scaled from WHIR Table 5, p. 54, JB at ρ = 1/8–1/16: 120–140 KiB / 0.9–1.1 ms at λ = 128, m = 18–24].

**Aggregation time** [E, from the p3r benchmark, which uses conjectured parameters; proven parameters roughly double query counts]:

- 2-to-1 layers cost about 85–175 ms each natively (14 cores).
- 11 leaves per slot today need about 4 tree levels.
- 128 zones × about 4 leaves is about 512 leaves, about 9 levels, and with parallel aggregators **about 1–2 s critical path**. That fits a 4-second slot, but only on native multi-core aggregators.

### 3.4 Recommendation

| Layer | PCS | Parameters (starting point) | Why |
|---|---|---|---|
| **Leaf** (per-slot or per-commitment, browser and native, batch-stark) | **FRI now** (`TwoAdicFriPcs`). Optionally **STIR later** once p3r can verify it or the leaf stops being recursed | `log_blowup = 2`, about 91–100 queries, query PoW 16–20, batch/commit PoW 8–16, `max_log_arity = 2`, `log_final_poly_len = 3`, gated by `p3-security` JB at ≥ 100 bits | Recursion needs an in-circuit verifier, and p3r has only FRI and WHIR. WHIR at the leaf means a multi-stark rewrite. STIR's leaf saving is about 25–35% [E], but it then has to be *verified in-circuit* by layer 1, which p3r cannot do |
| **Inner aggregation** (2-to-1 or 4-to-1, native aggregators) | **WHIR** (`--pcs whir` / `WhirRecursionBackendForExt`) | `SecurityAssumption::JohnsonBound` (**not** the example default CB); `starting_log_inv_rate` 3–4; folding factor 4; per-round PoW ≤ 16–18; KoalaBear D4 (D5 if targeting 128) | Smallest in-circuit verifier (fewest hashes and field operations), fastest native verify, JB proven. Its prover overhead is paid by native machines |
| **Outer** (one proof per beacon slot, verified by every beacon validator, possibly in wasm) | **WHIR-JB** at low rate (lb 3–4) | As above | Beacon cost is one verify per 4 s: about 1–2 ms native, or about 5–15 ms wasm [E], at about 100–130 KB. Use **FRI** instead only if a later EVM or zkVM wrap needs the most-reviewed verifier |
| **Circle / Stwo** | Not now | — | No Plonky3 recursion, no p3-security model, and full port cost. Revisit only if browser leaf proving stays the bottleneck after KoalaBear simd128 work (REPORT §6) |

**Expected end state** [E]:

- Per beacon slot: one proof of about 100–130 KB, verified in about 2 ms natively.
- Today: about 2.9 MB of proofs and about 120 ms of wasm verification.
- Leaves stay about 230 KB (FRI lb2), or about 160 KB with STIR, and become gossip-only between prover and aggregator.

---

## 4. Must-not-get-wrong list

### Security regime

1. **Never deploy `SecurityAssumption::CapacityBound`.** It is disproven (Crites–Stewart 2025/2046 disproves WHIR's mutual CA to capacity; Krachun–Kazanin–Haböck 2026/782 gives a counterexample on multiplicative subgroups of prime fields).
   - p3r's WHIR example (`examples/common/whir.rs:321`) and its tests use CB.
   - The p3r FRI examples use the legacy `(λ − pow)/log_blowup` formula.
   - Stwo's `security_bits()` is the same legacy formula.
   - Re-derive every count with `p3-security` at `JohnsonBound` (or `UniqueDecoding`).
2. **JB for WHIR, and for STIR's PCS lane binding, relies on *mutual* CA.** That was a conjecture in the WHIR paper (Conj. 4.12, p. 25) and was proven later (Hab25 / BCGM26, cited in `security/src/assumption.rs`). Pin the p3-security version whose JB formula uses these results (DKT26 line-MCA). Older WHIR parameter tables used `(ℓ−1)·2^{2m}/(|F|·(2·min(·))^7)` (p. 23), which is far more pessimistic.
3. **The extension field caps everything.**
   - The JB proximity-gap terms scale with n/|F| or worse. On KoalaBear⁴ (about 2^124) with an 8-element digest, about 100–108 proven bits is the practical ceiling.
   - 128 bits needs KoalaBear⁵ (`p3r` has a quintic option) and a digest of 9 or more elements.
   - **The Circle test config in `batch-stark/tests/common/config.rs:155` uses M31³ (about 2^93), which is far too small.** Use QM31 (degree 4) at minimum.
4. **The query PoW budget is not the same at every site.** p3-stir gives no PoW credit to OOD and Ans-check errors (`config.rs` docs on `max_pow_bits`). WHIR has separate `pow_bits` and `folding_pow_bits` per round. Check `SecurityReport` per term, not just the minimum.

### STIR-specific

5. **The folding factor must be ≥ 4** (paper Thm 5.1, p. 22; `log_folding_factor ≥ 2` in code). k = 16 was size-optimal at 2^18–2^30 (p. 31). For a 2^14 trace, k = 16 gives only about 2 rounds, so try k ∈ {4, 8, 16}.
6. **Keep `log_starting_folding_factor` small (2) for wide traces.** The PCS opens one input row per query, but the committed round-0 fiber is 2^{k_0} EF elements per query.
7. **Domains.** L_i = ω·⟨ω²⟩ must be disjoint from L_{i−1}^{k} (Remark 5.3, p. 23), or the Fill oracles and their checks are needed. Each round's starting domain must fit within the two-adicity (KoalaBear: 24; `config.rs:905`).
8. **Use s = 1 OOD sample under JB in the paper's provable setting** (p. 28). The code picks OOD counts from the field size (`determine_ood_samples`). With |EF4| about 2^124 and large list sizes ℓ, verify that ε^out = ℓ²/2·(d/(|F|−|L|))^s meets the target.
9. **Height classes.** `TwoAdicStirPcs` merges different trace heights onto one domain (`with_max_log_height_spread`). A short matrix in a tall group pays a `2^{H−h+b}` blowup. Opening points must lie outside the shared LDE coset, and extraction is to degree ≤ d, not < d (`pcs.rs` header).

### WHIR-specific

10. **WHIR is multilinear** (`MultilinearPcs`). It does not plug into batch-stark or uni-stark. The univariate adapter `WhirUniPcs` exists only in p3r.
11. **Set the folding schedule explicitly** with `FoldingFactor::{Constant, ConstantFromSecondRound, PerRound}`. The p3r built-in descriptor supports only a constant k. The paper's verifier-time results use k = 4 (p. 44).
12. **The rate schedule matters.** `round_log_inv_rates` sets each round's rate. The paper rule is ρ' = 2^{1−k}·ρ. Starting at lb1 doubles JB size versus CB (Table 5, p. 54). For recursion layers start at lb 3–4.
13. **Use one OOD sample per round and `commitment_ood_samples` for the initial commitment.** Both are field-size dependent. On D4 KoalaBear, check ε^out = 2^{m_i}·ℓ²/(2|F|) per round.
14. **HVZK-WHIR (`HidingWhirPcs`, `p3-zk-codes`) is separate from the non-hiding WHIR.** umis does not need hiding today, so do not enable it by accident: the masking budget changes the parameters.

### Circle-specific

15. **The prime must be CFFT-friendly:** 2^{n+1} | p+1 (Def. 1, p. 4). For M31 that allows domains of at most 2^30 (`CIRCLE_TWO_ADICITY = 31`). KoalaBear and BabyBear are *not* circle fields; Circle STARKs are for M31.
16. **Twin-coset vs standard-position domains.** The trace domain H and the evaluation domain D must be disjoint standard-position cosets for the zk variant (§5). The non-zk "D ⊇ H" variant puts D in group position and needs rotated coordinates (App. C, pp. 43–44).
17. **The dimension gap.** L'_N (FFT space) is one dimension smaller than L_N. The prover must send λ with f = g + λ·v_n before folding (Protocol 1, p. 29), and the quotient needs the limit-at-infinity handling (§5.3). Getting this wrong breaks completeness or soundness.
18. **Circle FRI fold order.** Fold 1 uses J (twiddle y), and later folds use π(x) = 2x²−1 (twiddle x). The code rate is 2^{−B}·(1 + 1/N), not exactly 2^{−B} (p. 28).
19. **The DEEP point must be on C(F) \ D over the extension**, and both the real and imaginary parts of the quotients must be tested (Remark 23, p. 30). The AIR error has the term d·(N+2)/(|F| + 1 − 2^{n+B}) (Thm 7).
20. **The proof is in the oracle model only** (Remark 19, p. 24). There is no RBR or BCS proof in the paper, and p3-security has no Circle model. Do not claim proven bits for a Circle config without an external analysis.
21. **`CirclePcs` lacks `log_final_poly_len` and early stopping** (`fri/src/config.rs:36` TODO). Proofs are therefore larger than `TwoAdicFriPcs` at equal parameters.

### Process

22. **Unaudited.** stir, whir, multi-stark, security, zk-codes and p3r postdate the only audit (Least Authority, Jul 2024), and circle was out of its scope. Budget an audit before any value-bearing deployment.
23. **Version skew.** p3r verifies 0.9.0-rc.2 proofs, and the vendored p3 is labelled 0.8.0. Transcripts changed incompatibly in 0.9 (typed DSFS), so leaf and aggregator must be built from the same Plonky3 revision. Re-pin golden vectors.
24. **Bind the statement.** The recursion circuit and the per-slot AIR must be fixed and absorbed into the transcript (Fiat–Shamir and GKR caveats in `research-literature.md` §2.3). This matters more once WHIR's sumcheck rounds and LogUp-GKR enter the circuit.
25. **Measure before adopting.**
    - `p3/examples/examples/pcs_comparison.rs` and `p3/whir/benches/fri_vs_whir.rs` exist, but no results are recorded anywhere. Run them on the umis shape: 2^14 rows, about 142 columns.
    - Run the p3r `--pcs fri|whir` aggregation at JB parameters, natively and in wasm, before fixing the phase-3 design.
