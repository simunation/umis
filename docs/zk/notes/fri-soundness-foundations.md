# FRI / STARK soundness foundations and parameter selection for umis

Status: November 2026. Plonky3 0.8.0 (`p3-security` 0.8.0, `p3-fri` 0.8.0, `p3-batch-stark` 0.8.0). Upstream `main` of `p3-security` was compared where it differs.

This note covers four things:
- what the four foundational FRI papers prove, with theorem numbers, page numbers and formulas;
- how every term that `p3-security` reports maps back to those papers;
- how to pick FRI/PCS parameters for umis, with worked numbers;
- what must not go wrong when someone changes them.

Sibling notes:
- `proximity-gaps-status.md`: the 2025–26 refutations, BCHKS25 and DKT26;
- `fiat-shamir-and-lookups.md`: transcript binding and LogUp;
- `plonky3-implementation-guide.md`: code paths;
- `pcs-stir-whir-circle.md`: alternative PCSs.

**Page convention.** All page numbers are **PDF page numbers** (what `Read pages=` and a PDF viewer show). The printed page number differs only for 2020/654: printed = PDF − 1, because the title page has no number. For the other three papers printed = PDF.

**Notation used throughout:**

| Symbol | Meaning (umis value) |
|---|---|
| `k` | trace height = polynomial degree bound (2^14 today; 2^16 used as the "next size") |
| `n` | LDE / FRI domain size = `k·2^lb` |
| `lb` | `log_blowup`; rate ρ = 2^−lb |
| ρ⁺ | rate inflated by the DEEP quotient: (k + max_combo)/n, with `max_combo` = 2 (z and g·z) |
| `m` | Johnson-regime multiplicity parameter, m ≥ 3; agreement α = (1 + 1/(2m))·√ρ |
| L⁺ | list size in the Johnson regime: (m + ½)/√ρ (BCHKS25/DKT26 form, as used by p3-security) |
| `F` | log₂ of the challenge-field size: \|EF4\| = p⁴ ≈ 2^123.95 (p3-security is fed 124) |
| `C` | number of AIR constraints (56) |
| `d` | max constraint degree (3); `c` = quotient chunks = next_pow2(d − 1) = 2 |
| `B` | number of batched opening functions (powers of the PCS batching challenge) = 198 |
| `N`, `W` | LogUp: N = interactions per row × k = 23·k, W = max message width = 5 |
| `s` | `num_queries` |
| `a` | FRI folding arity = 2^`max_log_arity` |
| `lfp` | `log_final_poly_len` |
| z_* | proof-of-work (grinding) bits at a site |

---

## 1. The papers

### 1.1 Ben-Sasson, Carmon, Ishai, Kopparty, Saraf, *Proximity Gaps for Reed–Solomon Codes* (ePrint 2020/654, FOCS 2020; full version 3 July 2021) — "BCI+20"

- **Local copy:** `tmp/zk-research/papers/eprint-2020-654.{pdf,txt}` (61 pp.)
- **What it proves.** Take an affine space or low-degree curve of words. Either almost all of its members are δ-close to the RS code, or only an ε-fraction are. When enough of them are close, the generating words share one common agreement set ("correlated agreement"). The unique-decoding regime needs only a field linear in n. The Johnson regime reaches δ < 1 − √ρ but needs a field quadratic in n. It also gives the first FRI soundness bound up to the Johnson radius.
- **Rate convention:** V = RS[F_q, D, k] is the set of polynomials of degree ≤ k, so ρ = (k+1)/n.

**Key statements:**

| Result | Page | Statement (paraphrased; formulas exact) |
|---|---|---|
| Def. 1.1 proximity gap | p.3 | |
| **Thm 1.2** proximity gap for affine spaces | p.3 | **UD:** for δ ∈ (0, (1−ρ)/2], ε_U = n/q (eq. 1.1). **Johnson:** for δ ∈ ((1−ρ)/2, 1−√ρ), with η = 1 − √ρ − δ, ε_J = (k+1)² / ((2·min(η, √ρ/20))⁷ · q) = O(n²/((ηρ)^{O(1)} q)) (eq. 1.2) |
| Remark 1.1 tightness | p.4 | UD error is sharp up to constants. The quadratic field requirement is inherent to the technique |
| Cor. 1.3 concentration | p.4 | |
| **Thm 1.4** correlated agreement over lines | p.5 | If Pr_z[Δ(u₀ + z·u₁, V) ≤ δ] > ε, then u₀ and u₁ share an agreement set of density ≥ 1 − δ |
| **Remark 1.2** extension-field sampling | p.5 | z may be drawn from an extension F_{q'}: replace q by q' in ε_U and ε_J. The codewords still lie in the base-field code. *This is what lets umis sample from EF4 while committing base-field words.* |
| **Thm 1.5** correlated agreement over degree-l curves | p.6 | The same conclusion if Pr over the curve > **l·ε**. This is the powers-of-α batching that Plonky3 uses |
| Thm 1.6 affine spaces | p.6 | error ε, with no factor l (independent random coefficients) |
| §3.2 FRI discussion | p.14 | t ≈ 2λ/log(1/ρ) queries suffice for q ≫ n² (versus 3λ/log(1/ρ) before) |
| **Thm 4.1** UD lines | p.14 | δ ≤ (1−ρ)/2 and \|S\| > n ⇒ S = F_q, with correlated agreement |
| **Thm 5.1** Johnson lines, explicit m | p.18 | m ≥ 3, δ ≤ δ₀ = 1 − √ρ − √ρ/(2m) (eq. 5.1); \|S\| > (1 + 1/(2m))⁷ m⁷ n² / (3ρ^{3/2}) (eq. 5.3) ⇒ correlated agreement |
| Thm 5.2 alternative form | p.18 | η ≤ √ρ/20, ε_J = ρ² n² / ((2η)⁷ q) (eq. 5.4) |
| **Thm 6.1** UD curves of degree l | p.29 | \|S\| > l·n ⇒ correlated agreement |
| **Thm 6.2** Johnson curves of degree l | p.29–30 | \|S\| > (1 + 1/(2m))⁷ m⁷ n² l / (3ρ^{3/2}) (eq. 6.1) |
| Thms 7.1–7.4 weighted correlated agreement | p.33–34 | The μ-weighted versions needed for FRI |
| §8.2 batched FRI, Eq. (8.3) | p.40 | f⁽⁰⁾ = f₀ + Σ xᵢ fᵢ with *independent* xᵢ (affine batching) |
| **Lemma 8.2** batched FRI commit error | p.41 | Pr[ε_Q > α⁽⁰⁾] ≤ ε_C, with α⁽⁰⁾ = max{α, √ρ(1 + 1/(2m))} and **ε_C = (m+½)⁷·\|D⁽⁰⁾\|² / (2ρ^{3/2}\|F\|) + (2m+1)(\|D⁽⁰⁾\|+1)/√ρ · Σᵢ l⁽ⁱ⁾ / \|F\|**, where l⁽ⁱ⁾ = \|D⁽ⁱ⁾\|/\|D⁽ⁱ⁺¹⁾\| is the arity of round i |
| **Thm 8.3** batched FRI soundness | p.41–42 | **ε_FRI = ε_C + (√ρ·(1 + 1/(2m)))^s**, with ρ = 2^−R and m ≥ 3 |
| Numerical example | p.42 | n = 2^20, ρ = 2^−4, q ≥ 2^256, m = 2^11 − 1, s = 65 ⇒ 128 provable bits |
| **Conjecture 8.4** up to capacity | p.43 | For δ ≤ 1 − ρ − η: ε ≤ n^{c₂}/((ηρ)^{c₁} q), and (l·n)^{c₂}/… for curves; "c₁ = c₂ = 2" is suggested. **Now refuted near capacity**, see `proximity-gaps-status.md` |

**What umis / Plonky3 rely on:**
- **UD regime:**
  - Thm 1.2 eq. (1.1), and Thm 6.1 for curves of degree l = B − 1 (opening batch) and l = a − 1 (each fold), give p3-security's UD `batch-combination` and UD `ldt-commit-phase` terms.
  - These are **classical and uncontested.** They are the conservative foundation of everything in umis.
- **Johnson regime:**
  - Lemma 8.2 / Thm 8.3 define the query term √ρ(1 + 1/(2m)) per query.
  - p3-security keeps BCI+20's **query** term but **replaces BCI+20's commit/batch count** (the (m+½)⁷ n² term) with the DKT26 line count `8n(m+½)³/(3ρ⁻)` (§2.3). In umis that is the difference between ≈83 and ≈101 proven bits for the same parameters (§3.6).
- **Remark 1.2:** sampling from EF4 while committing KoalaBear words is legitimate.
- **Not used by umis:**
  - The (m+½)⁷ n² Johnson count itself;
  - affine (independent-coefficient) batching. Plonky3 batches with powers of one challenge, i.e. Thm 1.5 / 6.x curves with factor l.

### 1.2 StarkWare, *ethSTARK Documentation v1.2* (ePrint 2021/582, July 2023)

- **Local copy:** `tmp/zk-research/papers/eprint-2021-582.{pdf,txt}` (52 pp.)
- **What it proves:**
  - the DEEP-ALI + FRI STARK protocol;
  - knowledge soundness in the IOP model (§5);
  - round-by-round soundness per round (§6);
  - that grinding scales each round's error by 2^−z (Thm 6);
  - the compilation to a non-interactive random-oracle argument, with explicit bits of security (§7).
- It also defines the historical "conjectured" formula that umis' `config.rs` comment still uses.

| Result | Page | Statement |
|---|---|---|
| §3.9–3.11 protocol, FRI, last layer, **grinding** | p.18–21 | Grinding: a nonce whose hash has ζ leading zeros, found before query sampling (§3.11.3, p.21) |
| **Thm 1** Johnson bound | p.30 | For η ∈ (0, 1−√ρ), RS is (1 − √ρ − η, 1/(2η√ρ))-list-decodable |
| Thm 2 Guruswami–Sudan | p.30 | |
| **Thm 3** batched FRI (= BCI+20 Thm 8.3) | p.30–31 | eq. (10): ε_FRI = (m+½)⁷ n² / (2ρ^{3/2} q) + (2m+1)(n+1)/√ρ · Σtᵢ / q + (√ρ(1 + 1/(2m)))^s |
| Remark 4 | p.31 | The first two summands are the commit phase and the last is the query phase |
| **Thm 4** knowledge soundness | p.32 | eq. (11): err_total = ℓ/\|K\| + (d_max + 2^h + a)·ℓ² / (\|K\| − a\|D\| + \|H₀\|) + ε_FRI, with ℓ = m/ρ. ALI uses **linear** batching (1/\|K\| per list element), and DEEP is charged **ℓ²** |
| Cor. 1–2 | p.32–34 | Extraction; STIK |
| §5.9 security, toy problem, **Conjecture 1** | p.38 | The attack succeeds with probability 1/\|F\| + ρ^t (p.39). This is the source of "conjectured" λ = R·s |
| **eq. (18)** | p.39 | Acceptance ε₀ + (1 − ε₀)·2^−ζ·ε₁. λ ≥ min{−log₂ε₀, ζ − log₂ε₁} − 1 (p.40). Digest ≥ 2λ bits (p.39) |
| **eq. (19)** conjectured parameters | p.40 | **λ ≥ min{ζ + R·s, log₂\|K\|} − 1**. Examples: R = 2, ζ = 20, s = 31/41/55 for 80/100/128 bits |
| eq. (20), (21) provable parameters | p.40–41 | Provable needs s = 79 / 104 at 80 / 100 bits (ρ = 1/4, ζ = 20, cubic extension of a 61-bit field). For 128 bits it needs a quartic extension and s = 140 |
| Def. 7 vector round-by-round soundness | p.42 | |
| **Thm 5** RBR soundness of ethSTARK | p.42 | ε₁ = ℓ/\|K\|; ε₂ = (d_max + 2^h + a)ℓ²/(\|K\| − (a\|D\| + \|H₀\|)); ε₃ = (m+½)⁷ n²/(3ρ^{3/2} q); ε_i (folds) = ε₃/t_{i−4} · Π t_j^{−2} + (2m+1)(n+1)t_{i−4}/(q√ρ); ε_k = (√ρ(1 + 1/(2m)))^s; with ℓ = m/(ρ − 2m/\|D\|) |
| **Thm 6** grinding | p.47 | z_i grinding bits in round i give RBR error (ε₁2^{−z₁}, …, ε_k 2^{−z_k}). *This is the justification for adding PoW bits per site* |
| Def. 9 bits of security; Claim 1 (state-restoration ≤ T·maxᵢεᵢ) | p.49 | |
| Thm 7 (BCS16 Thm 7.1); **Thm 8** | p.49 | ε(T, κ) ≤ T·maxᵢ{εᵢ2^{−zᵢ}} + 3(T² + 1)/2^κ |
| **Cor. 3** | p.49–50 | λ bits iff (1) maxᵢ εᵢ2^{−zᵢ} ≤ 2^{−(λ+1)} and (2) **κ ≥ 2λ + 3** (κ = hash output bits) |
| §7.1.1 | p.50 | Worked RBR parameters |

**What umis / Plonky3 rely on:**
- **Thm 6 and Cor. 3.**
  - These give the composition rule p3-security uses: security = min over rounds, with each round boosted by its own grinding.
  - Cor. 3 adds two things p3-security does **not** charge: a **one-bit loss** (2^{−(λ+1)}) and the **hash-output condition κ ≥ 2λ + 3**.
  - For an 8-element KoalaBear digest (κ ≈ 247.9 bits) that condition gives λ ≤ 122.4, slightly below the 124-bit collision cap that p3-security is fed.
- **eq. (19):**
  - `FriParameters::conjectured_soundness_bits()` (`p3-fri-0.8.0/src/config.rs`) and `p3_security::fri::legacy_conjectured_error` (`fri.rs:133`) compute lb·s + z_query.
  - For umis that is 1·100 + 16 = **116**, the "~116 conjectured bits" in `crates/umis-stark/src/config.rs:13`. ethSTARK itself subtracts 1 and caps at log₂\|K\|.
  - The basis of this formula, the toy-problem conjecture with capacity-rate behaviour, is no longer accepted (see `proximity-gaps-status.md`).
- **Thm 5:** this is the structure p3-security's terms follow: ALI, DEEP, batch, folds, queries.

### 1.3 Haböck, *A summary on the FRI low degree test* (ePrint 2022/1216, version of 17 Dec 2024)

- **Local copy:** `tmp/zk-research/papers/eprint-2022-1216.{pdf,txt}` (26 pp.)
- **What it gives:**
  - a clean restatement of BCI+20 for **algebraic (powers-of-λ) batching**, which is what Plonky3 does;
  - FRI as a polynomial commitment, including multi-point quotients;
  - a DEEP-ALI soundness theorem in the list-decoding regime where the list size enters **linearly**, an improvement over ethSTARK's ℓ².

| Result | Page | Statement |
|---|---|---|
| **Thm 1** correlated agreement (BCI+20 Thms 6.1/6.2, curve form) | p.4 | **UD:** ε = (N − 1)·\|D\|/\|F\| (eq. 1). **Johnson** (θ = 1 − √ρ(1 + 1/(2m)), m ≥ 3): ε = (N − 1)·(m+½)⁷/(3ρ^{3/2})·\|D\|²/\|F\| (eq. 2). For affine batching replace (N − 1) by 1 |
| §3.2 query phase (Protocol 2) | p.6–7 | |
| §3.3 batching, eq. (6) | p.7 | h = Σ λⁱ qᵢ (powers of a single λ) |
| **Thm 2** batched FRI (= BCI+20 Thm 8.3, powers version) | p.7 | eq. (7): ε = (L − ½)·(m+½)⁷/(3√ρ³)·\|D₀\|²/\|F\| + (2m+1)(\|D₀\|+1)·Σaᵢ/(√ρ\|F\|) + (1 − θ)^s |
| Remark 3; ε_C / ε_Q split | p.7–8 | ε_Q = (1 − θ)^s = (√ρ(1 + 1/(2m)))^s |
| §3.5 example parameters | p.8–10 | Recipe: choose the largest m with ε_C ≤ ½·2^−λ, then s with ε_Q ≤ ½·2^−λ. Tables for 64-bit base fields with degree-2/3 extensions |
| Conjecture 1 (= BCI+20 Conj. 8.4) | p.10 | |
| §4.1 FRI as a PCS in UD; §4.1.3 multi-point quotients | p.11–13 | k + m_max < (1 − θ)n; m_max ≤ k/2 suffices (eq. 11). Max degree becomes k + m − 1. *This is why p3-security uses ρ⁺ = (k + max_combo)/n in UD* |
| §4.2 list commitments | p.13–14 | L ≤ 1/(2ηρ⁺) |
| Protocol 3 DEEP-ALI for AIR | p.16–17 | Constraints batched with powers λ^{i−1}; quotient split into d segments; DEEP point z ∉ D ∪ H; FRI on (pᵢ − Vᵢ)/((x − z)(x − gz)) and (h_j − v_j)/(x − z); agreement α⁺ = (1 + 1/(2m))√ρ⁺, ρ⁺ = (\|H\| + 2)/\|D\| (eqs. 19–20, p.16) |
| **Thm 8** DEEP-ALI soundness | p.18 | eq. (22): **ε ≤ L⁺·( C/\|F\| + (d(k⁺ − 1) + (k − 1))/(\|F\| − \|D ∪ H\|) ) + ε_FRI**, with k⁺ = k + 2, L⁺ = (m + ½)/√ρ⁺ |
| **Remark 9** | p.18 | L⁺ enters linearly, not quadratically as in ethSTARK Thm 4. Powers batching gives C/\|F\|, linear batching 1/\|F\| |
| Remark 10 | p.18 | Two separate FRIs (BGKS20) give L⁺². List-PCS gives L⁺^w |
| §5.3 boosting (extension field; repeated challenges) | p.20 | ε₁ = (L⁺·C/\|F\|)^{N_λ}, ε₂ = (L⁺·(…)/\|F \ (D ∪ H)\|)^{N_z} |
| Conjecture 2 (BGKS20 Conj. 21) | p.21 | |

**What umis / Plonky3 rely on:**
- **Thm 8** is the template for p3-security's `air-composition` (L⁺·C/\|F\|) and `deep-ali` (L⁺·(d(k + 1) + (k − 1))/\|F\|) terms. The linear L⁺ charge is justified by Thm 8 / Remark 9; upstream `p3-security` main now cites them explicitly in `deep.rs`.
- §4.1.3 justifies ρ⁺ in the UD agreement α = (1 + ρ⁺)/2.
- §3.5 is the clearest description of the "choose m, then s" procedure that §3 below follows.

### 1.4 Haböck, *A note on mutual correlated agreement for Reed–Solomon codes* (ePrint 2025/2110, 17 Nov 2025)

- **Local copy:** `tmp/zk-research/papers/eprint-2025-2110.{pdf,txt}` (7 pp.)
- **What it proves:**
  - **Mutual correlated agreement** (MCA, ACFY24 terminology) for RS codes up to the Johnson radius.
  - Outside a small exceptional set of z, *every* codeword close to f₀ + z·f₁ is the combination p₀ + z·p₁ of a pair in the interleaved list.
  - Ordinary correlated agreement only says *some* close pair exists.

| Result | Page | Statement |
|---|---|---|
| Def. 1 MCA | p.2 | Pr_z[∃A, \|A\| ≥ (1 − γ)n: f_z\|_A ∈ C\|_A but [f₀, f₁]\|_A ∉ C²\|_A] ≤ ε(γ) |
| Lemma 1 (AHIV17/BKS18) | p.3 | If p₀ + z·p₁ is γ-close to f_z for \|S\| > ⌈γn⌉ + 1 values of z, the pair is γ-close jointly |
| **Thm 2** | p.3–4 | γ = 1 − (1 + 1/(2m))√ρ, m ≥ 3, ρ = k/n: the exceptional set has **\|E\| ≤ (ℓ⁷/3)·(ρn)²** with **ℓ = (m + ½)/√ρ**. Remark 3 (p.4): the same order as BCI+20 Thm 5.1 |
| Claim 1 | p.5 | Per irreducible factor, \|E_{i,j}\| ≤ (ℓ⁶/3)(ρn)² |
| Scope note | p.2 | Stated for lines. Powers and multilinear combinations are "proven similarly" |

**What umis / Plonky3 rely on:**
- MCA is what makes the **list-decoding analysis of multi-round protocols** sound. Once a fold or batch passes, the folded word's list *is* the fold of the parents' list.
- p3-security cites this paper (as \[Hab25\], together with BCGM26) for the statement that MCA holds at the Johnson radius (`assumption.rs` header and `SecurityAssumption::JohnsonBound`).
- The **numbers** p3-security uses, however, come from DKT26 (§2.3). Its count is O(n/η³), linear in n, whereas Hab25 Thm 2 is quadratic in n.
- For umis, Hab25 is therefore the *qualitative* justification for list-decoding FRI/WHIR composition, not a quantitative input.

### 1.5 Secondary sources consulted

- **`p3-security` 0.8.0** (`~/.cargo/registry/src/*/p3-security-0.8.0/src/`). Module headers cite:
  - BCI+20 and ethSTARK;
  - 2024/1553 (round-by-round FRI);
  - 2025/2010 (random words);
  - BCHKS25 (2025/2055);
  - DKT26 (2026/2056, "Theorem 5.12, App. B.1–B.2" — this is **Thm 4.4** in the 2 Nov 2026 version, see `proximity-gaps-status.md` §2.5).
- **Upstream `p3-security`** (`tmp/zk-research/src/p3/security/`, read only). Differences from 0.8.0 that matter for umis:
  - (a) `fri.rs` documents that **all committed words are assumed to have the maximal height**. A word that joins in a later round makes that round a curve of degree a, not a − 1. The bound is then short by 1 bit at arity 2, 0.42 bits at arity 4, and about 0.19 bits at arity 8. "Callers that commit words of several heights must not use these bounds as they stand."
  - (b) A short last fold (one that can't use the full arity) is now still charged as a round.
  - (c) The LDR commit term is documented as max{(a − 1)·C_DKT26, a(2m+1)(n+1)/(√ρ\|F\|)}, where the second term is from BCI+20 Lemma 8.2. That is numerically the same as 0.8.0.
  - (d) `compute_upper_m` is now computed with exact integer arithmetic.
  - (e) LogUp adds once-per-proof interactions.
  - (f) `deep.rs` cites Haböck Thm 8 / Remark 9 for the linear L⁺.
- **soundcalc** (`tmp/zk-research/src/soundcalc/`). The Ethereum Foundation's calculator. It covers UDR and JBR only, uses round-by-round minima, and grades commit rounds per round.
  - UD linear error (γn + 1)/\|F\| (BCHKS25 Cor. 1.4), against p3-security's slightly looser (n + 1).
  - JB error from BCHKS25 Thm 4.2 (`proxgaps/johnson_bound.py`); p3-security instead uses the newer DKT26 count.
  - Reference point: SP1 v6.1.0 claims **100 bits in UDR** with KoalaBear⁴, ρ = 1/4, 124 queries, 16-bit query PoW and a 248-bit hash (`reports/sp1.md`).
- **Least Authority audit of Plonky3** (31 July 2024, `tmp/zk-research/src/p3/audits/`). FRI-relevant points:
  - **Issue A** (p.8–9, resolved): the verifier trusts the prover's `degree_bits`. Polygon argued that Merkle path lengths pin it.
  - **Issue B** (p.9–10, partially resolved): Fiat–Shamir initialisation does not absorb the FRI configuration (`log_blowup`, `num_queries`, PoW bits, MMCS), only public values and degree. So **FRI parameters must be fixed out of band by the verifier and never read from the proof.**
  - **Issue C** (p.10, resolved in PR #393): `final_poly` was not absorbed before query sampling.
  - **Suggestion 4** (p.15): a soundness calculator. This is now `p3-security`.
  - **p.6, Multi-FRI deviation:** the collinearity check for mixed heights is done only in the last round. The auditors found no issue but "could not reason" about its round-by-round Fiat–Shamir impact.
  - Of these, Issue B and the Multi-FRI deviation still matter for umis.

---

## 2. Reconciliation: every `p3-security` term → paper, theorem, formula

Composite rule (`stark.rs:42` `proven_security_regime`, `stark.rs:195` `regime_report`, `stark.rs:239` `proven_security_report`):
- **security = min over terms**, capped at `collision_resistance`, evaluated separately in UDR (L = 1) and in the best-m LDR (L = L⁺(m)). The reported proven level is **max(UDR, LDR)**. The LDR m is picked by the *full composite*, not by the LDT alone (`ldt.rs` `ldr_candidates`).
- This is round-by-round soundness (ethSTARK Def. 7, Thm 5, Thm 6) composed with the minimum. ethSTARK Cor. 3 would subtract **one more bit** and also require κ ≥ 2λ + 3. p3-security does neither.
- Each PoW site adds its bits to exactly one term (`grinding.rs:62` `boost`; ethSTARK Thm 6, p.47).

In the table, "bits" means −log₂ε. `F` is `shape.modulus_bits`, L = 1 in UDR, and L = L⁺ = (m + ½)/√ρ in LDR.

| p3-security term (label) | Code | Formula as implemented | Source | Differences from / simplifications of the paper |
|---|---|---|---|---|
| **`air-composition`** | `air.rs:11` | F − log₂L − log₂C | Haböck 2022/1216 Thm 8 first term L⁺·C/\|F\| (p.18). ethSTARK Thm 5 ε₁ = ℓ/\|K\| (p.42) for linear batching | (1) L⁺ is computed with the nominal ρ, not ρ⁺ = (k + 2)/n (factor √(1 + 2/k), about 10⁻⁴ bits). (2) **No PoW site exists** before the constraint-folding challenge in Plonky3 0.8, so this term cannot be ground and is a hard ceiling: 118.2 bits UDR and ≈114–116 bits LDR at C = 56 and F = 124 |
| **`deep-ali`** | `deep.rs:29` | F − log₂L − log₂ max{d(k + c' − 1) + (k − 1), (c + 1)k + c' − 1} + z_ood, where c' = `max_combo` = 2 and c = quotient chunks | Haböck Thm 8 second term L⁺·(d(k⁺ − 1) + (k − 1))/(\|F\| − \|D ∪ H\|) with k⁺ = k + 2 (p.18). ethSTARK Thm 5 ε₂ (p.42) charges ℓ² | (1) L enters **linearly** (Haböck Remark 9), not ℓ² as in ethSTARK. (2) The denominator is \|F\|, not \|F\| − \|D ∪ H\| (negligible). (3) **Plonky3-specific** second branch (c + 1)k + c' − 1 for its power-of-two quotient-chunk split. It binds only when c > d. (4) Boosted by `with_ood_proof_of_work_bits` |
| **`low-degree-test`** (proven; the min of commit and query below) | `fri.rs:242` (UDR), `fri.rs:261` (LDR) | min{commit, query} | BCI+20 Lemma 8.2 / Thm 8.3 (p.41–42); ethSTARK Thm 3 (p.30), Thm 5 ε_i, ε_k (p.42); Haböck Thm 2 (p.7) | Reported as a single label in the proven path; split into the two terms below in the conjectured path |
| ↳ query phase, UDR | `fri.rs:231`, `proximity.rs:109` | z_q − s·log₂α, α = (1 + ρ⁺)/2 | UD radius δ = (1 − ρ⁺)/2 (BCI+20 Thm 1.2 UD; Haböck §4.1.3 for ρ⁺). Query acceptance (1 − δ)^s | Precondition k + c' < α·n is checked |
| ↳ query phase, LDR | `fri.rs:231`, `proximity.rs:117` | z_q − s·log₂((1 + 1/(2m))√ρ) | BCI+20 Thm 8.3 last term (p.42); ethSTARK ε_k (p.42); Haböck ε_Q (p.8) | Uses nominal √ρ instead of Haböck's √ρ⁺ (eq. 19). The cost is under 10⁻⁴ bits per query at k = 2^14. Upstream justifies it via the DKT26 B_jet list bound |
| ↳ commit phase, UDR (each fold round) | `fri.rs:165` | F − log₂((a − 1)(n + 1)) + z_commit, with n = the largest LDE domain | BCI+20 Thm 6.1 (curve of degree l = a − 1, \|S\| > l·n; p.29) | (1) The "+1" is slightly conservative. soundcalc uses (a − 1)(γn + 1)/\|F\| (BCHKS25 Cor. 1.4). (2) One n for all rounds (round 0 dominates). (3) Assumes **all words have maximal height** (see §1.5a). (4) `commit_proof_of_work_bits` is ground **before every fold challenge** (`p3-fri` verifier.rs:75) |
| ↳ commit phase, LDR | `fri.rs:196`, `proximity.rs:87` | min of: **(a)** F − log₂C_DKT − log₂(a − 1) + z_c, where log₂C_DKT = 3 + log₂n + 3log₂(m + ½) − log₂3 − log₂ρ⁻ and ρ⁻ = (k − 1)/n; **(b)** F − log₂a − log₂(n + 1) − log₂(2m + 1) + ½log₂ρ + z_c | (a) **DKT26** Johnson line-MCA count (Thm 4.4 current numbering; 5.12 in the cited version), extended to curves by p3-security's own derivation in `proximity.rs` docs. (b) BCI+20 Lemma 8.2 second summand (p.41) / ethSTARK Thm 5 ε_i second summand (p.42) | **The main departure from the four papers.** BCI+20 / ethSTARK / Haböck charge (m + ½)⁷n²/(3ρ^{3/2}\|F\|) here (the quadratic GS count). p3-security uses DKT26's 8n(m + ½)³/(3ρ⁻), which is linear in n and cubic in m. It takes the **larger** of (a) and (b), not their sum (upstream justifies this via DKT26 full agreement sets). The ε₃/t Π t_j^{−2} recursion of ethSTARK Thm 5 is dropped |
| **`batch-combination`** | `stark.rs:123` | UDR: F − log₂n − log₂(B − 1) + z_batch. LDR: F − log₂C_DKT(m) − log₂(B − 1) + z_batch | UDR: BCI+20 Thm 6.1 with l = B − 1 (p.29) = Haböck Thm 1 eq. (1) (p.4). LDR: DKT26 count × (B − 1) curve degree, in place of Haböck Thm 1 eq. (2) / BCI+20 Thm 6.2 | (1) Powers-of-α batching means curve degree B − 1 (not affine: BCI+20 Thm 1.6 would give a factor 1). (2) LDR uses the DKT26 count instead of (m + ½)⁷n² (as above). (3) Boosted by `batch_proof_of_work_bits` (grinding before α in `TwoAdicFriPcs::open`). (4) In the conjectured regime the UD form is used at L = 1. (5) Charging this round in the conjectured regime is a p3-security decision. ethSTARK eq. (19) ignores it |
| **`logup-fingerprint`** (extra) | `logup.rs:85` | F − log₂(N_int·k) − log₂(W + 2) + z_lookup | Haböck, *LogUp* (2022/1530): Schwartz–Zippel on the cleared fraction, plus the vanishing-denominator event (see `fiat-shamir-and-lookups.md`) | Not in the four papers. N grows linearly with trace height, which is why this term binds in the conjectured regime. Upstream adds once-per-proof interactions to N. In-circuit multiplicity overflow (Σ < p) is **not** modelled |
| **`commitment-collision`** | `stark.rs:195` | min with `collision_resistance`, which the caller supplies (umis probe: 124) | ethSTARK §5.10 (p.39: digest ≥ 2λ) and **Cor. 3 (p.49–50): κ ≥ 2λ + 3**; BCS16 via ethSTARK Thm 7–8 | p3-security caps at the caller's number. ethSTARK requires κ ≥ 2λ + 3, which gives 247.9-bit digests λ ≤ 122.4, not 124. The T²/2^κ BCS term and the challenger (duplex sponge) capacity are not modelled separately |
| **`ldt-commit-phase`** (conjectured) | `fri.rs:150` | identical to UDR commit: F − log₂((a − 1)(n + 1)) + z_c | BCI+20 Thm 6.1 count, assumed to hold at the conjectured radius with list size 1 | Not in ethSTARK's eq. (19). The commit round is kept on purpose (the code comments explain why) |
| **`ldt-query-phase`** (conjectured) | `fri.rs:102` | s·(−log₂(ρ + η)) + z_q, with η = (log₂(e/ρ)·ρ)/F | Diamond–Gruen 2025/2010 §1.5 random-words heuristic (see `proximity-gaps-status.md`). It replaces ethSTARK eq. (19)'s R·s | A **heuristic**. The up-to-capacity conjecture (BCI+20 Conj. 8.4) it softens has counterexamples on two-adic prime-field subgroups. The `legacy` regime (`fri.rs:133`) is ethSTARK's R·s + ζ without the −1 |

**Per-query bits by regime (before PoW), umis k = 2^14:**

| lb | UDR −log₂((1 + ρ⁺)/2) | Johnson m = 3 | Johnson m = 10 | Johnson m → ∞ (= lb/2) | random-words "conjectured" | legacy (= lb) |
|---|---|---|---|---|---|---|
| 1 | 0.415 | 0.278 | 0.430 | 0.5 | 0.972 | 1 |
| 2 | 0.678 | 0.778 | 0.930 | 1.0 | 1.960 | 2 |
| 3 | 0.830 | 1.278 | 1.430 | 1.5 | 2.949 | 3 |
| 4 | 0.913 | 1.778 | 1.930 | 2.0 | 3.938 | 4 |

Proven security costs about **2× the queries** of the conjectured/legacy figure (BCI+20 §3.2, p.14: t ≈ 2λ/log(1/ρ)). At lb = 1, UD gives only 0.415 bits per query, which is why `new_benchmark` is 57.5 bits UDR and 65.9 bits LDR.

**Cross-check.** An independent Python re-implementation of the formulas above (UDR, LDR m-search with composite-chosen m, conjectured) reproduces `secprobe/out.txt` to 0.1 bit for both `new_benchmark` and the lb2/arity-4/degree-5 variant (79.9 proven, m = 548, 110.6 conjectured at 2^14; m = 349 at 2^16). The worked numbers below come from that model. Re-check any chosen preset with the real `p3_security::stark::proven_security_report` (the `secprobe` harness).

---

## 3. Parameter-selection recipe for umis

### 3.0 Fixed facts (KoalaBear-specific)

| Fact | Value | Consequence |
|---|---|---|
| p = 2^31 − 2^24 + 1 = 2 130 706 433 | log₂p = 30.9887 | |
| p − 1 = 2^24·127 → **TWO_ADICITY = 24** (`p3-koala-bear-0.8.0/src/koala_bear.rs:69`) | Two-adic FRI domains live in the base field | **log₂k + lb ≤ 24** (one less with hiding/ZK, which doubles the trace). At lb = 3 the maximum trace is 2^21; at lb = 4 it is 2^20. EXT_TWO_ADICITY 26 (deg 4) / 27 (deg 8) doesn't help `TwoAdicFriPcs` |
| \|EF4\| = p⁴ | **2^123.95** (p3-security is fed 124; feeding 123.95 lowers every term by 0.05) | ALI ceiling ≈ 118 bits (UDR) and ≈ 114–116 bits (LDR) at C = 56. **128 proven bits are impossible with EF4** |
| \|EF5\| = p⁵ (`QuinticTrinomialExtensionField`, X⁵ + X² − 1, in p3-field 0.8) | **2^154.94** | Needed for 128 |
| 8-element Poseidon2 digest (`PaddingFreeSponge<Perm16,16,8,8>`, `TruncatedPermutation<Perm16,2,8,16>`) | 247.9 bits → **collision ≈ 2^124** (birthday). ethSTARK Cor. 3 gives λ ≤ 122.4 | **Caps umis at about 122–124 bits whatever the FRI parameters.** The duplex challenger (`DuplexChallenger<Val,Perm16,16,8>`, capacity 8) has the same cap |
| 9-element digest | 278.9 bits → collision 2^139; Cor. 3 gives λ ≤ 137.9 | Needs Poseidon2 width 24 (`default_koalabear_poseidon2_24`) for 2-to-1 compression (2·9 = 18 ≤ 24) and a challenger capacity ≥ 9 |
| Constraint degree vs blowup | Quotient chunks c = next_pow2(d − 1) must be ≤ 2^lb | lb = 1 → d ≤ 3 (today). lb = 2 → d ≤ 5. lb = 3 → d ≤ 9 |
| umis AIR shape (budget 3) | C = 56, d = 3, c = 2, max_combo = 2, B = 46 + 32 + 2·e + 2·14·e (**198 for e = 4, 228 for e = 5**), LogUp 23 interactions/row, W = 5 | B and N scale with the extension degree e and with k |

### 3.1 Step-by-step

1. **Fix the analysed shape at the worst case.**
   - Use the *largest* trace height the verifier will accept (all instances in a batch). Every LDR batch/commit term and the DEEP and LogUp terms lose about 0.5–1 bit per doubling of k. Example: the lb3 preset loses 1.4 bits from 2^14 to 2^16.
   - Derive C, d, c and B from the real AIR, with a layout that includes the preprocessed and permutation widths (`StarkAirParams::from_air` warning in `shape.rs`). Use `p3_batch_stark::security::num_batched_openings` for B.
   - *Assumption:* all committed matrices share the maximal height (true today: main, preprocessed, permutation and quotient are all at k·2^lb). If a second AIR of a different height is added (e.g. a Poseidon2 state-commitment AIR), add ≥ 1 bit of margin (§1.5a).
2. **Pick the challenge field.**
   - The ungrindable ALI term must clear the target: F − log₂L − log₂C ≥ λ.
   - EF4 gives at most ≈ 118 (UDR) / ≈ 115 (LDR). → **EF4 for 100. EF5 for 128.**
3. **Pick the hash.**
   - Digest bits κ ≥ 2λ + 3 (ethSTARK Cor. 3), and collision κ/2 ≥ λ.
   - → 8 elements for ≤ 122. **≥ 9 elements (Poseidon2-24) for 128.**
   - Set `collision_resistance` honestly in the calculator.
4. **Pick log_blowup.**
   - Constraints: d ≤ 2^lb + 1, and log₂k + lb ≤ 24.
   - Cost: prover LDE and Merkle work ∝ 2^lb.
   - Benefit: proven bits per query ≈ lb/2 (Johnson) or 0.4–0.9 (UD). lb = 3 or 4 minimises proof size. lb = 2 is the cheapest prover that is still reasonable.
5. **Pick the regime you are willing to rely on.**
   - **UDR presets** need only BCI+20 Thm 1.2/6.1 UD (2020) plus ethSTARK/Haböck composition. This is the most conservative choice; it costs about 1.6–2× more queries.
   - **Johnson presets** rely on p3-security's use of the **DKT26** line count (2026, very recent; checked independently only in `proximity-gaps-status.md`). With only BCI+20's Johnson count, the same parameters drop by about 17–20 bits (§3.6).
6. **Pick arity and final polynomial.**
   - Arity a costs log₂(a − 1) bits in each commit term (2.8 bits at a = 8), which `commit_proof_of_work_bits` recovers. It saves Merkle openings, so arity 8 (`max_log_arity = 3`) is smallest.
   - `log_final_poly_len` = 3–5 removes the last rounds (proof size −).
   - *Condition:* log₂k + lb > lfp + lb for every committed matrix (`FriProverError::InputHeightTooSmall`).
7. **Place PoW at every grindable site** (ethSTARK Thm 6):

   | Site | Configured by | Term |
   |---|---|---|
   | OOD | `StarkConfig::with_ood_proof_of_work_bits` | `deep-ali` |
   | lookup | `with_lookup_proof_of_work_bits` | `logup` |
   | batch | `FriParameters::batch_proof_of_work_bits` | `batch-combination` |
   | commit | `commit_proof_of_work_bits` (paid once **per fold round**) | `ldt-commit-phase` |
   | query | `query_proof_of_work_bits` | query term |

   - Honest cost ≈ 2^z Poseidon2 permutations per site, i.e. 2^z × the number of rounds for commit.
   - Keep non-query sites ≤ 16 and query ≤ 20 unless measured.
8. **Choose m, then s** (Haböck §3.5, p.8):
   - In LDR the batch and commit terms fall as 3·log₂(m + ½) while the query term rises with m. So:
     - pick the largest m with min(batch, commit) ≥ λ;
     - then s = ⌈(λ − z_q) / (−log₂((1 + 1/(2m))√ρ))⌉.
   - In UDR: s = ⌈(λ − z_q) / (−log₂((1 + ρ⁺)/2))⌉.
   - p3-security searches m ∈ [3, min(1000, m_max)] itself. Just increase s until the composite clears.
9. **Verify and pin.**
   - Run `proven_security_report` at the worst-case shape, with the *same* PoW numbers the transcript uses (`GrindingBudget::check` in `grinding.rs` exists for this).
   - Require ≥ λ + 1 to absorb ethSTARK Cor. 3's union-bound bit.
   - Assert it in CI.
   - Record the parameters in the `stark_config_id` the verifier fixes out of band (audit Issue B).

### 3.2 Worked example of step 8 (2^14, lb = 3, arity 8, EF4, z_commit = 12, z_batch = 16)

- n = 2^17 and ρ⁻ = (2^14 − 1)/2^17.
- log₂C_DKT(m) = 3 + 17 + 3·log₂(m + ½) − log₂3 + 3.0001 = 21.415 + 3·log₂(m + ½).
- **commit** = 124 − 21.415 − 3log₂(m + ½) − log₂7 + 12 = 111.78 − 3log₂(m + ½) ≥ 100 ⇒ m + ½ ≤ 15.2 ⇒ **m ≤ 14**.
- **batch** = 124 − 21.415 − 3log₂(m + ½) − log₂197 + 16 = 110.96 − 3log₂(m + ½) ≥ 100 ⇒ m + ½ ≤ 12.6 ⇒ **m ≤ 12**.
- **query** at m = 11: −log₂((1 + 1/22)·2^−1.5) = 1.436 bits per query ⇒ s = ⌈(100 − 20)/1.436⌉ = **56**, giving 100.4.
- The composite gives 100.4 at m = 11 (s = 56). All of ALI 113.2, DEEP 119.0 and LogUp 118.7 have slack.
- Without any non-query PoW the same s gives 89.5: batch 89.5, LDR commit 94.4. **Batch and commit grinding are not optional in the Johnson regime.**

### 3.3 (a) Target 100 proven bits — EF4, 8-element digest (current field and hash)

**Johnson-regime presets** (rely on DKT26 via p3-security). Every preset uses arity 8 (`max_log_arity = 3`), `log_final_poly_len = 5`, `commit_proof_of_work_bits = 12`, and **16 bits at the OOD, batch and lookup sites**. Each cell gives proven / conjectured bits and the proof-size estimate (secprobe model, about ±3%; it predicted 255 KB against 261 KB measured for `new_benchmark`).

| Preset | lb | s | z_query | 2^14 | 2^16 |
|---|---|---|---|---|---|
| **P100-lb3** (recommended) | 3 | 58 | 20 | **101.7** / 115.4, 153 KB | **100.3** / 113.4, 184 KB |
| P100-lb3-q16 | 3 | 62 | 16 | 102.2 / 115.4, 162 KB | 100.9 / 113.4, 195 KB |
| P100-lb4 | 4 | 44 | 20 | 101.6 / 114.4, 130 KB | 100.5 / 112.4, 155 KB |
| P100-lb2 (d ≤ 5, cheapest prover) | 2 | 86 | 20 | 101.4 / 116.4, 197 KB | 100.4 / 114.4, 239 KB |

**UDR-only presets** (classical BCI+20 UD; no DKT26 dependence). These use arity 8, lfp 5, no commit PoW, and 8 bits at OOD/batch/lookup:

| Preset | lb | s | z_query | 2^14 | 2^16 |
|---|---|---|---|---|---|
| U100-lb3 | 3 | 97 | 20 | 100.5, 239 KB (binding: query) | 100.5, 287 KB (binding: query; batch 105.4) |
| U100-lb4 | 4 | 88 | 20 | ≥ 100, 238 KB | ≥ 100, 285 KB |
| U100-lb2 | 2 | 118 | 20 | ≥ 100, 259 KB | ≥ 100, 314 KB (needs batch PoW ≥ 8) |

For comparison:
- today's `koalabear` = `new_benchmark` gives 65.9 proven (57.5 UDR) and 101.4 conjectured at 2^14 (255–261 KB);
- an SP1-like UDR setting (lb2, s = 124, z_q = 16, arity 2) gives 100.1 at 2^14 but **98.4 at 2^16**, because the batch term (124 − log₂n − log₂197) binds without batch PoW.

### 3.4 (b) Target 128 proven bits — needs EF5 and ≥ 9-element digest

The terms are evaluated with F = 154.9 (fed as 155), collision 139 and B = 228. The settings are the same as §3.3: arity 8, lfp 5, z_commit = 12, 16 bits at OOD/batch/lookup.

| Preset | lb | s | z_q | 2^14 | 2^16 |
|---|---|---|---|---|---|
| **P128-lb3** | 3 | 76 | 20 | **130.2** / 139.0, 221 KB | **129.1** / 139.0, 265 KB |
| P128-lb4 | 4 | 58 | 20 | 130.5, 189 KB | 129.3, 226 KB |
| P128-lb2 | 2 | 113 | 20 | 129.7, 285 KB | 128.8, 346 KB |
| P128-lb3-q16 | 3 | 78 | 16 | 129.5, 226 KB | 128.5, 272 KB |
| U128-lb3 (UDR only, no PoW except query 20) | 3 | 131 | 20 | 128.7, 353 KB | 128.2, 426 KB |
| U128-lb4 (UDR only) | 4 | 119 | 20 | ≥ 128, 353 KB | ≥ 128 (o/b/l 8), 424 KB |

With **EF4**, P128-lb3's FRI parameters give only 105.5 (2^14). The best EF4 configuration found (lb3, s = 114, z_q = 24, UDR) reaches **118.2**, the ALI ceiling. The 124-bit collision cap would bind next.

### 3.5 Conditions and assumptions behind the numbers

- **Fiat–Shamir is modelled as a random oracle** (BCS / ethSTARK §7). Poseidon2 is assumed to behave like one at its capacity. Transcript binding must be complete (see `fiat-shamir-and-lookups.md`: the periodic-column digest, params hash, and the FRI params fixed out of band).
- **Round-by-round minimum.** No −1 bit is applied; add it yourself (λ + 1).
- **Single height.** All committed words share the maximal LDE height (§1.5a).
- **Exact AIR counts:** C = 56, d = 3, B = 198/228, LogUp 23×k interactions with W = 5. If the AIR changes (lookup budget, degree, columns), recompute. A different lookup budget changes C, d, c and B together (the secprobe budget-5 variant: C = 51, d = 5, c = 4, B = 166).
- **No hiding/ZK.** HidingFriPcs adds random codewords to B, doubles the trace (one bit of two-adicity), and changes the chunk count.
- **Johnson presets assume** that p3-security 0.8's DKT26-based counts are correct, including its own curve extension (derived in `proximity.rs` docs and checked there by finite certificates, not in the paper). **UDR presets** assume only BCI+20 UD (Thm 1.2/6.1).
- **Conjectured numbers** (random words) are heuristic. The up-to-capacity versions are refuted. Never use them as the release criterion.

### 3.6 What the numbers would be under BCI+20's own Johnson bound

Re-evaluating with the 2020/654 Lemma 8.2 / Haböck eq. (7) counts gives: batch round (B − ½)(m + ½)⁷n²/(3ρ^{3/2}\|F\|); fold rounds (2m + 1)(n + 1)a/(√ρ\|F\|); L⁺ with ρ⁺. The other parameters are unchanged:
- P100-lb3: **82.8** at 2^14 and **78.8** at 2^16 (binding: batch, m = 3);
- P128-lb3 on EF5: **113.6**;
- `new_benchmark`: 60.2.

So **every Johnson-regime "≥ 100" claim for umis is a claim about DKT26-era bounds**, not about the four foundational papers. The UDR presets are the fallback that doesn't depend on any 2025–26 result.

---

## 4. "Must not get wrong" checklist (anyone touching FRI/PCS parameters)

1. **Don't quote lb·s + z_q.**
   - `FriParameters::conjectured_soundness_bits()` (the "~116" in `config.rs:13`) is ethSTARK's refuted toy-problem formula without even its −1.
   - Quote `proven_security_report` (UDR and LDR), state which regime, and name the paper the bound depends on (BCI+20 UD vs DKT26 Johnson).
2. **Analyse the largest height the verifier accepts.**
   - Security falls with k (batch, commit, DEEP and LogUp all grow with n or k).
   - If multi-slot proofs (n_z·k_sub·C rows) or new AIRs arrive, re-run at the new maximum.
   - The verifier must bound `degree_bits` from the statement, not trust the proof (audit Issue A).
3. **Respect two-adicity:** log₂k + lb ≤ 24 for KoalaBear (one less with ZK). lb = 4 caps traces at 2^20.
4. **Match blowup to constraint degree:** next_pow2(d − 1) ≤ 2^lb. Nonlinear physics with d = 4–5 needs lb ≥ 2. Pass the preset's `log_blowup` into the `ProverData::from_airs_and_degrees_with_lookup_budgets(…, &[3], 1)` call in `setup()` (`crates/task-water3d/src/air.rs:468-474`) instead of the literal `1`. Otherwise the degree budget silently goes stale. The `&[3]` there is the lookup budget, which sets C, d and B.
5. **Every PoW bit fed to the calculator must actually be ground by the transcript, and vice versa** (the "two places, one number" rule in `grinding.rs`).
   - OOD and lookup PoW default to **0** in `StarkConfig::new`; set them explicitly.
   - Commit PoW is per fold round; batch PoW is before the PCS batching α.
   - **There is no ALI grinding site.** The ALI term is a hard ceiling set by \|EF\|.
6. **Count B correctly.**
   - It includes the permutation trace opened at ζ and ζ·g, and every extension-field column counts e times.
   - Changing EF4 → EF5 changes B (198 → 228) and the opened-value size.
   - Use `p3_batch_stark::security::num_batched_openings`. Build `StarkAirParams` with a layout that has *all* widths (preprocessed and permutation). A base-only layout drops constraints and **overstates** security (`shape.rs` docs).
7. **The field caps security:**
   - EF4: ALI ≤ 118.2 (UDR), about 115 (LDR) at C = 56; \|EF4\| ≈ 2^123.95, not 2^124.
   - **128 needs EF5** (`QuinticTrinomialExtensionField`), not more queries.
8. **The hash caps security:**
   - An 8-element Poseidon2 digest and capacity-8 challenger give about 124 bits collision, and ≤ 122.4 by ethSTARK Cor. 3 (κ ≥ 2λ + 3).
   - **128 needs ≥ 9-element digests, Poseidon2-24 compression and challenger capacity ≥ 9.**
   - Set `collision_resistance` to the real value.
9. **Keep a margin:** require ≥ λ + 1 (Cor. 3's union-bound bit).
   - Add ≥ 1 more bit if words of different heights share one FRI (upstream `fri.rs` warning).
   - Add a further 1 bit if you rely on the Multi-FRI "last-round-only" collinearity check that the 2024 audit could not analyse.
10. **Johnson-regime numbers depend on DKT26** (via p3-security's `johnson_exceptional_line_count_log2`).
    - If policy is "classical results only", use the UDR presets (§3.3/3.4).
    - Re-run after every `p3-security` bump. The formulas keep moving: 0.8.0 switched to the DKT26 count (#2282), and upstream `main` has since changed the short-last-round accounting and `compute_upper_m`, and added the mixed-height warning.
11. **Check `log_final_poly_len`:** lfp + lb < log₂(LDE height) of every matrix (`FriProverError::InputHeightTooSmall`). Small test traces can trip this.
12. **Arity costs log₂(a − 1) bits** at every commit term. Raising `max_log_arity` without raising `commit_proof_of_work_bits` can *lower* proven security.
13. **LogUp term:**
    - It grows with k and the interaction count. Its PoW is `with_lookup_proof_of_work_bits`.
    - The multiplicity bound Σ mᵢ < p is a separate in-circuit obligation, not covered by the calculator. umis: 19 × 2^14 ≪ p. Re-check at larger k.
14. **FRI parameters aren't absorbed into the transcript** (audit Issue B).
    - They must be part of the out-of-band verifying configuration (`stark_config_id` / genesis) and validated there.
    - **Reject `koalabear-test` (4 queries, about 2 bits) outside dev genesis.**
15. **Grinding makes proof bytes nondeterministic** (parallel nonce search; see the `FriPreset::Test` comment).
    - Golden-vector and native-vs-wasm byte-equality tests must use a zero-PoW preset, or pin the nonce.
    - Proof addresses (`proof_address`) of PoW presets differ between provers.
16. **Proof-size and time budgets:**
    - Moving from `new_benchmark` (261 KB) to P100-lb3 cuts the proof to about 150 KB, but quadruples LDE/Merkle work (lb 1 → 3) and adds about 2^20 + 3·2^16 + 3·2^12 Poseidon2 permutations of grinding.
    - Benchmark both native (with AVX2 enabled) and wasm before switching. If a 20-bit query grind is too slow in wasm, use the z_q = 16 variants.
17. **Don't mix regimes inside one claim.** UDR and LDR are separate analyses; p3-security reports the max of the two. A term taken from one regime may not be combined with terms from the other.

---

## 5. Pointers

- Probe harness reproducing the p3-security numbers: `tmp/zk-research/secprobe/` (output in `out.txt`).
- p3-security 0.8.0 key functions:

  | File | Functions |
  |---|---|
  | `fri.rs` | 102 conjectured, 133 legacy, 165 UDR commit, 196 LDR commit, 231 query, 242/261 proven LDT, 319 m-candidates |
  | `proximity.rs` | 87 DKT26 count, 109/117 α, 159 L⁺, 172 m_max |
  | `stark.rs` | 123 batching, 195 regime_report, 239 proven report, 399 conjectured report |
  | `deep.rs` | 29 |
  | `air.rs` | 11 |
  | `logup.rs` | 85 |
  | `grinding.rs` | 62 |

- Plonky3 FRI presets: `p3-fri-0.8.0/src/config.rs` (`new_benchmark` at line 179; `fold_schedule` at line 323).
- umis presets: `crates/umis-stark/src/config.rs`.
