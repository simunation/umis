# Fiat–Shamir binding and logarithmic-derivative lookups in umis

Scope: how umis' water3d transition proof (Plonky3 0.8 `p3-batch-stark`, KoalaBear, EF = degree-4
extension) binds its statement into Fiat–Shamir (FS), and how its LogUp buses must be built to stay
sound. It covers the four papers in this cluster, the 0.8.0 sources, upstream main (read only,
under `tmp/zk-research/src/p3`), and the current code:

- `crates/task-water3d/src/air.rs`: the AIR, `claim_data`, `verify_inner` and its tests.
- `crates/umis-stark/src/toy.rs` and `crates/umis-stark/tests/toy.rs`: the toy AIR and its tests.
- `crates/umis-stark/src/config.rs`: the FRI presets and the challenger.

Conventions:
- `p = 2^31 − 2^24 + 1` (log2 p ≈ 30.99), so `|EF| = p^4 ≈ 2^123.95`.
- `0.8:` stands for `~/.cargo/registry/src/*/`, the 0.8.0 crate sources.
- `main:` stands for `tmp/zk-research/src/p3/`, upstream git main `0d803737b`.
- Paper pages are **PDF page numbers**, counted by form feeds in the `.txt`. Printed page numbers can differ by about one.

Status (2026-11-03):
- **No exploitable FS or LogUp gap in the current water3d proof.** This matches REPORT §3.3 and plonky3-internals §1–2.
- There are four hygiene gaps (§2.3).
- The tamper tests have one real weakness: in the default debug test profile they never reach the verifier (§5.2).

---

## 1. Papers

### 1.1 Dao, Miller, Wright, Grubbs: *Weak Fiat–Shamir Attacks on Modern Proof Systems* (ePrint 2023/691, IEEE S&P 2023)

Local copies: `tmp/zk-research/papers/eprint-2023-691.{pdf,txt}`.

**Key results**
- **Definition 3 (p5)** defines the two FS variants:
  - *strong FS*: `c_i = H(pp, x, a_1..a_i)`;
  - *weak FS*: `c_i = H(a_1..a_i)`, which omits the public parameters `pp` and the statement `x`.
- **Survey (Table I, p3).** 75+ repositories examined; 36 weak-FS implementations across 12 proof systems.
- **Attack template (§III, p6).** Four steps:
  1. Find the part of the statement that is not hashed.
  2. Find the verifier check that uses it.
  3. Build a proof from arbitrary witness polynomials, which fixes all the challenges.
  4. Solve for the unhashed public value that makes the final check pass.
- **Bulletproofs (§IV, Theorem 4, p8).** Adaptive knowledge soundness breaks under weak FS (assuming DL-REL).
- **Plonk (§V, Theorem 6, p11).** The prover commits arbitrary `a, b, c, z, t`, derives `β, γ, α, z` from the proof alone, then sets the public input `PI` so that `Σ PI_i·L_i(z)` equals the rest of the identity at `z` (step 8, eq. (5)). One public input is enough.
- **Spartan (§VI, Theorem 7, p13).** The same pattern, through the R1CS public input.
- **Wesolowski VDF (§VII, Theorem 8, p14).** An unhashed delay parameter `T` lets the prover claim `T ≫ t`.

**Other misuse classes (§VIII, p15)**
1. A prover message that is not hashed.
2. A fresh transcript started for a sub-protocol. This is a special case of (1).
3. Public parameters that are not hashed, such as the circuit or R1CS matrices. This matters whenever the circuit can be chosen by the prover.

**Mitigations (§IX, p16)**
- Specify exactly what is hashed.
- Declare every FS input and challenge when the transcript is initialised, so that drawing a challenge before every input is absorbed is an error.
- Plonky3's `p3_challenger::fs` pattern/`DomainSeparator` machinery is an implementation of this idea (`0.8: p3-challenger-0.8.0/src/fs/domain_separator.rs:131-158`).

**What this means for a STARK with periodic columns or public values**

The Plonk attack carries over directly. If `X_out` (periodic column `x_out`) were neither absorbed nor bound by a public value, a prover could forge as follows:
1. Commit any trace, then derive `α, β`, the folding challenge and `ζ`.
2. Pick the 4096×4 values of `x_out`. Their periodic interpolant at `ζ` must make `last·(qn − x_out)(ζ)` close the quotient identity: one linear equation in about 16k unknowns.
3. The verifier, holding that `X_out`, accepts.

The same holds for `x_in`, the halos and `tide`. This is the reason the keccak digest in umis' public values exists (§2).

### 1.2 Khovratovich, Rothblum, Soukhanov: *How to Prove False Statements: Practical Attacks on Fiat–Shamir* (ePrint 2025/118, v. 2025-12-11)

Local copies: `tmp/zk-research/papers/eprint-2025-118.{pdf,txt}`.

**Protocol attacked (§1.1.1, p3–4; §2.1, p7).** `Π_{comm,d}` is GKR combined with a multilinear PCS for a depth-d circuit C:
- the prover sends `x`, `y` and `α = comm(w)`;
- `r = h(⟨C⟩, x, y, α)`;
- GKR reduces the claim on `ŷ(r)` to claims on `x̂` and `ŵ`.

**Theorem 1 (Basic Attack, p4)**
- If `comm` has depth `d_comm` and `h` has depth `d_h`, then for every `d ≥ d_comm + d_h + O(1)`, `FS_h(Π_{comm,d})` is **not adaptively sound** (Definition 6, p8–9).
- **Construction 1 (p9–10):**
  1. `C*(w)` reads `w` as a digest `ψ`.
  2. It computes `α = comm(w)` and `γ = h(ψ, y*, α)`.
  3. It outputs `(γ, γ−1)`, which can never equal `y* = (0,0)`.
- With `w = ⟨C*⟩`, `ŷ(r) = γ − r = 0 = ŷ*(r)`. From there on the honest GKR prover is accepted with probability 1.

**Theorem 2 (Extended Attack, p5)**
- For **any** admissible circuit C (one whose witness is at least as large as `⟨C⟩`), there is a functionally equivalent `C*` of depth `d + d_comm + O(d_h·log ℓ)` whose FS proof accepts any chosen output `y*`.
- So soundness depends on how the circuit is *implemented*, not only on what it computes.

**Universal-circuit variants (§4, Theorems 9–10, p13–15).** These break non-adaptive soundness, either with very deep circuits or under extra assumptions.

**When it applies.** All three preconditions must hold:
1. The circuit is chosen by the prover, or at least is not canonical.
2. Intermediate values are not committed. GKR's selling point is exactly that, and §5 (p15) names it as the enabling property.
3. The circuit's depth budget is enough to evaluate the FS hash and the commitment.

**When it does not apply**
- **Remark 3 (p6):** GKR used for lookups or grand products (Papini–Haböck, among others) has a *canonical, extremely simple* circuit, and the attack "does not seem to be applicable".
- Constraint-system STARKs commit the whole trace, which removes precondition 2.

**Mitigations (§5, p15–16)**
- Make the circuit family too weak to compute `h`: bound its depth or degree, use a deeper FS hash, or commit intermediate layers.
- Arnon–Yogev [AY25] propose a larger hash with proof of work.
- Both remain unproven.

### 1.3 Haböck: *Multivariate lookups based on logarithmic derivatives* (LogUp; ePrint 2022/1530, v. 2023-03-31)

Local copies: `tmp/zk-research/papers/eprint-2022-1530.{pdf,txt}`.

**Lemma 3 (p5).** For sequences of length `n` over a field with characteristic `p > n`:
- `Π(X + a_i) = Π(X + b_i)` holds iff `Σ 1/(X + a_i) = Σ 1/(X + b_i)`.
- Logarithmic derivatives turn products into sums.

**Lemma 4 (p6).** Fractional representations `Σ m(z)/(X − z)` are unique.

**Lemma 5 (Set inclusion, p6)**
- Take characteristic `p > N`. Then `{a_i} ⊆ {b_i}` iff there exist field elements `m_i` with `Σ_{i≤N} 1/(X + a_i) = Σ_{i≤N} m_i/(X + b_i)`.
- The `m_i` are **arbitrary field elements**: the table-side multiplicities need no constraint.
- The condition `N < p` is what makes the multiplicity of every queried value non-zero mod p.

**Protocol 2 (p10)**
- Batch-column lookup of M columns over `H = {±1}^n`, which **requires `char F > M·2^n`**.
- The verifier samples `x ← F \ {−t(H)}`.
- **Remark 3 (p11)** allows `x ← F` instead, at a completeness error of about `|H|/|F|`. A vanishing denominator then leaves the helper `h_0` unconstrained. This is the "vanishing denominator" case that p3 counts as a soundness term.

**Soundness (§3.2, eq. (22), p11):**
`ε_1 ≤ ((M+1)·|H| − 1) / (|F| − |H|)`

**Theorem 4 (p12):**
`ε < ((M+1)·|H| − 1)/(|F| − |H|) + (K+1)/|F| + ε_sumcheck`
- `K = ⌈(M+1)/ℓ⌉` is the number of helper columns, for partial-sum size `ℓ`.
- `ε_sumcheck` comes from **Theorem 2 (p8, [LFKN92])**: `ε_sumcheck ≤ (1/|F|)·Σ_i deg_{X_i} p`. It is taken over `M+4` variables with individual degree `M+3`.

**Cost (§3.3, p12–13; Table 2, p13)**
- The prover commits `m` plus the K helper columns `h_k`.
- Grouping `ℓ` fractions per helper trades committed columns for constraint degree `ℓ + 1`.
- This is exactly p3's `pack_same_bus_with_degree`.

**Vector-valued lookups (§3.4, p14)**
- Tuples are compressed with independent challenges `y_1..y_k`. Soundness is unchanged, because the polynomials `X − Σ c·Y^…` are irreducible.
- p3 uses powers of a single β instead, which multiplies the fingerprint degree by the width W (§3.1 below).

**Concrete bound used for umis.** p3-security's form (`0.8: p3-security-0.8.0/src/logup.rs:1-45`) is

`ε_logup ≤ N·(W+1)/|EF|  (numerator root)  +  N/|EF|  (vanishing denominator)  =  N·(W+2)/|EF|`

- `N` is the number of interactions × trace height, summed over all buses.
- `W` is the widest payload.

For water3d:
- `N = 23·2^14` and `W = 5`, so `ε ≈ 2^-102.6`. With no lookup PoW, that is about **102.6 bits**.
- At 2^16 rows it drops to about 100.6 bits.
- In Haböck's single-column form (`M = 19` weighted queries plus tables, `|H| = 2^14`) the first term alone is about 2^-105.5. The β-power compression costs the remaining ≈3 bits.

### 1.4 Papini, Haböck: *Improving logarithmic derivative lookups using GKR* (LogUp-GKR; ePrint 2023/1284, v. 2023-09-18)

Local copies: `tmp/zk-research/papers/eprint-2023-1284.{pdf,txt}`.

**Circuit (§3.1, p4).** A binary tree of projective fraction additions:
- `(p, q) ⊕ (p', q') = (p·q' + p'·q, q·q')`;
- layer `k` holds multilinear polynomials `p_k` and `q_k`.

**GKR (§3.2, p4–6)**
- Per layer:
  - a two-point to one-point reduction, with a random `μ_k` and `r_{k+1} = (ρ_k, 1 − μ_k)`;
  - the batching `p_k + λ_k·q_k`;
  - a degree-3 sumcheck.
- The verifier finally checks:
  - `p_1(+1)·q_1(−1) + p_1(−1)·q_1(+1) = 0`, that is, the sum is zero;
  - **`q_1(+1)·q_1(−1) ≠ 0`**, that is, the root denominators are non-zero.

**Proposition 1 (p6):**
`ε_GKR ≤ (2(n−1)+1)/|F| + Σ_{k=1}^{n−1} ε_sumcheck(|H_k|) ≤ n·(3n+1) / (2|F|)`
- It uses `ε_sumcheck(|H_k|) ≤ 3k/|F|`, since the sumcheck is degree 3.

**Proposition 2 (logUp-GKR soundness, p7–8)**
- For M witness columns and one table column over `H_n`, with `char F > M·2^n`:
  `ε ≤ ((M+1)·2^n − 1)/(|F| − |H|) + ε_GKR(|H_{n+m}|)`, where `m = ⌈log2(M+1)⌉`.
- The first, fingerprint term still dominates. The paper notes the batching adds "as good as no change" (p7).

**Cost**
- GKR fractional sumcheck: `|H_n|·(43·M + 29·A)` field operations (eq. (5), p6).
- Applied to logUp: `|H_{n+m}|·(43M + 29A)`, against `|H_{n+m}|·(19M + 16A)` **plus committing M+1 helper columns** for plain logUp (eqs. (6) and (7), p8).
- **Only the multiplicity column is committed.**
- Estimated gain: about 10× for elliptic-curve PCS, and **about 2–10× for Reed–Solomon/hash-based PCS** (p8).

**Univariate case (§5, Proposition 3, p10)**
- An IOP for multilinear evaluation from a univariate PCS has error `≤ 2·d_max/|F| + ε_sumcheck`.
- Univariate logUp-GKR has error `ε ≤ ε_MVlogUp + ε_trafo` (p11–12).
- The paper defers formal proofs to "another document".

---

## 2. Transcript binding rules for umis

### 2.1 What p3-batch-stark 0.8 binds (in order)

| # | Step | 0.8 code |
|---|---|---|
| 0 | Seed: `VERSION=1` and `NAME="p3-batch-stark"`; the pattern hash, which fixes the instance count, every public-value count, preprocessed/lookup/zk presence and the PoW bit counts; and an instance label with the instance count, main widths and preprocessed widths | `p3-batch-stark-0.8.0/src/transcript.rs:80-83, 249-400, 408-432`; `p3-challenger-0.8.0/src/fs/domain_separator.rs:131-158` |
| 1 | `degree_bits` per instance, absorbed as an EF element | `transcript.rs:587-594`; prover `prover.rs:253`; verifier `verifier/mod.rs:648` |
| 2 | main commitment | `transcript.rs:606`; `prover.rs:271`; `verifier/mod.rs:650` |
| 3 | public values (`Kind::Public`, `Length::Fixed(count)`; never on the wire) | `transcript.rs:280-289, 608-611`; `p3-challenger-0.8.0/src/fs/state/prover.rs:182-205` |
| 4 | preprocessed commitment, taken from the verifier's own `CommonData` | `transcript.rs:615-620`; `prover.rs:272`; `verifier/mod.rs:651` |
| 5 | lookup PoW (0 bits), then **α, β ∈ EF** | `transcript.rs:628-659` (PoW `:644`) |
| 6 | permutation commitment, then every LogUp terminal, then the constraint-folding challenge | `transcript.rs:670-685` |
| 7 | quotient commitment (plus the randomization commitment if zk) | `transcript.rs:688-695` |
| 8 | OOD PoW (0 bits), then ζ | `transcript.rs:703-712` |
| 9 | FRI PCS sub-protocol on the same sponge | `transcript.rs:721-727`; `p3-fri-0.8.0/src/pcs_transcript.rs:65-80, 261-273`; `p3-fri-0.8.0/src/transcript.rs:68-92, 255-283` |

Within step 9:
- The FRI seed binds `log_blowup`, `max_log_arity` and the per-round arities (`p3-fri transcript.rs:265-280`).
- `num_queries`, the final-poly length and every PoW bit count are fixed `Length`s in its pattern (`:208-245`).
- The final polynomial is absorbed. This was Least Authority Issue C, now resolved.

Verifier-side checks that complement the transcript:
- The preprocessed `degree_bits` must equal the setup's (`verifier/mod.rs:285`).
- Instance, public-value and terminal counts are checked (`:391-399, 482-491, 605-616`).
- The multiplicity bound is checked (`:477-478`).
- The terminal sum must be zero (`:843`).

**Not absorbed in 0.8, and still not in batch-stark on main:**
- periodic column values (the verifier evaluates its own: `verifier/mod.rs:798-802`);
- the AIR's constraints and identity (the verifier re-derives its own symbolic AIR);
- the number of periodic columns;
- the hash, MMCS and field choice;
- any application domain separator (`config.rs` builds `Challenger::new(perm)` without observing anything).

### 2.2 Checklist: everything that must be bound for a umis transition statement

"Bound" means bound into FS before the first challenge (α, β), either directly or through a collision-resistant digest that sits in the public values.

| Item | Bound in p3 0.8 FS? | Bound by umis? | Where |
|---|---|---|---|
| Protocol id/version of the proof system | yes (`VERSION`, `NAME`; FRI has its own) | n/a | `transcript.rs:80-83` |
| **umis protocol/AIR version** | no | **no** (tag `"water3d-air"` carries no version) | `air.rs:338` (gap G3) |
| `params_hash` (zone shape, point, `n_loc`, `k_sub`, `shift`, physics ratios) | no | **yes**, digest bytes 0–31; also `stmt.params_hash == z.params_hash` | `air.rs:329, 562` |
| genesis seed / coefficients (`wv`, `wc`, `fv`, `fc`, `has`, `mult`) | **yes**, via the preprocessed commitment, which the verifier rebuilds locally | indirectly: `setup()` rebuilds from params+seed | `air.rs:456-480, 591`; `transcript.rs:615-620` |
| preprocessed commitment (the VK) | **yes** (step 4) | built locally, never read from the proof | `verifier/mod.rs:651` |
| `slot` | no | **yes**, digest (`slot.to_be_bytes()`) and `tide(slot·slot_ticks)` | `air.rs:330, 339` |
| `tide` | yes, as public value 0 | computed by the verifier | `air.rs:339-340` |
| `X_in` | **no** (periodic) | **yes**, digest | `air.rs:331-337` |
| `X_out` | **no** (periodic) | **yes**, digest | same |
| halos (6 faces) | **no** (periodic) | **yes**, digest | same |
| AIR identity (constraints, buses) | no | no; the verifier evaluates its own `WaterAir::eval` | fixed by code |
| AIR shape: widths 46/32, public count 17, lookups present | yes (label and pattern) | n/a | `transcript.rs:408-432, 280-289` |
| periodic column count (32) | no | fixed by code | n/a |
| instance count (1) | yes (pattern and label) | `verify_batch` gets a one-element slice | `verifier/mod.rs:391` |
| trace height | yes (`degree_bits` absorbed; also pinned to the preprocessed meta) | `Shape::of` | `verifier/mod.rs:285` |
| FRI parameters | yes (FRI seed and pattern) | the verifier uses its own `FriPreset`, chosen by `genesis.stark_config_id` | `air.rs:442-450` (gap G2) |
| lookup / OOD PoW bits | yes (pattern) | 0 / 0 | `transcript.rs:305, 372` |
| state roots, `inputs_commitment`, `outputs_root` | no | checked natively against the data, which then enters the digest | `air.rs:565-585` |
| `stmt.zone` | no | **not checked** in `verify_inner`; the caller must pass that zone's params | gap G4 |

**Digest encoding**
- `keccak256("water3d-air" ‖ params_hash(32) ‖ slot(8, BE) ‖ X_in ‖ X_out ‖ halo_0..5)`, with every value as `i32` LE (`air.rs:328-338`).
- Every segment has a fixed length once `params_hash` is fixed, so the encoding is unambiguous.
- It is split into 16 × u16 limbs, each less than p, so the map from digest to public values is injective (`air.rs:341-343`).

**Rule.** The digest **must stay** as long as umis uses batch-stark, including on main.

Upstream binds `BaseAir::periodic_statement` only in p3-multi-stark:
- `main: multi-stark/src/transcript.rs:32, 385-389`;
- `main: air/src/air.rs:103-115`.

### 2.3 Gaps (none exploitable today)

**G1. No application domain separator in the challenger** (`config.rs:108-115`).
- A cross-application replay would need the same AIR and the same preprocessed commitment, so this is not exploitable.
- Defence in depth: observe `b"umis/water3d/v{N}" ‖ stark_config_id` right after `Challenger::new`. This is LA Suggestion 5.
- It breaks proof compatibility, so do it together with a version bump.

**G2. The FRI preset is a genesis choice, not part of the statement.**
- The verifier enforces its own config, so a proof made with fewer queries is rejected: the pattern lengths differ.
- But `koalabear-test` (4 queries, no PoW, about 4 bits) is accepted by `from_config_id` and used by SimNet (`umis-simnet/src/lib.rs:120`).
- Rule: refuse it in any non-dev genesis.

**G3. No AIR version in the digest tag.**
- Not a soundness issue, since the verifier evaluates its own AIR.
- But stored proofs become ambiguous across AIR upgrades.
- Use `"water3d-air/v1"` and bump it whenever constraints, buses or the encoding change.

**G4. `stmt.zone` is not tied to `params` in `verify_inner`.**
- `params` includes `point`, so different zones have different `params_hash` values.
- The binding of zone id to params is the caller's job (beacon, `apply_proof_commitment`). Document it, or check it in `verify_inner`.

Related hygiene, not a binding issue:
- **Halo length leniency.** `halos()` zero-fills a face of the wrong length (`air.rs:294-298`), while `decode_inputs` rejects it. It should reject.
- **Merkle-cap malleability.** In 0.8.0 the cap is malleable (fixed upstream in #2378), so proof bytes are not canonical. Never use a proof hash as a unique id.

---

## 3. LogUp rules for umis

### 3.1 Requirements, checked against `air.rs`

**(R1) Each bus address must be provided by exactly one table entry, or the keys must be unique by construction.**
- Memory-bus soundness rests on Lemma 5 plus "one value per address".
- `air.rs:262` provides `(addr + C, qn)`, with `addr + C ∈ [C, k_sub·C)` and `MULT ≠ 0` only when `t + 1 < k_sub` (`:191`).
- `air.rs:263` provides `(addr, q)` with `IN_MULT ≠ 0` only when `t = 0`, so `addr ∈ [0, C)` (`:192`).
- The two ranges are disjoint, so a read `(a, v′)` with `v′ ≠ written` has coefficient `−#reads ≠ 0`. ✓
- Addresses stay below 2^14 + C, far below p, so they cannot wrap. ✓

**(R2) The query side must satisfy `Σ_i weight_i·height_i < p`, and every weight must be honest.**
- p3 checks this on both sides: `0.8: p3-lookup/src/types.rs:474-502`, `prover.rs:159`, `verifier/mod.rs:477-478`.
- The weights are mem 7 (`active` plus `has_0..5`), rem 4 and byte 8, for a total of 19.
- `19·2^14 = 311,296 < p`. ✓
- The current pattern stays within the bound up to 2^26 rows.
- The bound is the `N < p` / `char F > M·2^n` hypothesis of Lemma 5 and Protocol 2.

**(R3) Query multiplicities must be constrained to the declared bound.**
- `Count::bounded(e, w)` only *declares* `|e| ≤ w` (`p3-lookup/src/count.rs:40-55`). The AIR must enforce it.
- If it does not, a read with an unconstrained count `c = −1` turns into a **write**: it supplies an arbitrary value to the table.
- water3d uses `Count::bounded(active, 1)` (`air.rs:264, 275`) and `Count::bounded(has_d, 1)` (`:269`). These are preprocessed booleans, so the bound holds by construction. ✓
- The byte lookups use the constant `1` (`:280-281`), whose weight is set automatically. ✓
- If a gate ever moves to the **main** trace, add `g·(1 − g) = 0`. For mutually exclusive flags (`lookup_key_exclusive`, `bus.rs:104-122`), also add `Σ g ≤ 1`.

**(R4) Table-side multiplicities may be prover-chosen where Lemma 5 allows it: range tables and function tables.**
- `M_REM` and `M_BYTE` are main columns with no constraint (`air.rs:277, 283`). ✓
- Repeated table values (`r mod 2^s`, `r mod 256`) are fine. A wrong multiplicity simply leaves the sum non-zero.
- **Memory-bus** table multiplicities may be prover-chosen only if (R1) holds *regardless of their value*. For example, a prover-set `IN_MULT` on a row with t ≥ 1 would create a second provider for address `addr ≥ C` and break the memory.
- water3d keeps `MULT` and `IN_MULT` preprocessed (`air.rs:191-192`). ✓ This is stronger than the toy, which uses a main `MULT` (`toy.rs:285`); that is still sound because its keys are unique per row.

**(R5) Gating.**
- The `rem` lookups are gated by `active` (`air.rs:275`). This is not needed for soundness, since a padding `qn` is never read (`MULT = IN_MULT = 0` there).
- The byte lookups are ungated, and padding rows supply `lo = 0`, `hi = 128` (`air.rs:405-413`).
- The memory reads are gated by `active` and `has_d`. Without gating, reads at `addr + off_d` on boundary cells would hit addresses in other substeps or out of range.
- Rule: gate any lookup whose padding or boundary value is not a valid table member. Never gate with a selector inside a lookup count; main #2574 forbids it.

**(R6) Payload widths must be uniform per bus.**
- `[x]` would collide with `[0, x]` (`0.8: p3-lookup/src/types.rs:96-115`, asserted).
- mem uses 5 (`air.rs:257-259`), rem 1 and byte 1. ✓
- Buses are separated by `prefix[bus] = α + (bus+1)·β^W` with `W = max width = 5` (`0.8: p3-lookup/src/challenges.rs:70-91`). This is injective by construction.

**(R7) The challenges must come from EF and be drawn after every trace that feeds a fraction is committed.**
- α and β are EF4 (about 124 bits). They are drawn after the main commitment, the public values and the preprocessed commitment (`transcript.rs:628-659`; `prover.rs:271-282`). ✓
- One pair serves the whole batch, which global buses require.
- The permutation commitment and the terminals are absorbed before the folding challenge (`:670-685`). ✓

**(R8) Bus messages must not read periodic columns in 0.8.**
- `LookupTraceBuilder` has an empty periodic slice.
- water3d messages use only `main` and `preprocessed` (`air.rs:257-283`). ✓

**(R9) Range-check arithmetic must not wrap.**
- `Σ w·val + tide·F = 2^s·qn + rem` holds over the integers only while both sides stay below p.
- With `|qn| ≤ 2^15` (bytes) and `rem < 2^s` that needs `s ≤ 14`. `validate_params` still accepts `8..=16` (`crates/task-water3d/src/lib.rs:208`).
- **Cap it at 14** (REPORT finding #2).
- Also check natively that `|x_in|, |halo| ≤ 2^15` (§3.3 of REPORT).

### 3.2 Design guide for new buses

**A. A Poseidon2 (or any function) table over a bus, as a second AIR in the same batch**

1. **Single proof, single transcript.**
   - Put the hash AIR in the same `prove_batch` call, as another `StarkInstance`.
   - Cross-AIR buses are sound only because *one* main commitment covers every instance before α and β are drawn (`transcript.rs:271-327`), and the verifier sums every terminal (`verifier/mod.rs:843`).
   - Never sum terminals across separate proofs: their α and β differ.
2. **Buses are global by name** (`0.8: p3-lookup/src/types.rs:190-201`). The name is the interface, so give each bus a unique, versioned name (`w3d_p2_v1`).
3. **Message = (tag, input…, output…)** as one tuple.
   - Send input and output together, never in separate messages, or they are not bound to each other.
   - Pad every interaction on the bus to the same width (R6).
4. **Table side.** The Poseidon2 AIR constrains `out = P(in)` on each row and provides `(in, out)` with a main multiplicity column. A prover-chosen multiplicity is fine for a function table (R4).
5. **Query side.** The water3d AIR queries with a preprocessed or boolean-constrained count (R3).
6. **Height pinning.**
   - An instance without preprocessed columns has a prover-chosen `degree_bits`. It is absorbed, and only its range is validated.
   - That changes the multiplicity sum (still checked at verify time with the actual heights) and the FRI domain.
   - Prefer giving the table a preprocessed column (row index or round constants), which pins the height through `verifier/mod.rs:285`, or check the height natively.
7. **Soundness cost.**
   - N grows by the table's rows × interactions.
   - `W` grows from 5 to about 25 (16 input + 8 output + tag): `log2(27/7) ≈ 1.9` bits lost on the fingerprint term.
   - To recover bits, set `lookup_proof_of_work_bits` to 8–16 (`transcript.rs:644`).
8. **Watch-outs**
   - p3-poseidon2-air is deprecated on main (#2559).
   - A Poseidon2 that matches the challenger's permutation creates no FS issue in a committed-trace STARK. It does matter under a GKR-proved circuit (§4).
9. **Binding a hash output to the statement.** Constrain `first`/`last` × (col − public value) through preprocessed selectors. batch-stark rejects AIRs with `public_boundary_io` (`prover.rs:141`).

**B. Merging range checks to reduce auxiliary columns**

Today:
- rem uses 5 interactions in 3 fraction columns, one of them half-empty.
- byte uses 9 interactions in 5 columns, one half-empty.
- There are 2 preprocessed tables (`T_REM`, `T_BYTE`) and 2 multiplicity columns.

Two pairs fit per column at degree 3 (`log_blowup = 1`). Options, in order of preference:

1. **One range table, the interval `[0, 2^14)`, reusing `ADDR`** (REPORT §8, item 4). `ADDR = r` takes every value in `[0, height)` exactly once.
   - Value range: write `qn + 2^15 = l + 2^14·b_0 + 2^15·b_1` with `l` looked up and `b_0`, `b_1` boolean-constrained in the AIR (degree 2). That is one lookup per value.
   - Remainder: look up both `rem` and `2^{14−s}·rem` in the same table.
     - The first lookup gives `rem ∈ [0, 2^14)` as an integer.
     - The product is then below `2^{28} < p`, so it cannot wrap, and the second lookup gives `rem < 2^s`.
   - **Both lookups are required.** With only the scaled one, `rem = t/2^{14−s}` is a valid field element for any `t`.
   - The result is 12 query interactions plus 1 table interaction = 13, so 7 columns instead of 8.
   - It drops `T_REM`, `T_BYTE`, `M_BYTE`, `lo` and `hi`. It adds `l`, `b_0` and `b_1`.
   - The query weight stays 12 per row.
   - It needs `height ≥ 2^14` and `s ≤ 14` (consistent with R9).
2. **One tagged bus for all range checks:** `(tag, v)`, with tag 0 for rem and 1 for byte. This saves only the half-empty column: 14 interactions in 7 columns. The tag must be a constant, never a prover value.
3. **All buses on one tagged bus:** `(tag, a, v0..v3)` padded to width 6, giving 23 interactions in 12 columns (one saved). Costs `log2(8/7) ≈ 0.2` bits.
4. Pinning more fractions per column needs a lookup degree budget above 3, which means `log_blowup ≥ 2` (proof size up about 2×). LogUp-GKR (§4) removes these columns entirely.

**Rules for any merge**
- Each table must be **exactly** the allowed set.
- Scaling tricks need a prior unscaled range check and no wrap.
- Recompute `Σ w_i·h_i < p` and `N·(W+2)/|EF|`.
- Re-run the tamper tests (§5).

---

## 4. Moving to LogUp-GKR / p3-multi-stark (Plonky3 main)

**Stack.** p3-multi-stark (`main: multi-stark/src/lib.rs:1-13`) is SuperSpartan-style:
- a multilinear zerocheck;
- WHIR or a binary PCS, instead of FRI;
- lookups proved by fractional GKR (`main: multi-stark/src/lookup/mod.rs:1-13`, `fractional_gkr/`);
- buses through `p3-bus` and `p3-gkr` (`main: gkr/src/lib.rs:1-19`).

It means rewriting `config.rs` and the prove/verify glue (plonky3-internals §6).

**Cost**
- **Committed LogUp columns go from 56 base columns to about 0.**
  - The fractions are no longer committed. Only the multiplicities are, and water3d already holds them in main (`M_REM`, `M_BYTE`) or preprocessed (`MULT`, `IN_MULT`).
  - This removes today's largest commit cost (REPORT §5).
- **Prover arithmetic** is about `|H_{n+m}|·(43M + 29A)` (Papini–Haböck eq. (6)).
  - For 23 interactions × 2^14 rows that is `2^19` leaves.
  - So roughly 2.3·10^7 EF multiplications, plus the leaf materialisation.
- **Proof size** gains `O(L²)` EF elements for `L = 19` layers:
  - `Σ i` rounds × 3 coefficients, plus 4 claims and 2 challenges per layer;
  - that is about 10 KB, against FRI openings of 56 auxiliary columns today.

**Soundness**
- The fingerprint term is unchanged: `N·(W+2)/|EF| ≈ 2^-102.6`. In Proposition 2 the first term still dominates.
- GKR adds `ε_GKR ≤ n′(3n′+1)/(2|EF|)` (Proposition 1) with `n′ = 19`, so `551/2^124 ≈ 2^-114.9`. That is negligible.
- p3's own model charges `5R/|F|` per radix-4 round, plus batching and collapse terms (`main: security/src/gkr.rs:1-5`; `security/src/multilinear.rs:10-13`).
- The PCS term changes from FRI to WHIR. That needs its own analysis (outside this note).

**FS risks after a move, checked against 2025/118 and 2023/691**

1. **Periodic data becomes bound natively.**
   - multi-stark seeds `periodic_statement` (`main: multi-stark/src/transcript.rs:385-389`) and forbids periodic columns inside lookups (`lookup/mod.rs:200-206`).
   - Keep the keccak digest anyway until the move is complete. It also carries `params_hash` and `slot`, which are not periodic.
2. **Ordering must be kept.**
   - multi-stark's statement transcript absorbs the preprocessed commitment, then main, then the public values, all before the bus and lookup brackets (`transcript.rs:43-52`).
   - The lookup transcript draws α and β before the fractions and θ after both openings (`lookup/transcript.rs:24-36`).
   - The fractional-GKR transcript absorbs the root denominator before any challenge, each round polynomial before its challenge, and the children before the branch coordinate (`fractional_gkr/transcript.rs:1-50`).
   - Tests must cover each of these (§5).
3. **Root denominator must be non-zero.** This is the `q_1(+1)·q_1(−1) ≠ 0` check of Papini–Haböck §3.2 (p6). Without it, a 0/0 fraction lets imbalanced buses through.
4. **Leaf claims must be authenticated.**
   - GKR ends in claims about the leaf tables at a random point, which must be re-linked to the committed columns: "The caller must also authenticate the returned leaf evaluations" (`main: gkr/src/lib.rs:17-18`).
   - In multi-stark the zerocheck rebuilds them (`lookup/mod.rs:10-12`).
5. **2025/118 applicability**
   - The GKR here is the canonical fractional-sum tree (Remark 3, p6). The prover does not choose the circuit, and the AIR trace is fully committed. **The attack does not apply.**
   - It **would** apply if umis adopted a GKR prover for the *stencil itself* (REPORT §8 research track) over a circuit family that is
     - chosen or parameterised by an untrusted party, and
     - deep enough to evaluate the FS hash. Here that hash is Poseidon2-KoalaBear-16, which is also what any in-circuit hashing table would use.
   - Stencil circuits are linear and shallow, far below Poseidon2's depth, so they are safe by the §5 depth argument.
   - Never let proven circuits include the challenger's permutation together with uncommitted intermediate layers.
   - Keep the circuit description fixed by verifier code. Today the preprocessed data comes from on-chain params and the seed; the prover never supplies it.
6. **No fresh transcripts.** Every sub-protocol must run as a bracket on the same sponge, as `delegate` does in 0.8 and as `delegation::<…>` does in multi-stark. Restarting a transcript is Dao et al.'s misuse class 2.

---

## 5. Must-not-get-wrong checklist and negative tests

### 5.1 Checklist

1. Keep the keccak digest of `params_hash ‖ slot ‖ X_in ‖ X_out ‖ halos` in the public values for as long as periodic columns are not absorbed (batch-stark on 0.8 and on main).
2. Compute the public values and periodic columns **on the verifier** from data it checked natively. Never read them from the proof or the prover.
3. Build the preprocessed commitment (`CommonData`) locally from params and the seed. Never deserialize it.
4. Use challenges only from EF4. Draw lookup challenges only after every committed trace that feeds a fraction.
5. For every query count, either use a preprocessed boolean or a constant, or constrain it in the AIR to its declared `Count::bounded` weight.
6. Keep `Σ weight·height < p`. Re-derive it whenever you add a lookup or rows.
7. Every memory or bus address must have one provider. Memory-table multiplicities stay preprocessed.
8. Use uniform payload width per bus, and constant tags only.
9. Keep `shift ≤ 14` (no wrap). Range tables must be exactly the intended intervals.
10. Only `koalabear` in production genesis. `koalabear-test` is about 4 bits.
11. Never sum terminals or share buses across separate proofs.
12. Never use proof bytes or proof hashes as unique ids (cap malleability in 0.8.0).
13. Reject wrong-length halos in the AIR path, as `decode_inputs` does.
14. On any AIR change, bump the digest tag version (G3) and re-pin the test vectors.

### 5.2 The existing tests and their weak spot

**water3d** (`air.rs:687-751`)
- `trace_matches_step`.
- `honest_proof_verifies_and_tampering_is_rejected`. The verifier sees a tampered `X_out` with matching roots, a tampered halo with a matching `InputRef`, and a wrong `prev_state_root`.
- `tampered_traces_are_rejected`. Four trace edits:
  - `Q` on row 5;
  - `REM + 2^12`, `QN − 1` on row 0;
  - `NB` on row 7;
  - `REM + 1` on row 20.

**toy** (`crates/umis-stark/tests/toy.rs:87-236`, for Goldilocks and KoalaBear)
- honest proof plus a serde round-trip;
- setup reuse, where proof A must not verify claim B;
- tiny shapes;
- wrong `X_out`;
- wrong `X_in`;
- a tampered cell or neighbour read, with the row re-derived;
- a remainder out of range, consistent elsewhere;
- a fractional output;
- a wrong memory multiplicity;
- a wrong range multiplicity;
- the reference witness.

**Weak spot: the tamper tests never reach the verifier in normal test runs.**
- `prove_batch` runs `check_constraints` and `check_lookups` under `#[cfg(debug_assertions)]` (`0.8: p3-batch-stark/src/prover.rs:305-351`).
- `rejected`/`accepted` count a prover panic as a rejection (`air.rs:728-736`; `tests/toy.rs:73-81`).
- `cargo test` keeps debug assertions on, even with `opt-level = 3` for task-water3d (`Cargo.toml` `[profile.dev.package.*]`).
- So `tampered_traces_are_rejected` and the toy tamper tests only test Plonky3's **debug checker**.

Fix:
- Run them in CI with `--release`, or set `[profile.test.package.p3-batch-stark] debug-assertions = false` for a dedicated test job.
- Assert `Ok(Err(_))` from `verify`, not just "not accepted", so a prover panic cannot pass.

Further weaknesses in the existing water3d tamper cases:
- The `REM + 1` on row 20 is described as a "fractional output", but `qn` is not re-solved. It only breaks the stencil identity.
- The row-0 case breaks the stencil range, `lo`/`hi` and the memory all at once. Neither isolates the range check.
- The `NB` edit is not re-derived, so the stencil catches it before the memory bus does.
- The toy has a `rederive_row` helper (`tests/toy.rs:42-58`) for this purpose. water3d needs one.

### 5.3 Negative tests that are missing

water3d unless noted. Each should check `verify` returns `Err` with debug checks off.

1. **Isolated memory-bus tamper.** Change `q` or `nb[d]` on one row, re-derive that row's `qn`/`rem`/`lo`/`hi` and the multiplicities so that every local constraint holds, and expect rejection by LogUp alone (the toy has this; water3d does not).
2. **Isolated remainder range.** At the last substep set `rem += 2^s`, `qn −= 1`, re-derive `lo`/`hi`, refill `M_REM`/`M_BYTE`, and claim the matching `X_out` with a consistent root/outputs. The rejection must come from the rem bus (toy: `remainder_out_of_range_rejected`).
3. **Fractional `qn`.** Choose a different in-range `rem`, set `qn = (L − rem)/2^s` in the field, and give `lo`/`hi` any value. The rejection must come from the byte bus (toy: `fractional_output_rejected`).
4. **Table-multiplicity tamper.** `M_REM ± 1` and `M_BYTE ± 1` (toy: `wrong_range_multiplicity_rejected`). Also a negative test that **preprocessed** `MULT`/`IN_MULT` cannot be altered: proving with a modified `ProverData` must fail against the verifier's locally built `CommonData`.
5. **Tampered `X_in`.** Use a different prev state with a matching `prev_state_root` (only `X_out` and the halo are tested now).
6. **Slot and tide binding.** A valid proof for slot s, verified as slot s+1 with otherwise identical data, must fail. That covers both the digest and the tide.
7. **params/seed binding.** A proof for zone A must fail under zone B's params, or under a different genesis seed with the same params (different preprocessed commitment).
8. **Digest property test.** `claim_data`'s public values must change when any single byte of `params_hash`, `slot`, `X_in`, `X_out` or any halo cell changes, including halo cells that are interior and therefore unused by the AIR. This locks in rule 1 without needing an adaptive-attack harness.
9. **Proof-bytes tampering.** Flip:
   - each commitment (main, permutation, quotient);
   - a lookup terminal;
   - `degree_bits`;
   - an opened value;
   - a FRI final-poly coefficient and a query index;
   - a PoW witness.

   Expect `Err`. Also truncate or extend the proof and expect `Malformed`/`Err`, never a panic. Wrap the call in `catch_unwind` and require `Ok(Err)`.
10. **Lookup terminal non-zero.** A hand-built single-instance proof whose terminal is non-zero must fail at `verify_terminal_sum`. This is covered by 9 if the terminal edit is done carefully.
11. **FRI preset mismatch.** A `Test`-preset proof must fail under a `Benchmark` verifier, and the reverse.
12. **`validate_params` boundaries.** `shift = 15, 16` rejected once capped. A unit test should also show the wrap at `s = 16` (`2^16·32512 + 1 = p`).
13. **Halo length.** A wrong-length halo payload with a matching `InputRef` must be rejected by `verify` (today it is zero-filled).
14. **Native value bounds.** Once added, an `X_in` or halo value with `|v| > 2^15` must be rejected natively.
15. **Count-gate soundness (toy or a dedicated AIR).** A demonstration that a main-column query count without a boolean constraint, set to `−1`, makes a forged read verify. Then the same AIR with the constraint must reject. This documents R3 as a regression test.
16. **Cross-AIR bus** (once a second AIR exists). An unmatched message in instance 2 must fail, and so must a message whose width differs (expect a setup panic or error).
17. **Interior-halo independence (positive control).** Changing a halo cell that no boundary cell reads must still change the digest (test 8), and therefore must fail against the old proof.

**Toy-specific gaps**
- The toy has no proof-bytes tampering (test 9).
- It has no public-values/periodic mismatch test. Example: verify with an AIR whose periodic `x_out` differs while the public values stay the same. That would show FS binding by public values as opposed to the constraint check.
- `tests/toy.rs:126-145` cover only the case where both are changed together.
