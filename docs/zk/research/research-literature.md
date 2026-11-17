# ZK / STARK landscape for umis (as of 2026-11-03)

Scope: the water3d per-slot proof uses Plonky3 0.8.0 (`p3-batch-stark`), KoalaBear with a degree-4 extension, a Poseidon2 Merkle MMCS, `TwoAdicFriPcs` with the FRI `new_benchmark` preset (`log_blowup=1`, 100 queries, 16 query-PoW bits, 0 commit/batch PoW, arity 2), and LogUp. The trace is 2^14 rows: 4 substeps × 4096 cells. It has 46 main and 32 preprocessed columns, and constraint degree is at most 3. Proofs are about 261 KB. They are verified one by one on the beacon chain.

Provenance legend:
- **[V]** I verified this against a primary source: code, release notes, an advisory, an abstract or a PDF.
- **[C]** I computed this myself with a re-implementation of Plonky3's `p3-security` formulas (script at `scratchpad/scripts/sec.py`, `sec2.py`).
- **[S]** This comes from a secondary source (blog, news or summary).
- **[U]** Unverified, or my own estimate.

---

## 0. Executive summary

1. **Plonky3.** The latest release is `0.9.0-rc.2`, published 2026-10-16 on crates.io and GitHub. There is no stable 0.9.0 yet. 0.8.0 came out on 2026-09-23 [V]. 0.9 is a large breaking release: typed Fiat–Shamir transcripts (DSFS), a `p3-security` soundness calculator, STIR/WHIR/Circle PCS, the multilinear `p3-multi-stark` (SuperSpartan-style, with LogUp-GKR buses), binary-field PCS, HVZK sumcheck and WHIR, and zk-codes. `Plonky3-recursion` (`p3-recursion`) tracks 0.9.0-rc.2. It already does recursive verification of uni-stark and batch-stark proofs, 2-to-1 aggregation, FRI or WHIR inside the circuit, and ZK. It is **unaudited** [V].
2. **Security of the current config.** The comment in umis `config.rs` ("~116 conjectured bits") uses the obsolete ethSTARK formula `Q·log_blowup + pow`. Using the formulas Plonky3 itself now ships [C]:
   - **Conservative conjectured: about 102 bits.** This is capped by the batching, DEEP and commit terms in a 124-bit field, not by the queries.
   - **Proven, Johnson regime: about 66 bits.**
   - **Proven, unique decoding: about 57.5 bits.**

   The "up-to-capacity" conjectures behind the 116-bit figure were **disproven in late 2025** (Diamond–Gruen; Crites–Stewart; BCHKS; Krachun–Kazanin–Haböck 2026) [V]. The Ethereum Foundation now accepts only *provable* security: 100 bits by May 2026 and 128 bits by the end of 2026, measured with `soundcalc` [V].
3. **Cheapest move to about 100 proven bits on the quartic field.** Set `log_blowup=2`, use about 100–124 queries, 16 query-PoW bits, and about 10 bits each of batch and commit PoW. This gives roughly 100–102 bits (Johnson) [C]. It also lifts the constraint-degree cap from 3 to 5, which the planned degree-2 nonlinear physics needs.

   **128 proven bits needs a quintic extension (KoalaBear⁵)** and a wider digest: an 8-element KoalaBear digest has only about 124-bit collision resistance [C/V].
4. **Aggregation and IVC.** The lowest-friction route is `p3-recursion`: same field, same Poseidon2 and same batch-stark, with 2-to-1 aggregation trees and FRI or WHIR. zkVM wrapping (SP1, OpenVM, RISC Zero) is heavier. It only makes sense for a final, EVM-friendly wrap. Folding schemes (Nova family, LatticeFold+, Neo/SuperNeo, Arc) do not aggregate existing STARK proofs. They would mean re-arithmetising the step function, and only Nova has mature Rust crates.
5. **Browser proving.** Plonky3's KoalaBear (`monty-31`) has **no wasm32 `simd128` packed field**. Only Goldilocks, Keccak, SHA-256 and BLAKE3 have wasm SIMD paths [V]. So umis' 682 ms single-thread wasm prove runs fully scalar. Stwo does ship a wasm `simd128` M31 multiply [V]. A `PackedMontyField31` for wasm, plus `wasm-bindgen-rayon` with cross-origin isolation, are the two clear browser speed levers.

---

## 1. Plonky3 status

### 1.1 Versions [V]

- `p3-batch-stark` on crates.io: **max_version 0.9.0-rc.2** (2026-10-16). Earlier releases: 0.9.0-rc.1 (10-11), 0.8.0 (09-23), 0.7.0 (09-04), 0.6.x (Jun–Jul), 0.5.x (Mar–Jul) and 0.4.3.
- GitHub releases use per-crate tags such as `p3-batch-stark-v0.9.0-rc.2`. `CHANGELOG.md` has not been updated since 0.3.0, so the release notes are the changelog.
- New crates on crates.io at 0.9.0-rc.2: `p3-whir`, `p3-stir`, `p3-circle`, `p3-security`, `p3-multi-stark`, `p3-zk-codes`, plus `sumcheck`, `gkr`, `bus`, `binary-*`, `word` and `bitz` in the repo.
- `git log p3-batch-stark-v0.8.0..v0.9.0-rc.2` has 108 commits. Most are binary-field and multi-STARK performance work.
  - Fixes relevant to batch-stark users:
    - `fix(lookup): reject local lookup weight overflow instead of saturating` (#2379). With saturation, LogUp multiplicities could wrap mod p.
    - `fix(uni-stark)!: reject mis-shaped input in the public verifier helpers` (#2401).
    - `fix(fri,merkle-tree): release the hiding RNG lock before parallel work` (#2368).
    - Several Merkle-cap and matrix bound checks.
  - `fix(air,bus,multi-stark)!: bind periodic values into the transcript` (#2381) applies **only to multi-stark**. batch-stark still does not absorb periodic columns. umis already works around this by hashing `X_in`, `X_out` and the halos into the public keccak digest, which is the correct mitigation.
- Many soundness and hygiene PRs merged *before* 0.8.0, so umis on 0.8.0 already has them:
  - **#2033** — the FRI verifier used to accept a *prover-chosen folding schedule*. It now derives it. This only matters for `max_log_arity>1`; umis uses arity 2.
  - **#2048** — security-report accounting fixes.
  - **#2100** — enforces PCS budgets and composes multi-STARK soundness. It also fixed a STIR case that delivered 86.76 bits against a 100-bit target.
  - **#2106** — proof-of-work witnesses at **zero difficulty were unbound** (FRI `batch_pow_witness` and `commit_pow_witnesses` and others). This let proofs be **malleable**. The PR says batch-stark itself was already correct. umis uses 0 commit and batch PoW and hashes proof bytes on the beacon chain, so this one matters.
  - **#2199** — PoW grinding squeezes the transcript first. This breaks the proof format.

### 1.2 Feature status relevant to umis [V unless marked]

- **Lookups/LogUp.** `p3-lookup` has local and global LogUp buses. On the multi-STARK path there are also multilinear lookups, logUp* indexed lookups (#2145–2150) and GKR-based buses (`p3-bus`, `p3-gkr`). There is also a timestamped read-write memory bus (#2314/#2317). That is an alternative to umis' hand-built `(addr, v0..v3)` memory bus, but it lives on the multi-stark path, not batch-stark.
- **Periodic columns.** Supported. #2197 packs only the repeating period. They are not bound in batch-stark (see above).
- **Public values.** In batch-stark 0.8 they are absorbed in `main_phase` (checked in `prover.rs`). Boundary-IO public cells (#1947) are multi-STARK only. batch-stark asserts that `public_boundary_io()` is empty.
- **Preprocessed columns.** Supported. The commitment is absorbed (`preprocessed_phase`). #2281 regroups the preprocessed openings, which is a breaking change.
- **ZK/hiding.** batch-stark has `config.is_zk()` with `HidingFriPcs` and a hiding MMCS. Enabling it doubles the trace degree and quotient chunks. A comment in the source says it is "only statistically ZK". #2100 adds masking-budget enforcement. Newer HVZK pieces: an HVZK sumcheck (#2260, which requires a CSPRNG) and HVZK-WHIR (Chiesa–Fenzi–Weissenberg, ePrint 2026/391).
- **WHIR/STIR/Circle.** STIR is wired into uni-stark (#2037). WHIR is used in multi-stark and binary PCS. The Circle PCS runs over Mersenne31. Recursion supports FRI and WHIR.
- **Security calculator.** `p3-security` 0.9.0-rc.2 implements unique-decoding, Johnson-bound (with the DKT26 line-MCA count) and "conjectured" (Diamond–Gruen random-words) regimes, plus a "legacy" ethSTARK regime. #2064 notes that legacy is "for historical comparison, not a soundness bound". `prove_with_security` and `verify_with_security` exist for multi-STARK (#2100).
- **wasm.** Plonky3 has wasm32 `simd128` code for Goldilocks (packing, Poseidon2, MDS), Keccak, SHA-256 and BLAKE3. `monty-31`, which covers KoalaBear and BabyBear, has only `aarch64_neon`, `x86_64_avx2`, `x86_64_avx512` and `no_packing` back-ends. **On wasm, KoalaBear arithmetic, DFTs and Poseidon2 are scalar.**

### 1.3 Plonky3-recursion [V]

- Repo `Plonky3/Plonky3-recursion` was pushed 2026-10-29 and depends on `p3-* 0.9.0-rc.2`. The crates.io `p3-recursion` is stale at 0.1.0 (2025-08). Use git.
- Features:
  - A fixed recursive verifier for uni-stark and batch-stark proofs.
  - 2-to-1 aggregation of heterogeneous children: `build_and_prove_aggregation_layer`, `PreparedAggregation`, and `--arity4` for FRI.
  - FRI or WHIR in-circuit, a Poseidon2/Poseidon1 non-primitive op, ZK, and a KoalaBear quintic option.
  - Default per-term security targets: 124 bits for FRI and 66 bits for WHIR.
- The README warns: "under active development and hasn't been audited yet".
- `PreparedLayer`'s check is shape-only and does not bind child keys or statements. Use `TrustedPreparedLayer` or `TrustedPreparedAggregation` for a verifier-authoritative circuit.
- No published KoalaBear aggregation timings were found [S]. A June 2026 third-party spike (zk-coins) confirmed that 2-to-1 aggregation composes, on Goldilocks only.

### 1.4 Security advisories (GitHub GHSA) [V]

| ID | Date | Severity | Issue | Affects umis 0.8? |
|---|---|---|---|---|
| GHSA-vrmm-4mm5-38vm | 2025-01 | high | Purported opened values not absorbed in the transcript (a classic weak-Fiat–Shamir bug) | No (fixed long before) |
| GHSA-m23j-cj9m-ppg9 | 2025-03 | high | Missing size checks in the FRI verifier | No |
| GHSA-f69f-5fx9-w9r9 | 2025-06 | high | Missing final-polynomial degree check in the FRI verifier | No |
| GHSA-3g92-f9ch-qjcm | 2026-04 | low | Sponge not collision-resistant across input lengths (<0.6) | No (≥0.6) |
| GHSA-vj64-rjf3-w3v7 / CVE-2026-46654 | 2026-05 | high | `MultiField32Challenger` transcript malleability and entropy loss (fixed 0.4.3 / 0.5.3) | No: umis uses `DuplexChallenger` over KoalaBear |

There is also a Least Authority audit PDF in `audits/` (Polygon Plonky3, updated final report). It predates batch-stark and LogUp.

---

## 2. FRI/STARK soundness, late 2025 to 2026

### 2.1 What changed

- **Proven baseline.**
  - BCIKS20 (ePrint 2020/654) proves proximity gaps up to the Johnson radius `1−√ρ`.
  - ethSTARK (2021/582) used the *conjectured* formula `bits ≈ Q·log₂(1/ρ) + pow` (Conjecture: list-decoding and correlated agreement up to capacity).
  - BCHKS25 (Ben-Sasson, Carmon, Haböck, Kopparty, Saraf, ePrint 2025/2055) improves the Johnson-regime exceptional count from O(n²) to O(n). Plonky3's docs say this enables "128-bit provable security with degree-5 extensions of small prime fields (e.g. KoalaBear)".
  - Haböck 2025/2110 proves mutual correlated agreement at Johnson.
  - DKT26 (Dao–Kominers–Thaler, ePrint 2026/2056, Sept 2026) tightens MCA above Johnson from `O(n/(qη⁵))` to `O(n/(qη³))`. It also gives explicit beyond-Johnson bounds "in sufficiently large characteristic". It reports proof-size savings of 4.6–11% for ZisK, LambdaVM and ProveKit, and its bounds are formalised in Lean (ArkLib) [V abstract].
- **Disproofs.**
  - Diamond–Gruen (ePrint 2025/2010) give explicit RS families showing the BCIKS capacity conjecture is false. Their families have rate → 0. They suggest restricting the conjecture to rate ≥ ρ₀ > 0. They propose a "random-words" query estimate, `bits/query ≈ −log₂(ρ + log₂(e/ρ)·ρ/log₂q)` [V abstract; formula S, matches `p3-security`].
  - Crites–Stewart (2025/2046) disprove capacity correlated agreement (BCIKS), WHIR's mutual-CA-to-capacity and DEEP-FRI's list-decodability-to-capacity. They also show that small-error correlated agreement implies list decoding [V].
  - Krachun–Kazanin–Haböck (2026/782) give a simple counterexample over *multiplicative subgroups of prime fields*, the exact STARK setting. Gaps fail at `δ = 1−ρ−η` for `η = Θ(1/log n)` [V]. Kambiré (arXiv 2604.09724) writes up the full proof.
  - Over binary fields, beyond-Johnson gaps fail (Dao–Kominers–Thaler–Zheng, 2026/2424, with an example on Binius64's domain) [V].
  - For general linear codes, the 1.5-Johnson bound is tight (2026/1894).
- **Positive asymptotics (2026).** Brakensiek–Chen–Putterman–Zhang–Zheng (arXiv 2609.08005) and Jeronimo (2609.05870) give poly-time list decoding to capacity (BCPZZ proves list decoding only; the MCA results are Jeronimo's and Zheng's — corrected 2026-11-03), and MCA error `n^{O_γ(1)}/q`, for RS over **prime fields** with q ≥ n [V PDF]. The constants are not concrete enough for parameter setting. The results are stated for prime fields, while STARK challenges live in extension fields. Treat them as "the gap may close eventually", not as usable bounds today [U].
- **Practical guidance.**
  - zkSecurity (Nov 2025) says moving to proven parameters roughly **doubles proof size and verifier time** (prover mostly unaffected), while the "nearest new conjecture" costs only about 2–3% [S].
  - EF "zkEVM security foundations" (2025-12-18) [V]:
    - Milestone 1: soundcalc integration by 2026-02.
    - Milestone 2: **100-bit provable security and ≤600 KiB by end of May 2026**.
    - Milestone 3: **128-bit provable security, ≤300 KiB and a formal recursion-soundness argument by end of 2026**. Only provable security is accepted.
  - EF "better codes" challenge (2026-08-20): a Lean-checked benchmark (koalaIRS12) to push proven bounds toward 128 bits [V]. The reported certificates are 63.99 bits (lower) and 116.13 bits (upper) [S, not confirmed on the EF page].
  - `soundcalc` removed its capacity-bound regime after DG25 and CS25 and integrated BCHKS25 [V README]. Current report [V `reports/summary.md`]:
    - SP1 6.1: KoalaBear⁴, **ρ=1/4, 124 queries, 16 PoW → 100 bits UDR**, 524 KiB.
    - OpenVM2 (WHIR, BabyBear⁴): 100 bits.
    - Venus and ZisK (Goldilocks³): 128 bits JBR, 240–270 KiB.
    - zkDTVM (KoalaBear⁵): 128 bits.

### 2.2 Bits for umis' exact config [C]

Model: `p3-security` (main, Oct 2026) formulas, re-implemented. Assumptions:
- trace 2^14 and LDE 2^15;
- |F| = KoalaBear⁴ ≈ 2^124;
- about 60 constraints, max degree 3, 2 quotient chunks;
- **about 150 batched opened functions** (my estimate) [U];
- collision cap 124 bits (8-element KoalaBear digest ≈ 248 bits);
- round-by-round minimum composition, as in `p3-security`.

| Config | Conjectured (random words) | LDT query term (conj.) | Legacy ethSTARK | Proven UDR | Proven JB (best m) |
|---|---|---|---|---|---|
| **umis now**: lb1, Q100, qpow16, quartic | **101.8** (batching-limited) | 113.2 | 116 (LDT only) | **57.5** | **65.9** (query-limited) |
| lb1, Q100, +batch/commit PoW 10 | 108.0 (DEEP-limited) | 113.2 | — | 57.5 | 65.9 |
| lb1, Q200, b10/c10, quartic | 108.0 | 210 | — | 99.0 | 99.6 |
| lb2, Q100, b10/c10, quartic | 108.0 | 212 | — | 83.8 | **100.0** |
| lb2, Q124, b10/c10, quartic | 108.0 | 259 | — | 100.1 | **101.9** |
| lb3, Q70, b10/c10, quartic | 108.0 | 222 | — | 74.1 | 99.9 |
| lb2, Q124, qpow20, b10/c10, **quintic** | 124 (collision cap) | 264 | — | 104.1 | **124.0** |
| lb1, Q250, qpow20, b8, quintic | 124 | 264 | — | 123.7 | 124.0 |

Per-query rates:
- At ρ=1/2, proven per-query soundness is about 0.415 bits (UDR: `log₂(1/α)` with `α=(1+ρ⁺)/2`) or about 0.50 bits (Johnson). That is why 100 queries give only about 42–50 bits plus 16 PoW.
- The random-words estimate gives about 0.97 bits per query, slightly under the legacy 1.0.

Conclusions:
- **(a)** On quartic KoalaBear, the proven ceiling is about 100–108 bits regardless of queries. The batching and commit-phase terms are `≈124 − log₂(n·poly(m)) − log₂(#functions)`, and the DEEP term is `124 − log₂(4k)`. Getting there needs PoW at the batch and commit sites.
- **(b)** ≥128 proven bits needs KoalaBear⁵ and a digest of 9 or more KoalaBear elements, or a 256-bit hash.
- **(c)** Prover cost: going from lb1 to lb2 doubles the LDE and commitment work (about 1.6–2× the prove time [U]). Proof size scales with Q × (Merkle path + leaf openings): lb2 with Q≈110 should land near 1.1–1.3× today's 261 KB [U]. That is cheaper than lb1 with Q=200, which would be about 2× [U].
- **(d)** lb2 also allows degree-5 constraints, the quotient degree being 4.

### 2.3 Fiat–Shamir

- **Weak FS / "Frozen Heart".** Dao–Miller–Wright–Grubbs (ePrint 2023/691, IEEE S&P 2023) found 36 weak-FS implementations across 12 proof systems [V]. The fix is to hash the full statement (circuit/AIR description, public inputs and all commitments) before the first challenge. Plonky3's 2025 GHSA (opened values not in the transcript) was this class of bug.
- **Plonky3 0.9 typed transcript.** Plonky3 0.9 adds a typed DSFS (Duplex-Sponge Fiat–Shamir, IETF draft) transcript layer with per-protocol domain separation (#1603 and the #2035–#2129 series). It includes tests that no two protocols share a seed.
- **What umis must still bind itself** [U, advisory]:
  - the zone parameters and coefficients: the preprocessed commitment is absorbed, but the verifier must take it from its *own* verifying key, not from the proof;
  - the periodic data, which umis already hashes into the public digest;
  - the slot and beacon context, so proofs cannot be replayed across slots.
- **Khovratovich–Rothblum–Soukhanov** (ePrint 2025/118, "How to Prove False Statements") give practical FS attacks on a GKR-based argument where the circuit can depend on the hash [V]. This matters if umis moves to GKR or LogUp-GKR (multi-stark, Stwo) or to recursion circuits that evaluate the same hash in-circuit. Keep statements and circuits fixed and absorbed, and do not let the prover choose the circuit.
- **PoW malleability** (#2106): if proof bytes are used as IDs or hashed into blocks, require canonical encodings or hash only the statement, not the proof.

---

## 3. Alternative and upgrade primitives

Columns: (a) per-slot stencil proofs; (b) aggregating many slot proofs per beacon slot; (c) IVC over a zone's chain.

| System | Maturity / crate | wasm | Size / verify | (a) | (b) | (c) |
|---|---|---|---|---|---|---|
| **Plonky3 batch-STARK + FRI** (current) | Production-used by SP1 and others; v0.8/0.9-rc | Works single-thread; no KoalaBear SIMD | 261 KB, 11 ms | Good | via p3-recursion | via p3-recursion chain |
| **STIR** (Arnon–Chiesa–Fenzi–Yogev, CRYPTO'24, ePrint 2024/390) | `p3-stir` 0.9-rc, wired into uni-stark | Same as Plonky3 | 1.25–2.46× smaller than FRI; e.g. 114 KiB vs 211 KiB at 2^26, ρ=1/4, 128 bits [V] | Smaller proofs, similar prover; uni-stark only | Smaller recursion leaves | — |
| **WHIR** (ePrint 2024/1586) | `p3-whir` 0.9-rc; `WizardOfMenlo/whir` (arkworks); used by OpenVM2 SWIRL and ProveKit | Plausible | 63 KiB, 360 µs verify for degree 2^22 at 100 bits [V] | Needs multilinear (multi-stark) path | **Best fit for recursion**: cheap verifier circuit; p3-recursion supports `--pcs whir` | Good |
| **Circle STARK / Stwo** (Haböck–Levit–Papini, ePrint 2024/278) | `stwo` 2.3.0 crates.io; nightly-2026-01-15 toolchain; production (SHARP/Starknet) | **Has a wasm simd128 M31 mul** [V]; zkSecurity reports WebGPU work giving 2× on the native pipeline [S] | ~1.4× vs BabyBear in the paper [V]; ZK-ACE: 341-row AIR in 14.5 ms, 107 KB, 1.1 ms verify [S] | Fast M31, GKR-LogUp built in. **Port cost is high**: new AIR DSL, different field (M31, |F|=2^31−1, QM31 challenges) | Recursion via Cairo verifier (heavy) | Possible but tooling Cairo-centric |
| **LogUp-GKR** (Papini–Haböck, ePrint 2023/1284) | In Stwo; in Plonky3 multi-stark buses (#2132, `p3-bus`) | — | Removes the committed LogUp helper columns | Saves committed width for the memory bus and range checks | — | — |
| **Plonky3 multi-STARK** (SuperSpartan-style zerocheck + WHIR + GKR bus) | 0.9-rc, new; `prove_with_security` | — | Not benchmarked here | Promising: sumcheck-native; linear stencil maps well; neighbour reads become shifted openings [U] | Recursion support unclear (p3-recursion targets uni- and batch-stark) | — |
| **Binius / Binius64** (Diamond–Posen, ePrint 2023/1784) | Old `binius` archived; `binius-zk/binius64` active, no releases | `wasm-binius` exists (old) [S] | — | Poor fit: integer arithmetic over GF(2) towers; beyond-Johnson gaps fail there (2026/2424) | — | — |
| **SuperSpartan / Spartan-WHIR** (CCS: Setty–Thaler–Wahby 2023/552) | `spartan2` 0.9.0 (2026-05); ProveKit (Spartan+WHIR, R1CS from Noir), deployed in World ID on mobile [S] | ProveKit targets mobile | Spartan-WHIR PIOP ≈2.6 KB plus PCS [S, unattributed] | Viable for R1CS-shaped steps | WHIR verifier is recursion-friendly | — |
| **Nova / HyperNova / ProtoStar** (2021/370, 2023/573, 2023/620) | `nova-snark` 0.76 (2026-09) | MSM in wasm is slow; curve cycle | Constant-size IVC; final SNARK | Re-arithmetise as R1CS/CCS; not post-quantum | **Cannot fold existing STARK proofs** | Natural IVC, mature |
| **LatticeFold / LatticeFold+** (Boneh–Chen, 2024/257, 2025/247 CRYPTO'25) | Research code | — | LF+: 5–10× faster prover than LF [V] | — | — | PQ IVC over small fields; research |
| **Neo / SuperNeo** (Nguyen–Setty, 2025/294, superseded by 2026/242) | Research (MSR) | — | Pay-per-bit Ajtai commitments, small-field sumcheck, Module-SIS [S] | — | — | Best theoretical fit for small-integer IVC; no production crate [U] |
| **Arc** (Bünz–Mishra–Nguyen–Wang, ePrint 2024/1731, CRYPTO'25) | Research | — | Hash-based accumulation of RS proximity claims, unbounded depth [V] | — | "Almost drop-in" for IOP-based PCD: amortises FRI/WHIR verification across slots | Yes |
| **Jolt / Lattice Jolt** (2023/1217; Twist & Shout 2025/105) | Alpha; Lattice Jolt (Akita PCS, Module-SIS) released 2026-09-09; `jolt-core` crate stale | — | <100 KB claimed [S] | Overkill for a fixed stencil | RISC-V verifier of Plonky3 proofs possible | — |
| **SP1 (Hypercube, v6.10)** | Production; KoalaBear⁴, Jagged PCS + FRI; soundcalc 100 bits UDR [V]; l2beat says 98 [S] | No browser prover | 524 KiB worst-case (soundcalc) | — | Guest program verifies N Plonky3 proofs, then Groth16/Plonk wrap: heavy but EVM-ready | Yes |
| **OpenVM 2.0** (SWIRL: GKR-LogUp + WHIR) | v2.0.2, production since 2026-07 [S]; audited by zkSecurity | — | <300 kB, 100 bits provable [S] | — | Same as SP1 | Yes |
| **RISC Zero** | v3.1.0-rc.1 / `risc0-zkvm` 5.0.0-rc.1 (2026-10-30) | — | soundcalc report exists (not read) | — | zkVM route | Yes |

Fit notes for umis:

**(a) Per-slot proofs.**
- Stay on Plonky3 batch-stark.
- The biggest wins are engineering, not protocol: wasm SIMD for MontyField31 and multi-thread wasm.
- Next, consider:
  - **lb2** for both security and degree headroom;
  - **STIR or WHIR** to shrink the proof;
  - LogUp-GKR / multi-stark to drop committed lookup helper columns.
- About half the main width (`nb[6][4]` = 24 of 46 columns) is copies of neighbour values that the memory bus re-checks. A sumcheck-based (multi-stark/GKR) design could serve these as evaluation claims on `qn` at shifted points instead of committed columns [U, design idea].

**(b) Aggregation per beacon slot.**
- Use `p3-recursion` 2-to-1 trees, optionally `--arity4`, with WHIR in the inner layers (smaller in-circuit verifier).
- Then a single FRI or WHIR root proof.
- Upgrade to 0.9 first: p3-recursion pins 0.9.0-rc.2 and transcripts changed incompatibly.
- Arc-style accumulation is the research alternative.

**(c) Zone IVC.**
- Recursion chain with `PreparedLayer` and the trusted variant.
- Lattice folding (Neo, LatticeFold+) is the long-term PQ option for tiny-step IVC, but immature.

---

## 4. Prior art: physics, stencils, cellular automata and browser proving

- **Direct "zk-physics" / PDE work: essentially none found** [S].
  - Closest examples: GoL2, a Conway's Game of Life Starknet/Cairo contract from 2022 that is no longer maintained [S], and hackathon projects (ZKLife) [S].
  - No paper benchmarks STARK proofs of finite-difference or finite-volume solvers. umis' integer stencil with `floor(·/2^s)` via remainder plus range check appears to be novel as a deployed design [U].
- **Convolutions = stencils (zkML).**
  - zkCNN (Liu–Xie–Zhang, CCS 2021) proves convolutions with an O(N) FFT sumcheck. It reports VGG-16 (15M params) at about 88 s to prove and 341 KB of proof [S].
  - Later zkML work (zkDL, zkSMART 2026/111, zkComposer 2026) uses sumcheck/GKR for linear layers and lookups for non-linearities [S].
  - Takeaway: data-parallel linear maps are cheapest in GKR or sumcheck. Committed work can shrink to inputs, outputs and remainders. That supports the multi-STARK direction for scaling beyond 4096 cells.
- **Browser and mobile proving.**
  - FibRace (arXiv 2510.14693, KKRT Labs and Hyli, 2025): 2.2M Cairo-M (Stwo) proofs on 1,420 phone models. Most proved in under 5 s, and devices need at least 3 GB RAM [V PDF]. These were native mobile apps, not wasm.
  - wasm-bindgen-rayon: SharedArrayBuffer plus Web Workers. It needs COOP/COEP cross-origin isolation, nightly `-Z build-std` and the atomics + bulk-memory target features. Its demo showed about a 3× speedup (273 → 87 ms) [S].
  - Miden ships `+simd128` and a rayon multi-threaded web build, with a claimed 3–5× speedup [S, unverified].
  - WebGPU reached Baseline in Jan 2026 per a secondary source [S]. zkSecurity's Stwo WebGPU work reported 5× on constraint evaluation and 2× end-to-end, but measured natively, not in a browser [S].
  - No published browser benchmark of a Plonky3/KoalaBear prover with SIMD and threads was found.
  - Expected levers for umis' 682 ms single-thread wasm [U]:
    1. a 4-lane `i32x4` Montgomery `PackedMontyField31` for wasm, using `i64x2_extmul_{low,high}_u32x4` as Stwo's `mul_wasm` does, plausibly about 2–3× on DFT and Poseidon2;
    2. rayon via Web Workers, about 3–6× on 8 cores;
    3. WebGPU NTT and Merkle hashing only for much larger traces. At 2^15 LDE the dispatch and shader-compile overheads (over 200 ms cold) dominate.

---

## 5. Recommendations for umis (ordered)

1. **Fix the security claim now.** Replace "~116 conjectured bits" with roughly "≈102 bits conjectured (random-words, Plonky3 p3-security model), ≈66 bits proven (Johnson)". Then:
   - choose a target: 100 proven bits on quartic, or 128 on quintic;
   - add `batch_proof_of_work_bits` and `commit_proof_of_work_bits` of about 8–10, which costs nothing in proof size.
2. **Prototype lb2 + Q≈110 + batch/commit PoW 10** and measure proof size and prove time. This also unlocks degree-5 constraints for nonlinear physics.
3. **Plan the 0.9 migration** once 0.9.0 is stable. Transcripts break and proofs are not cross-version, so put a proof-system version byte in the beacon record. After migrating:
   - adopt `p3-security`'s report APIs in CI to pin the claimed bits;
   - re-evaluate periodic binding (batch-stark still does not do it);
   - pick up the LogUp overflow fix.
4. **Aggregation.** Prototype `p3-recursion` 2-to-1 aggregation of water3d batch proofs, both FRI and WHIR inner layers. Measure per-level time natively and in wasm. Note it is unaudited.
5. **Browser.** Implement and upstream the wasm `simd128` packing for MontyField31, then enable wasm threads behind cross-origin isolation.
6. **Private tasks.** Hiding FRI in batch-stark is statistically ZK with doubled trace degree. For stronger guarantees, track HVZK-WHIR (2026/391) in multi-stark. Use a CSPRNG for masks (#2260).
7. **Do not** rely on capacity or "up-to-capacity" conjectures in any parameter set. Track soundcalc and DKT26/BCHKS updates for cheaper proven parameters.

---

## 6. Citations

1. Ben-Sasson, Carmon, Ishai, Kopparty, Saraf. *Proximity Gaps for Reed–Solomon Codes.* FOCS 2020 / J.ACM 2023. https://eprint.iacr.org/2020/654
2. StarkWare. *ethSTARK Documentation.* 2021. https://eprint.iacr.org/2021/582
3. Haböck. *A summary on the FRI low degree test.* 2022. https://eprint.iacr.org/2022/1216
4. Diamond, Gruen. *On the Distribution of the Distances of Random Words.* 2025. https://eprint.iacr.org/2025/2010
5. Crites, Stewart. *On Reed–Solomon Proximity Gaps Conjectures.* 2025. https://eprint.iacr.org/2025/2046
6. Ben-Sasson, Carmon, Haböck, Kopparty, Saraf. *On Proximity Gaps for Reed–Solomon Codes.* 2025. https://eprint.iacr.org/2025/2055
7. Haböck. *A note on mutual correlated agreement for Reed–Solomon codes.* 2025. https://eprint.iacr.org/2025/2110
8. Krachun, Kazanin, Haböck. *Failure of proximity gaps close to capacity.* 2026. https://eprint.iacr.org/2026/782
9. Kambiré. *Proximity Gaps Conjecture Fails Near Capacity over Prime Fields.* 2026. https://arxiv.org/abs/2604.09724
10. Dao, Kominers, Thaler. *Reed–Solomon Codes Beyond Johnson: Efficient Decoding and Smaller Cryptographic Proofs.* 2026. https://eprint.iacr.org/2026/2056
11. Dao, Kominers, Thaler, Zheng. *Counterexamples to Beyond-Johnson Proximity Gaps over Binary Fields.* 2026. https://eprint.iacr.org/2026/2424
12. Kominers, Thaler, Zheng. *The One-and-a-Half Johnson Bound Is Tight for Proximity Gaps of General Linear Codes.* 2026. https://eprint.iacr.org/2026/1894
13. Brakensiek, Chen, Putterman, Zhang, Zheng. *Algorithmic List Decoding of Reed–Solomon Codes up to Capacity.* 2026. https://arxiv.org/abs/2609.08005
14. Jeronimo. *Algorithmic List Decoding at Capacity and Optimal Proximity Gaps for Reed–Solomon Codes.* 2026. https://arxiv.org/abs/2609.05870
15. Harsha, Kumar, Saptharishi. *Reed–Solomon Codes at Capacity: Algorithmic List-Decoding and Proximity Gaps* (exposition). 2026. https://arxiv.org/abs/2610.08610
16. Chai, Fan. *Action–Orbit FRI Soundness Above the Johnson Radius* (relies on conjecture Q2; not peer-reviewed). 2026. https://eprint.iacr.org/2026/861
17. Dao, Miller, Wright, Grubbs. *Weak Fiat–Shamir Attacks on Modern Proof Systems.* IEEE S&P 2023. https://eprint.iacr.org/2023/691
18. Khovratovich, Rothblum, Soukhanov. *How to Prove False Statements: Practical Attacks on Fiat–Shamir.* 2025. https://eprint.iacr.org/2025/118
19. Arnon, Chiesa, Fenzi, Yogev. *STIR: Reed–Solomon Proximity Testing with Fewer Queries.* CRYPTO 2024. https://eprint.iacr.org/2024/390
20. Arnon, Chiesa, Fenzi, Yogev. *WHIR: Reed–Solomon Proximity Testing with Super-Fast Verification.* EUROCRYPT 2025. https://eprint.iacr.org/2024/1586
21. Chiesa, Fenzi, Weissenberg. *Zero-Knowledge IOPPs for Constrained Interleaved Codes* (HVZK-WHIR). 2026. https://eprint.iacr.org/2026/391
22. Haböck, Levit, Papini. *Circle STARKs.* 2024. https://eprint.iacr.org/2024/278
23. Papini, Haböck. *Improving logarithmic derivative lookups using GKR.* 2023. https://eprint.iacr.org/2023/1284
24. Haböck. *Multivariate lookups based on logarithmic derivatives* (LogUp). 2022. https://eprint.iacr.org/2022/1530
25. Diamond, Posen. *Succinct Arguments over Towers of Binary Fields* (Binius). 2023. https://eprint.iacr.org/2023/1784
26. Kothapalli, Setty, Tzialla. *Nova.* CRYPTO 2022. https://eprint.iacr.org/2021/370
27. Kothapalli, Setty. *HyperNova.* CRYPTO 2024. https://eprint.iacr.org/2023/573
28. Bünz, Chen. *ProtoStar.* 2023. https://eprint.iacr.org/2023/620
29. Boneh, Chen. *LatticeFold.* 2024. https://eprint.iacr.org/2024/257
30. Boneh, Chen. *LatticeFold+.* CRYPTO 2025. https://eprint.iacr.org/2025/247
31. Nguyen, Setty. *Neo* (2025/294, superseded by *Neo and SuperNeo*). 2026. https://eprint.iacr.org/2026/242
32. Bünz, Mishra, Nguyen, Wang. *Arc: Accumulation for Reed–Solomon Codes.* CRYPTO 2025. https://eprint.iacr.org/2024/1731
33. Setty, Thaler, Wahby. *Customizable constraint systems (CCS/SuperSpartan).* 2023. https://eprint.iacr.org/2023/552
34. Arun, Setty, Thaler. *Jolt.* 2023. https://eprint.iacr.org/2023/1217
35. Liu, Xie, Zhang. *zkCNN.* CCS 2021. https://eprint.iacr.org/2021/673 [number from memory, U]
36. Malatrait, Sirac. *FibRace: a large-scale benchmark of client-side proving on mobile devices.* 2025. https://arxiv.org/abs/2510.14693
37. Kadianakis (EF). *Shipping an L1 zkEVM #2: The Security Foundations.* 2025-12-18. https://blog.ethereum.org/2025/12/18/zkevm-security-foundations
38. EF. *Better codes challenge.* 2026-08-20. https://blog.ethereum.org/2026/08/20/better-codes-challenge
39. Ethereum Foundation. *soundcalc.* https://github.com/ethereum/soundcalc (reports/summary.md, reports/sp1.md)
40. Mohnblatt (zkSecurity). *Proximity Gaps: What Happened and How Does It Affect our SNARKs.* 2025-11-14. https://blog.zksecurity.xyz/posts/proximity-conjecture/
41. Plonky3 repo, releases and advisories. https://github.com/Plonky3/Plonky3 , https://github.com/Plonky3/Plonky3/security/advisories ; PRs #2033, #2048, #2100, #2106, #2199, #2379, #2381
42. Plonky3-recursion. https://github.com/Plonky3/Plonky3-recursion
43. Stwo. https://github.com/starkware-libs/stwo
44. Succinct. *SP1 Hypercube.* https://blog.succinct.xyz/sp1-hypercube/ ; Axiom. *OpenVM 2.0.* https://axiom.xyz/blog/openvm-2
45. wasm-bindgen-rayon. https://github.com/RReverser/wasm-bindgen-rayon

## 7. Files

PDFs saved (with `.txt` extractions alongside) in `scratchpad/papers/`:
- `arxiv-2604.09724.pdf` (Kambiré, prime-field counterexample)
- `arxiv-2609.05870.pdf` (Jeronimo)
- `arxiv-2609.08005.pdf` (BCPZZ)
- `arxiv-2510.14693.pdf` (FibRace)

**ePrint PDFs could not be downloaded.** eprint.iacr.org returned a Cloudflare bot challenge, and I did not try to get around it. ePrint claims above rest on abstract pages (fetched as HTML), Plonky3 source and docs, and the cited secondary sources. Re-download manually if full texts are needed: 2020/654, 2021/582, 2025/2010, 2025/2046, 2025/2055, 2026/782, 2026/2056, 2023/691, 2024/390, 2024/1586, 2024/278, 2023/1284.

Other artifacts:
- `scratchpad/scripts/sec.py` and `sec2.py` — my re-implementation of the `p3-security` FRI formulas. They are pure Python with no downloaded code executed.
- `scratchpad/src/` — shallow clones of Plonky3, Plonky3-recursion, stwo and soundcalc. These were read only, never built or run.
