# Reed–Solomon proximity gaps and list decoding: what is refuted, proven or open (status Nov 2026)

Status: research note, 2026-11-03. Scope: what the 2025–26 results on RS proximity gaps, correlated agreement (CA), mutual correlated agreement (MCA) and list decoding mean for the soundness numbers umis may quote.

## Context and conventions

- **umis baseline.** Plonky3 0.8 `p3-batch-stark` over KoalaBear, p = 2^31 − 2^24 + 1 (two-adicity 24). Challenges come from the quartic extension EF4, |EF4| ≈ 2^124. FRI runs at rate ρ = 1/2 (`log_blowup = 1`) with arity 2, 100 queries and 16 query-PoW bits. The trace has 2^14 rows and the LDE has 2^15. The FRI evaluation domains are cosets of two-adic multiplicative subgroups of F_p^×.
- **Current numbers** (`tmp/zk-research/secprobe/out.txt`, `new_benchmark`):
  - proven 65.9 bits (Johnson / list-decoding regime, query-limited), 57.5 bits under unique decoding;
  - "conjectured" 101.4 bits, limited by batching. Its terms are 113.2 bits for the query phase and 109.0 for the commit phase.
- **Tags.**
  - **[P]** stated in a paper, with theorem and page.
  - **[C]** read from code in `tmp/zk-research/src/p3/security`.
  - **[D]** my derivation from a paper's construction, with the assumptions stated.
  - **[E]** numerical estimate, from the script described in §5.
- **Page numbers** are PDF page indices. They match the printed numbers for every paper here.
- **Agreement and distance.** "Agreement a" is the fraction of positions where a word matches a codeword. Relative distance is θ = 1 − a. Capacity is a = ρ, Johnson is a = √ρ, and unique decoding is a = (1 + ρ)/2.
- **Bits per query.** A query costs −log₂ a bits. Capacity gives log₂(1/ρ), and Johnson gives ½·log₂(1/ρ).

---

## 1. Timeline and one-glance status

### 1.1 Timeline

| When | Paper | What happened |
|---|---|---|
| 2019–20 | DEEP-FRI (BGKS20); BCIKS20 (2020/654, Conj. 8.4) | Conjectures posed: RS list decoding up to capacity with poly(n/η) lists, and proximity gaps / CA up to capacity with error poly(n)/((ρη)^c·q). |
| 2021 | ethSTARK (2021/582) | Uses the capacity conjecture: bits ≈ Q·log₂(1/ρ) + PoW. |
| 2024 | STIR Conj. 5.6; WHIR Conj. 4.12 | Same conjectures for STIR, and MCA up to capacity for WHIR. |
| late 2025 | **Diamond–Gruen** (2025/2010) | BCIKS Conj. 8.4 is false for all fields and domains, with families whose rate → 0. Proposes the "random-words" cutoff and an amended conjecture (Conj. 5.1). |
| 6 Nov 2025 | **BCHKS** (2025/2055) | Proven Johnson-regime gaps with O(n) exceptions instead of O(n²). Refutes the n^τ conjecture for every τ in characteristic 2. Shows gaps beyond the list-decoding radius cost ≥ 1/(2n). Conditional prime-field examples. |
| 19 Dec 2025 | **Crites–Stewart** (2025/2046) | Refutes up-to-capacity CA (BCIKS), MCA (WHIR) and list decoding (DEEP-FRI). Proves "CA with small error ⇒ list decoding". Gives entropy-corrected conjectures, including an extension-field variant that uses H_p of the characteristic. |
| Apr 2026 | **Krachun–Kazanin–Haböck** (2026/782) | Explicit counterexample on **multiplicative subgroups of prime fields**, the STARK setting. At gap η = Θ(1/log n) from capacity, lists and bad-line counts are ≥ n^τ. |
| 9 Apr 2026 | **Kambiré** (arXiv 2604.09724) | Self-contained proof of the Krachun–Kazanin sketch. |
| 5–7 Sep 2026 | **Jeronimo** (2609.05870); **BCPZZ** (2609.08005) | Proven *asymptotic* list decoding up to capacity (BCPZZ, Jeronimo) and MCA up to capacity (Jeronimo) for any fixed gap γ, over prime fields and large characteristic. Constants are not effective. |
| Sep–2 Nov 2026 | **Dao–Kominers–Thaler** (2026/2056) | Explicit, Lean-checked bounds. Johnson MCA count O(n/η³). First-order beyond-Johnson threshold a₁(ρ) < √ρ, valid when char > k−1. Uniform capacity bounds. First deployed proof-size savings (4.6–11%). |

### 1.2 Status table

| Claim | Status | Where | Fields and domains it covers |
|---|---|---|---|
| **ethSTARK / BCIKS Conj. 8.4**: gaps or CA up to δ = 1−ρ−η with error ≤ n^{c₂}/((ρη)^{c₁}·q) for universal c₁, c₂ | **Refuted** | DG Thm 4.14 p.22 (all MDS codes, so every field, characteristic and domain; rate → 0, q = n^{c*+1}). CS Cor. 1 p.13 + Thm 1 p.8 (prime fields, beyond list-decoding capacity 1−H_q(δ)). KKH Thm 1 p.3 / Prop. 3 p.15 (prime fields, **multiplicative 2-power subgroups**). BCHKS Thm 1.6 p.10 (characteristic 2). | Everything, including KoalaBear-type smooth subgroups |
| **"n^τ-bounded" gaps** (BCHKS Conj. 1.2 p.6): a = O_η(n^τ) for some fixed τ, error allowed to grow arbitrarily in 1/η | **Refuted in char 2** (BCHKS Thm 1.6 p.10, Cor. 1.7 p.11). **Refuted near capacity in prime fields** with a shrinking gap η = Θ(1/log n) (KKH Thm 1 p.3; Kambiré Thm 1 p.2). Open at a fixed gap η over prime fields. | as listed | char 2 (random F₂-subspace domains); prime-field 2-power subgroups |
| **List decoding to capacity** with poly(n/η) list (DEEP-FRI Conj. 2.3, STIR Conj. 5.6) | **Refuted as stated.** It fails beyond 1 − H_q(δ), Elias (CS p.7–8). For extension fields with the domain in a subfield, it fails beyond the subfield's entropy (CS Cor. 2 p.19). KKH Prop. 2 p.10 gives n^τ lists at η = Θ(1/log n) on prime-field subgroups. | CS; KKH | all |
| **List decoding at a fixed gap** γ from capacity, list n^{O_γ(1)} | **Proven**, asymptotically: BCPZZ Cor. 1.2 p.3 (prime q ≥ Cn); Jeronimo Thm 1.1 p.3 (prime q ≥ n); DKT Thm 1.2 p.6 (char 0 or p > k−1, any n distinct points, n ≥ N(δ), list C_δ·n^{d(δ)} with d(δ) = ⌈e^{1.5/δ}⌉). | as listed | prime fields / large characteristic. KoalaBear (p > k) is covered by DKT and by Jeronimo §8.4. Constants are huge. |
| **MCA up to capacity** (WHIR Conj. 4.12(2)) | **Refuted** as stated (CS p.15–16) | CS | all |
| **MCA at a fixed gap** γ from capacity | **Proven**, asymptotically: Jeronimo Thm 1.2 p.4 (error n^{C(γ)}/q); DKT Thm 1.2 p.6 (C_δ·n^{d(δ)+1}/q) | as listed | prime fields / char > k−1. Not usable for parameters (§3.3). |
| **CA / MCA up to Johnson** | **Proven, every field.** BCIKS20 (O(n²)); **BCHKS Thm 1.5 p.9 (O(n/η⁵))**; Haböck 2025/2110 (MCA); **DKT Thm 4.4 p.29: exceptional count < 8n(m+½)³/(3ρ⁻), O(n/η³)**. Plonky3 0.8 uses this count [C]. | as listed | every characteristic and every domain |
| **Beyond Johnson, first order**: agreement a₁(ρ)+η₁ | **Proven**: DKT Thm 1.1 p.6, Thm 5.1/5.2 p.30–31. List O(n/η₁²), MCA count O(n²/η₁⁴), needs char p > max{k−1, B_∂}. | DKT | any n distinct points. **KoalaBear qualifies** (p ≈ 2^31 > k). Blocked in practice by n²/q on EF4 (§3.3). |
| **Beyond Johnson in char 2** | **Refuted** (DKTZ26, 2026/2424, cited in DKT p.71: superpolynomially many bad challenges approaching Johnson at rate 1/4 on additive domains) | DKTZ26 | binary fields. Not umis. |
| **Diamond–Gruen "random words" cutoff**: η_RW ≈ ρ·log₂(e/ρ)/log₂q. This is what `p3-security` calls "conjectured". | **Heuristic, not a theorem.** DG present it as advice: stay above it if you must use conjectures (p.6). DG's *formal* amended conjecture (Conj. 5.1 p.23) needs η = Ω(1) and allows constants K(ρ,η), so it says nothing about η_RW for concrete n and q. KKH-type lines (§3.2 [D]) put the commit-phase error at the RW radius far above the n/q that `p3-security` assumes for EF4 and 2-adic domains. | DG §1.5 p.5–6; `p3-security` `fri.rs` [C] | Applied with q = |EF4|, it ignores CS's subfield effect |
| **CS entropy-corrected conjectures** (Our Conj. 1–6, p.8–23). For an extension field: δ ≤ 1 − H_p(δ) − 1/n − η with p the characteristic. | **Open.** This is the best-standing explicit capacity-side conjecture. KKH needs η = Θ(1/log n) and so stays inside it asymptotically, but KKH lines are concretely bad at n-independent gaps (§3.2). | CS Conj. 5 p.23 | |
| **DG amended Conj. 5.1** (fixed ρ, η > 0, error K·n^c/q, constants depending on ρ, η) | **Open.** It is consistent with every result here because K may be exponential in 1/η. Not usable for concrete parameters. | DG p.23 | all |

---

## 2. Per-paper notes

### 2.1 Diamond–Gruen, *On the Distribution of the Distances of Random Words* (ePrint 2025/2010)

- **Local copy:** `tmp/zk-research/papers/eprint-2025-2010.{pdf,txt}` (24 pp.).
- **Main results.**
  - **Thm 2.5 (p.8).** For any linear code, err(C, z) ≥ ((q−1)/q)·Pr_u[d(u, C) ≤ z]: the proximity-gap error is at least the probability that a uniformly random word is close.
  - **Thm 3.10 (p.11), Thm 3.16 / Cor. 3.17 (p.15–17).** Sharp estimates of ball volumes and their intersections.
  - **Thm 4.14 (p.22).** For every c* ≥ 2 there are infinite families of MDS codes, RS among them, with err(C, z) > n^{c*}/q. This refutes BCIKS Conj. 8.4, restated as Conj. 2.2 on p.7. The result "applies to all Reed–Solomon codes, over all characteristics, and all domains" (p.1).
- **Counterexample regime.**
  - q = n^{c*+1}, so q is polynomial in n.
  - Both the rate k/n and the relative shortfall h/n go to 0; the technique needs k, h = o(√n) (p.1, Remark 4.2 p.19).
  - The list size is not the mechanism. The mechanism is the coverage of F_q^n by balls.
- **Random-words heuristic (§1.5, p.5–6).** Balls of radius z start to fill F_q^n once η ≲ η_RW = ρ·log₂(e/ρ)/log₂q (eq. 3). The refined version, eq. 19 on p.23, divides by log₂q − log₂(e/ρ) instead.
  - The query count for b bits is −b / log₂(ρ + η_RW) (eq. 5).
  - Table 1 (p.6, log₂q = 128): 102.8 queries at ρ = 1/2, 50.98 at ρ = 1/4, 33.89 at ρ = 1/8, where capacity needs 100, 50 and 33.3.
  - The authors do not call the region between list decoding and random words "safe". They recommend proven bounds, and advise anyone who uses conjectures to stay above the random-words cutoff (p.6).
- **Amended Conj. 5.1 (p.23).** Fixed ρ, fixed η > 0, error K·n^c/q with K and c depending on (ρ, η), and only for n large. The authors call their confidence in it "rather low" (p.23).
- **Applies to umis?**
  - The *refutation* covers every RS code, but its families (rate → 0) are not umis' parameters.
  - The *random-words* formula is exactly `p3-security`'s "conjectured" FRI query term [C: `fri.rs` `conjectured_error`, with `modulus_bits = 124` for umis]. That gives 0.9719 bits per query at ρ = 1/2 and Q·0.972 + 16 = 113.2 bits.
  - The p3 conjectured commit-phase term reuses the unique-decoding count (n+1)/|F| [C: `conjectured_commit_phase_error`]. Nothing in DG proves that this count holds at the RW radius.

### 2.2 Crites–Stewart, *On Reed–Solomon Proximity Gaps Conjectures* (ePrint 2025/2046, 19 Dec 2025)

- **Local copy:** `tmp/zk-research/papers/eprint-2025-2046.{pdf,txt}` (24 pp.).
- **Main results.**
  - **Thm 1 (p.8).** For f < n−k and any u⁽¹⁾ far from the code there is a u⁽⁰⁾ such that at least C(n,f)·q / (C(n,f) + q^{n−f−k}·g(·)) values of λ make u⁽⁰⁾ + λu⁽¹⁾ f-close. The proof uses the second moment of the number of agreement sets of a random word (Lemma 1).
  - **Cor. 1 (p.13).** When n(1−H_q(f/n)) + 2 + √(nH_q(f/n) − f) ≤ k ≤ n−f−2, the **entire line** is f-close while u⁽¹⁾ is not. This refutes BCIKS Conj. 8.4, WHIR Conj. 4.12 (MCA to capacity) and, through Elias's list-decoding capacity theorem (p.7), DEEP-FRI Conj. 2.3.
  - **Thm 2 (p.16).** CA over lines with error ε < (q−n)/(kq) ⇒ RS(k+1) is (f/n, L)-list-decodable with L ≈ 2εq. So any future CA proof with small error implies a list-decoding theorem. The proof uses the quotient trick u/(x−a), 1/(x−a) (Claim 3, p.16–18).
  - **§5 (p.19–23), extension fields with the domain in a subfield.**
    - Lemma 3 and Cor. 2 (p.19): if RS over F_{q_sub} is not list decodable, neither is RS over F_{q_ext} on the same domain. Capacity is therefore governed by H_{q_sub}.
    - Thm 3 (p.19–23): a line over F_{q_ext} built from a *base-field* random word gives ≈ C(n,f)/(q_sub^{n−f−k}·g) bad λ.
- **Modified conjectures.**
  - Our Conj. 1–3 (p.8, 15, 16) cover prime fields: replace δ ≤ 1−ρ−η by H_q(δ) ≤ 1−ρ−η, and for CA δ ≤ 1 − H_q(δ) − 1/n − η.
  - **Our Conj. 4–6 (p.19, 23) cover extension fields and use H_p with p the characteristic.**
  - The authors estimate the cost as about 1/log₂q in distance, "only 1/31" even for the smallest practical fields (p.3).
- **Counterexample regime.** Only beyond the list-decoding capacity 1 − H_q(δ), roughly within n/log₂q·(…) of capacity. Corollary 1 applies when n−f−k ≲ n/log₂q (p.13).
- **Applies to umis?**
  - Yes, through §5. umis' domain is in F_p (p ≈ 2^31) and its challenges are in EF4, so the relevant entropy is H_p with log₂p ≈ 31, not log₂|EF4| ≈ 124.
  - [E] Solving δ + H₂(δ)/31 = 1 − ρ gives **0.910, 1.850 and 2.794 bits per query at ρ = 1/2, 1/4, 1/8**, against 1, 2 and 3 at capacity and 0.972, 1.961 and 2.949 for RW with q = 2^124.
  - At Q = 100 and ρ = 1/2 that is 91.0 bits instead of 97.2 bits before PoW.
  - The CS paper does not discuss FRI/DEEP-specific attacks; the bounds are combinatorial.

### 2.3 Ben-Sasson, Carmon, Haböck, Kopparty, Saraf, *On Proximity Gaps for Reed–Solomon Codes* (ePrint 2025/2055, 6 Nov 2025; STOC 2026)

- **Local copy:** `tmp/zk-research/papers/eprint-2025-2055.{pdf,txt}` (49 pp.).
- **Positive results** (all fields, all domains).
  - **Thm 1.3 (p.8) / Cor. 1.4 (p.9).** Up to δ/2 with a > γn + 1 exceptions and zero loss, which is optimal.
  - **Thm 1.5 (p.9).** Up to Johnson, γ < 1−√ρ: a > (m+½)/(3ρ^{3/2})·n + (2(m+½)⁵ + 3(m+½)γρ)/√ρ = O_ρ(n/η⁵) with m = max(⌈√ρ/(2η)⌉, 3). BCIKS20 had O(n²/η⁷).
  - **Thm 4.2 (p.27).** Curves of degree M: M times the count.
  - **Thm 4.3 / Cor. 4.4 (p.27–28).** Agreement on given sets and weighted CA, which is what FRI's round-by-round analysis uses.
  - **Thm 4.6 (p.28–29).** List / mutual CA up to Johnson with the same count.
- **Negative results.**
  - **Thm 1.6 (p.10).** Characteristic 2, random F₂-subspace domains, δ = 1−λ_τ, γ = 1−4λ_τ with λ_τ = 2^{−(τ+2)}: (1−o(1))q bad z at proximity loss 2λ_τ. This refutes the n^τ conjecture (Conj. 1.2, p.6) for every τ.
  - **Cor. 1.7 (p.11).** δ = 15/16 at the Johnson radius: a must reach n^{2−o(1)}, so the O(n) Johnson bound is tight there.
  - **Thm 1.9 (p.11).** For **every** RS code, at γ = LDR_{q}(δ) + 2/n there are ≥ q/(2n) bad z. The soundness error is ≥ 1/(2n) **independent of q**. Gaps beyond Johnson therefore need list decoding beyond Johnson first.
  - **Thm 1.13 (p.12) + Conj. 1.12.** Prime fields, multiplicative subgroups, conditional on a sum-set conjecture.
    - Explicit instance for q = M31⁴ ≈ 2^124 (p.13): rate ≈ 1/2 − 4/b with b = 248, δ ≈ 0.508. At γ = 1/2 ≈ δ − 0.008, **all q** values of z are close, while the pair has proximity loss 1/b ≈ 0.004.
    - This is close to umis' setting: a 31-bit prime with a quartic extension.
  - **Thm 1.17 (p.15–16), §8 (p.40–45).** An attack on the DEEP-ALI STARK reduction for a toy AIR (CYCLE-SUM): with probability Ω(1/n) the prover ends at proximity (1 + γ_q)/2.
  - **Lemma 8.4 (p.45).** On the ethSTARK toy problem the prover succeeds with probability "1/2n + (1−γ_q)^t", as printed.
- **Applies to umis?**
  - The positive Thm 1.5 / 4.x results are what make 66 proven bits possible. `p3-security` 0.8 now uses the sharper DKT count (§2.5) [C: `proximity.rs`].
  - The negative results bound how far any analysis can go. Beyond the list-decoding radius for list size q, the gap error is at least 1/(2n) ≈ 2^−16 for n = 2^15, so no gap-based argument can work there.
  - The M31⁴ instance shows that "a 124-bit extension field" does not make gaps hold near capacity.

### 2.4 Krachun–Kazanin–Haböck, *Failure of proximity gaps close to capacity* (ePrint 2026/782, Apr 2026); Kambiré (arXiv 2604.09724)

- **Local copies:** `tmp/zk-research/papers/eprint-2026-782.{pdf,txt}` (16 pp.); `tmp/zk-research/papers/arxiv-2604.09724.{pdf,txt}` (6 pp.).
- **Main results.**
  - **Thm 1 (p.3).** For τ ≥ 1, δ ∈ (0,1) and β > max{τ+1, 12/5} there are infinitely many n = 2^b and primes p ≡ 1 (mod n), p = Θ(n^β), with RS[F_p, H, k] on a **multiplicative subgroup H of size n**, such that at η = c/(τ·log n)·(1+o(1)):
    - the list size at δ* − η is ≥ 2^{c/η} = n^{τ−o(1)};
    - a line has ≥ n^{τ−o(1)} points at distance ≤ δ* − 2η while Δ(u₁, C) ≥ δ* − η.
  - This refutes Conj. 1 (BCIKS) and Conj. 2 (BGKS list decoding) as stated (p.2).
  - Other statements:
    - Prop. 1 (p.5): the direct construction from vanishing polynomials of unions of cosets x ↦ x^m onto a subgroup G of size s ≈ log n.
    - Lemma 1 (p.6): sums of r distinct elements of G are distinct when p > s^{s/2}.
    - Prop. 2 (p.10): list-size version.
    - Remark 5 (p.10): distinct subsets give distinct vanishing polynomials, so ≥ C(s, r) list members with **no field condition**.
    - Prop. 3 (p.15): the simpler quotient construction u₀ = u/(x^m − z^m), u₁ = 1/(x^m − z^m).
    - **Prop. 4 (p.16): ≥ p/(2n) bad λ, so the error is ≥ 1/(2n), at η = Θ(1/log n).**
  - Kambiré, Thm 1 (p.2): ρ ∈ (0, ½), p < n^A with A = K·log 8, η = 2/s = Ω(1/log n); explicit parameter bookkeeping.
- **Counterexample regime.** The construction depends only on s (the size of the subgroup G ⊂ H) and r:
  - agreement r/s, code degree ≤ (r−2)m, list members P_S(X^m) for r-subsets S ⊂ G;
  - the field restriction in the theorem (p = Θ(n^β)) serves only to obtain an infinite family and polynomial-size primes.
  - KKH note on p.4 that their theorem hits practical IOP parameters harder than Fenzi–Sanso 2025/2197 suggested, and point to the S-two whitepaper (2026/532) for discussion.
- **Applies to umis? Yes, structurally, and concretely bad at small gaps [D].** The Appendix-A construction (Lemma 3 p.13 + Prop. 3 p.15 + Remark 5) needs only:
  - (i) a 2-power multiplicative subgroup G ⊂ H of size s dividing n. KoalaBear has two-adicity 24 and umis' FRI domains are such subgroups (or cosets of them);
  - (ii) a challenge field large enough for Lemma 3 to separate the list. EF4 is.

  Working it through for FRI rate ρ: take r = ρs + 1. The line code has degree ≤ (ρs−1)m < ρn, and u₁ = 1/(x^m − z^m) has agreement < ρ + 1/s with any degree-< ρn polynomial. Lemma 3, with z ranging over m-th powers, then gives

  > #bad λ ≥ ½·min( C(s, ρs+1), q/(ρn) ) at agreement ρ + 1/s, so ε ≥ ½·min( C(s, ρs+1)/q, 1/(ρn) ).

  The bad fraction does not depend on n except through the 1/(ρn) cap. Numbers are in §3.2.

### 2.5 Dao–Kominers–Thaler, *Reed–Solomon Codes Beyond Johnson: Efficient Decoding and Smaller Cryptographic Proofs* (ePrint 2026/2056, version of 2 Nov 2026)

- **Local copy:** `tmp/zk-research/papers/eprint-2026-2056.{pdf,txt}` (107 pp.).
- **Main theorems.**
  - **First-order threshold (eq. 1, p.5).** For ρ ≥ ρ_c = 11 − 3√13 ≈ 0.183, a₁(ρ) = (3ρ + 2√(ρ(5−ρ)(2−ρ)))/(8−ρ). Otherwise a₁(ρ) = √(2ρ)·cos(⅓·arccos(√(ρ/8) − 1)).
    - Values [E, checked against p.5]: a₁(1/2) = 0.6899, a₁(1/4) = 0.4688, a₁(1/8) = 0.3191.
    - The Johnson values are 0.7071, 0.5 and 0.3536.
  - **Thm 1.1 (p.6), Thm 5.1 / 5.2 (p.30–31).** At agreement a₁(ρ) + η₁, over char 0 or p > max{k−1, B_∂} on any n distinct points, the list is O_ρ(n/η₁²) and the line-MCA exceptional count is O_ρ(n²/η₁⁴).
    - Exact finite forms are eq. (40) and (41), with parameters from (28) on p.23.
    - Finite certificates come from Prop. C.2 (p.81).
  - **Thm 1.2 (p.6), Thm 8.1 (p.44).** Uniform capacity bounds at gap δ: list C_δ·n^{d(δ)} and MCA C_δ·n^{d(δ)+1}/q with d(δ) = ⌈e^{1.5/δ}⌉ for δ < 6/25, for n ≥ N(δ) and char p > k−1.
  - **Thm 5.7 (p.35).** At gap δ ≥ 6/25: list ≤ 307n and MCA count ≤ 1,325,775·n².
  - **Thm 4.4 (p.29–30), Johnson MCA in every characteristic.** With D = k−1, ρ⁻ = D/n, a = √ρ⁻ + η₀, m = max(⌈√ρ⁻/(2η₀)⌉, 3), t = m + ½: the exceptional count is < **8nt³/(3ρ⁻)**, which is O(n/η₀³). BCHKS had O(n/η₀⁵).
    - This is the count `p3-security` 0.8 uses [C: `proximity.rs::johnson_exceptional_line_count_log2`, where it is cited as "Theorem 5.12" of an earlier DKT version].
  - **Cor. 6.3–6.7 (p.36–38).** Affine families, interleaved codes, and h-level multilinear folds (error h·E_exc/q).
  - **Table 2 (p.7) and Table 4 (p.40).** Deployed savings: ProveKit 11.1%, ZisK 4.63%, LambdaVM 4.85%. All use BN254 or cubic Goldilocks challenge fields of 192–254 bits.
- **Limits (§10, p.70–71).**
  - The capacity bounds need a fixed gap, and the paper defers to KKH/Kambiré for gaps that shrink like Θ(1/log n).
  - Characteristic matters: extending a small prime field enlarges q but not p.
  - DKTZ26 refutes beyond-Johnson gaps over binary fields.
- **KoalaBear data point (p.71).** On the EF Proximity-Prize benchmark (8 RS words, length 2^18, dimension 2^17, so ρ = 1/2, over **F_{p⁶} on a multiplicative subgroup of KoalaBear**, with a 2^−128 algebraic error target):

  | Certificate | Agreement A | Score (128 queries) | Bits per query |
  |---|---|---|---|
  | DKT first-order | 183210 (a ≈ 0.6989) | **66.15 bits** | 0.517 |
  | DKT Johnson | 185372 | **63.99 bits** | 0.500 |
  | Best prize submission (15 Sep 2026) | 181284 (a ≈ 0.6915) | **68.11 bits** | 0.532 |
- **Applies to umis?**
  - **Johnson part: yes, already in use.** Plonky3 0.8's LDR commit term is the Thm 4.4 count.
  - **First-order beyond-Johnson part: the theorem applies, the numbers do not, on EF4 [E].**
    - KoalaBear's characteristic (≈ 2^31) exceeds k − 1 (≤ 2^14 for umis), so the hypothesis holds; DKT applied it to KoalaBear F_{p⁶} themselves.
    - But the MCA count is quadratic in n. Its leading term (48D²B_Z + …)·λ²·B_jet·B_∂ alone is ≥ 2^33 at n = 2^15 before the B factors, against q = 2^124. That leaves at most ~90 bits on the commit term with no margin.
    - The gain is also small: 0.5355 vs 0.5 bits per query asymptotically at ρ = 1/2 (+7%), and 1.093 vs 1.0 at ρ = 1/4 (+9%).
    - It is plausibly usable with EF5 (2^155) or EF6 (2^186), but only via a per-code certificate from the DKT artifact (Prop. C.2).

### 2.6 Jeronimo, *Algorithmic List Decoding at Capacity and Optimal Proximity Gaps for Reed–Solomon Codes* (arXiv 2609.05870, 5 Sep 2026)

- **Local copy:** `tmp/zk-research/papers/arxiv-2609.05870.{pdf,txt}` (37 pp.).
- **Main results.**
  - **Thm 1.1 (p.3).** Fixed γ, n ≥ n₀, prime q ≥ n, k ≤ (1−γ)n: deterministic decoding at radius 1 − k/n − γ in q^{C_alg} time, with list ≤ n^{C_list} independent of q.
  - **Thm 1.2 (p.4).** Curve MCA: ≤ n^{C(γ,ℓ)} bad parameters, so the line-MCA error is n^{C(γ,1)}/q with no proximity loss. Cor. 7.3 (p.30) gives ordinary correlated proximity gaps.
  - **Cor. 8.3 (p.31–32).** "Certified radii" ρ_cap − γ at rates 1/2 … 1/16 for the prize challenges, given q > 2^λ·n^{C*}. The author states these are lower-bound certificates only.
  - **§8.4 (p.32).** The list and MCA arguments extend to fields of characteristic p > max{k−1, B_{γ,L}}, which covers KoalaBear's extensions. The decoder needs a prime field.
- **Limitations, stated by the author (§8.5, p.32).** The constants are "not optimized" and there is no "useful certificate" for small block lengths. The shrinking-gap window of KKH is explicitly not addressed (§8.4).
- **Applies to umis?** Only as evidence that fixed-gap capacity MCA is *true*. There are no usable numbers.

### 2.7 Brakensiek–Chen–Putterman–Zhang–Zheng, *Algorithmic List Decoding of Reed–Solomon Codes up to Capacity* (arXiv 2609.08005, 7 Sep 2026)

- **Local copy:** `tmp/zk-research/papers/arxiv-2609.08005.{pdf,txt}` (28 pp.).
- **Main results.**
  - **Thm 1.1 (p.3) / Thm 4.1 (p.20).** Low rate R ≤ (1−θ)ε over a **prime** field, q ≥ max(n, 4ε^{1−9/θ}·n/k): decoding from 1−ε errors in q^{O(ε^{−12/θ})} time, list ≤ q^{O(ε^{−3/θ})}. Requires ε < (θ(1−θ)³/768)^{(5+θ)/(1−θ)} (eq. 26).
  - **Cor. 1.2 (p.3) / Cor. 5.1 (p.22).** Every constant rate, by padding (due to Alrabiah–Goyal–Guruswami): agreement R + δ, prime q ≥ C(R,δ)·n, list q^{O_{R,δ}(1)}.
- **List decoding only, no MCA.** The MCA results attributed to this line of work are Zheng's manuscript (Zhe26) and Jeronimo. Correction to `research-literature.md` §2.1, which ascribes MCA to both papers.
- **Applies to umis?** No for parameters. The list bound grows with q (2^124), which makes it useless at EF4. The constants are astronomical, and the field must be prime.

---

## 3. What the counterexamples cost umis, in bits

### 3.1 Per-query bits by regime

[E] Q-term in bits before PoW, at query counts that give ≈ 100 bits at capacity:

| Regime | ρ = 1/2, Q = 100 | ρ = 1/4, Q = 50 | ρ = 1/8, Q = 34 | Status |
|---|---|---|---|---|
| Capacity (ethSTARK, p3 "legacy") | 100.0 (1.000 b/q) | 100.0 (2.000) | 102.0 (3.000) | refuted |
| DG random words, q = 2^124 (p3 "conjectured") | 97.2 (0.972) | 98.0 (1.961) | 100.3 (2.949) | heuristic |
| DG random words with q = p = 2^31 (base-field variant, CS §5 rationale) | 89.1 (0.891) | 92.4 (1.848) | 95.4 (2.807) | heuristic |
| CS Conj. 5, H_p with p = 2^31 | 91.0 (0.910) | 92.5 (1.850) | 95.0 (2.794) | open conjecture |
| Ceiling from KKH lines for a 2^−100 commit term on EF4 (§3.2) | ≤ 91.3 (0.9125) | ≤ 95.6 (1.913) | ≤ 96.2 (2.830) | upper limit for any gap-based proof |
| DKT first-order a₁(ρ), asymptotic | 53.6 (0.536) | 54.6 (1.093) | 56.0 (1.648) | proven; not usable on EF4 |
| Johnson √ρ, asymptotic | 50.0 (0.500) | 50.0 (1.000) | 51.0 (1.500) | proven |
| p3 JB default (m = 10, α = 1.05√ρ) | 43.0 (0.430) | 46.5 (0.930) | 48.6 (1.430) | proven; p3 searches m up to 1000 for FRI |

Reading the table:

- **Against capacity**, the realistic "post-counterexample conjectured" level (the CS/KKH rows) costs about **9 bits at ρ = 1/2, 4–8 bits at ρ = 1/4 and 6–7 bits at ρ = 1/8** for a ≈ 100-bit query term.
- **Against `p3-security`'s RW number**, the cost is 6, 2–6 and 4–5 bits.
- Lower rates lose slightly less in absolute bits but cost a 2–4× larger LDE.
- **Proven security halves the bits per query at every rate.** That, not the counterexamples, is the dominant gap.

### 3.2 KKH/quotient lines on two-adic subgroups (Prop. 3 construction) [D, E]

The bad fraction is ε ≥ ½·min(C(s, ρs+1)/q, 1/(ρn)) at agreement ρ + 1/s, where s is the size of the subgroup G ⊂ domain. Entries are log₂ε for q = 2^124 (EF4) and n = 2^15.

| s (excess 1/s) | ρ = 1/2: agreement → log₂ε | ρ = 1/4 | ρ = 1/8 |
|---|---|---|---|
| 16 (0.0625) | 0.5625 → −111.5 | 0.3125 → −112.9 | 0.1875 → −115.9 |
| 32 (0.0313) | 0.5313 → **−95.9** | 0.2813 → −100.3 | 0.1563 → −107.4 |
| 64 (0.0156) | 0.5156 → **−64.4** | 0.2656 → **−74.7** | 0.1406 → **−90.3** |
| 128 (0.0078) | 0.5078 → **−15** (capped at ≈ 1/(2ρn)) | 0.2578 → −23.2 | 0.1328 → −55.9 |
| ≥ 256 | → ≈ −log₂(2ρn) | ≈ −14 | ≈ −13 |

With EF5 (q = 2^155) every C(s,·)/q entry drops by 31 bits, for example s = 64 at ρ = 1/2 gives −95.4. The values are identical for n = 2^16 and 2^17 except for the cap.

Consequences:

- **The RW radius is inside the region these lines break.** At ρ = 1/2 the RW agreement is 0.50985. Lines with excess 1/64 (agreement 0.5156 > 0.50985) already have **ε ≥ 2^−64** on EF4. That contradicts the (n+1)/|F| ≈ 2^−109 commit-phase term that `p3-security` reports for umis' conjectured regime.
  - So the conjectured commit-phase term at the RW radius has no proof behind it.
  - It is also contradicted by an explicit construction on exactly umis' kind of domain, if a fold input can take this shape.
- **This is not a demonstrated attack on umis.**
  - In interactive FRI the cheater needs both a bad λ (probability ε) and passing queries (≈ (ρ + 1/s)^Q). For Q = 100 that product is below ρ^Q in every row.
  - Under Fiat–Shamir an adversary can grind the fold challenge separately, so the costs add rather than multiply. If an adversary could make the first FRI fold input equal such a (u₀, u₁) pair, the cheapest row (s = 32) would cost about 2^95.9 + 2^107.3 ≈ **2^107 work for umis' preset**, against 2^113.2 for p3's conjectured query term and 2^116 for legacy.
  - Whether the DEEP and batching randomness lets a prover steer the fold input into this shape is not shown in any of these papers. BCHKS §8 gives Ω(1/n)-probability attacks only on toy AIRs.
  - Treat the 6-bit gap as a **worst-case erosion of the conjectured margin**, not as an attack.
- **umis' headline conjectured number survives.** It is limited by batching at 101.4 bits, so it is unchanged even if the query term drops to about 105–107. Only the margin shrinks.
- **Proven numbers are unaffected.** All refutations live beyond the list-decoding radius. The proven Johnson regime has a = √ρ ≈ 0.707 at ρ = 1/2, far below the s ≥ 8 rows.

### 3.3 Is any proven beyond-Johnson bound usable today?

**Not on EF4.**

- DKT first-order (Thm 5.2, eq. 41) has an MCA count Θ(n²/η₁⁴) with large constants. At n = 2^15 and q = 2^124 the commit-phase term cannot reach 100 bits.
- Jeronimo, BCPZZ and DKT's capacity theorem have non-effective or enormous constants.

**What is usable now, and already implemented in Plonky3 0.8** [C: `security/src/proximity.rs`, `fri.rs`]:

- **Per-query.** α = (1 + 1/(2m))·√ρ, with Q-term = −Q·log₂α + PoW_query.
- **Per fold** (arity f). The commit-phase error is the smaller in bits of
  - (f−1)·E/|F| with E = 8n(m+½)³/(3ρ⁻) (DKT Thm 4.4, p.29), and
  - f·(n+1)·(2m+1)/(√ρ·|F|) [C: `commit_phase_error_ldr_m`],

  each credited with the commit PoW.
- **Batching / DEEP.** List size L⁺ = (m+½)/√ρ (BCHKS Thm 4.2 at the Johnson radius; `list_size_ldr_m`).
- **Choice of m.** p3 searches m ∈ [3, 1000]. For umis this gives the reported 65.9 proven bits, bound by queries.

**Beyond Johnson.** It becomes plausible only with a larger challenge field (EF5 ≈ 2^155, EF6 ≈ 2^186) together with a per-code certificate from the DKT artifact (Prop. C.2 p.81, Appendix C p.79–84). The expected gain at ρ = 1/2 is ≈ 3% fewer queries, from the prize data point: 66.15 vs 63.99 bits at 128 queries. That is not worth the engineering cost before the larger questions (rate, PoW placement, extension degree) are settled.

---

## 4. Guidance for umis

1. **Quote proven (Johnson) security as the headline.**
   - This means `p3-security`'s `proven_security_report`, which takes the best of the UD and LDR regimes with the DKT Thm 4.4 count. Today that is 65.9 bits for `new_benchmark`.
   - The Ethereum Foundation and soundcalc accept only this (`research-literature.md` §2.1).
   - No 2025–26 result threatens the Johnson regime. It is proven in every characteristic and on every domain (BCHKS Thm 1.5, DKT Thm 4.4, Haböck 2025/2110).
2. **If a "conjectured" figure is shown at all**, label it with the exact assumption and use the conservative variant:
   - Per-query bits = −log₂(1 − δ*), where δ* + H₂(δ*)/log₂p = 1 − ρ and p = 2^31 is the characteristic (CS Conj. 5, p.23). That is 0.910 b/q at ρ = 1/2.
   - Cap the result by the KKH-line ceiling of §3.2: 0.9125 b/q at ρ = 1/2 for a 2^−100 commit term.
   - Do not quote the legacy ethSTARK number (116 bits). It rests on refuted conjectures.
   - Do not quote p3's RW number (113.2-bit query term, 109-bit commit term) as anything stronger than "heuristic".
   - In practice the conjectured total stays ≈ 101 bits, limited by batching. Report it as "≈101 (heuristic; batching-limited; query term 105–107 under CS/KKH-corrected assumptions)".
3. **Parameter changes should target proven bits.**
   - The REPORT §2.2 options still stand: lb2 + Q ≈ 124 + batch/commit PoW gives about 100 proven bits; EF5 plus a wider digest is needed for 128.
   - At rate 1/4 the counterexamples cost relatively less (§3.1). Proven per-query bits double from 0.5 to 1.0, which is the real win.
   - Higher rates (1/8) do not change the picture beyond the larger LDE.
4. **Do not rely on beyond-Johnson bounds for EF4.** Revisit DKT first-order certificates only if umis moves to EF5/EF6. Even then, run the DKT artifact for the exact (n, k, A) rather than using asymptotic formulas.
5. **Code hygiene.**
   - If umis ships its own security printout, it should name the regime and the DKT/BCHKS theorem it uses.
   - It should pass `modulus_bits = 124` and also report the base-field variant. `p3-security`'s RW term silently uses the extension field size.

## 5. What to monitor

- **EF Proximity Prize** (proximityprize.org) and the **better.codes "koalaIRS"** leaderboard. That is the KoalaBear F_{p⁶}, rate-1/2 instance. Current best is 68.11 bits per 128 queries (DKT p.71). Any proof pushing the per-query exponent toward a₁(1/2) = 0.6899 or below, with errors that fit EF4/EF5, matters directly.
- **ABF26**, Arnon–Boneh–Fenzi, *Open problems in list decoding and correlated agreement* (ePrint 2026/680), which defines the prize metrics; **Fenzi–Sanso** 2025/2197 (attacks on small-field SNARKs); the **S-two whitepaper** 2026/532 (Appendix A.5 conjecture, cited by KKH p.4 and Kambiré p.1).
- **Prime-field near-capacity constructions with n-independent gaps.** If KKH-type lines can be steered through DEEP and batching into a real FRI fold, the conjectured query term loses about 6 bits at Q = 100. Also watch for any construction beating C(s, ρs+1) bad points on 2-adic subgroups.
- **DKTZ26** (2026/2424): binary-field failure. Relevant only if umis moves to Binius-style fields.
- **`p3-security` and soundcalc changes.** Whether they re-base the "conjectured" regime on H_p or drop it; whether they add a DKT first-order LDR option, which would then need checking against EF4 n²/q terms.
- **Effective constants** for Jeronimo, BCPZZ (a promised future version) and the DKT capacity theorem. A practical O(n^c/q) MCA beyond a₁(ρ) with small c would be the first thing that changes the proven numbers by more than a few percent.

## 6. Reproducing the estimates

The tables in §3 come from a short throwaway Python script; nothing is kept in the repo. It computes four things:

- C(s, ρs+1) via `lgamma`;
- the RW η = ρ·log₂(e/ρ)/log₂q;
- the CS H_p radius, by bisecting δ + H₂(δ)/31 = 1 − ρ;
- DKT a₁(ρ) from eq. (1).

Fiat–Shamir costs add, 2^{−log₂ε} + 2^{Q·(−log₂(ρ+1/s)) + PoW}, minimised over s. Assumptions:

- the line counts use the m-th-power restriction of Lemma 3, which gives the 1/(ρn) cap;
- FRI arity is 2 with a single fold line;
- DEEP and batching steering is assumed possible in the Fiat–Shamir row only.
