# Upstream report and fix: periodic columns missing from two AirLayouts (Plonky3-recursion `3a69ba6`, = main on 2026-11-24)

Status: fix committed on branch `fix/periodic-columns-in-air-layouts` (commit `013a10de`) in a local clone; the push to
the fork `csiki/Plonky3-recursion` was blocked by the session's permission system, so nothing is posted yet.
Commands to post (run by the owner):

```sh
cd <clone>   # /tmp/claude-1000/-u-proj-umis/<session>/scratchpad/p3r, or re-create it with the patch below
git push fork fix/periodic-columns-in-air-layouts
gh issue create -R Plonky3/Plonky3-recursion --title "Batch transcript replay and declares_interactions ignore periodic columns" --body-file issue.md
gh pr create -R Plonky3/Plonky3-recursion --base main --head csiki:fix/periodic-columns-in-air-layouts --title "fix(recursion): declare periodic columns in batch replay and interaction layouts" --body-file pr.md
```

## Issue text

**Title:** Batch transcript replay and `declares_interactions` ignore periodic columns

Two places build an `AirLayout` without the AIR's periodic columns:

- `recursion/src/generation.rs`, `replay_batch_stark_transcript`: for a batch AIR with periodic columns the quotient
  degree computation panics in `AirLayout::validate_against_air` ("AirLayout num_periodic_columns does not match AIR",
  left 0, right n). Native `p3-batch-stark` proves and verifies such AIRs, so their proofs cannot be recursed.
- `recursion/src/traits/air.rs`, `RecursiveAir::declares_interactions`: the AIR is evaluated with no periodic values.
  An AIR that indexes them panics; one that tolerates their absence (e.g. substitutes zeros, or gates a lookup on a
  periodic selector) can report fewer interactions than it has, and this check decides whether lookups must be
  enforced.

`eval_folded_circuit` and `get_log_num_quotient_chunks` in the same file already set `num_periodic_columns`.

Repro: a one-column AIR with periodic column `[1, 0]` and constraint `periodic·(x − 1) = 0`; `prove_batch` and
`verify_batch` succeed, `replay_batch_stark_transcript` panics.

## PR text

**Summary:** declare `num_periodic_columns` in the two `AirLayout`s that omitted it. Fixes #<issue>.

**Why:** see the issue; both now match `eval_folded_circuit` / `get_log_num_quotient_chunks`.

**Changes:**
- `generation.rs`: `num_periodic_columns: air.num_periodic_columns()` in the batch replay layout.
- `traits/air.rs`: the same in `declares_interactions`.

**Tests:** `tests/periodic_columns.rs` (in the `fri_recursion` suite): a batch proof of a periodic-gated AIR replays;
`traits::air::tests::interaction_air_with_periodic_columns_is_detected`. Both fail before the fix (panics quoted in the
issue). `cargo test --release -p p3-recursion --lib --test fri_recursion`: 498 + 89 passed;
`cargo clippy -p p3-recursion --all-targets -- -D warnings` clean; nightly rustfmt clean.

## The patch (same as umis `vendor/p3r-periodic-layout.patch`, plus the tests)

Re-create with `patch -p1 < vendor/p3r-periodic-layout.patch` on a clone of `main`, then add the two tests described
above.

Full upstream commit (fix + tests): `.plan/drafts/p3r-periodic-columns.upstream.patch`
(`git am` it on a clone of `main`).
