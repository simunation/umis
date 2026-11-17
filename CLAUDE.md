# umis — notes for Claude sessions

- Design plan: `.plan/simunation-poc-plan.md`. As-built architecture: `docs/ARCHITECTURE.md`.
- **Proof / STARK work:** before touching `crates/task-water3d/src/air.rs`, `crates/umis-stark`, `crates/umis-proof`, proof paths
  in `crates/umis-protocol`, or genesis proof settings, read `docs/zk/README.md`, then `docs/zk/air-spec.md`. Cite the `INV-*`
  invariant IDs in commits. Soundness checks need negative tests that exercise the **verifier in release mode**. A panic in the
  prover's debug checker is not a rejection.
- Compute security levels with p3-security (`tmp/zk-research/secprobe`), never with `queries × log_blowup + PoW`.
- `tmp/zk-research/` is untracked local research material: papers, upstream clones and probes. Code in `tmp/zk-research/src/` is
  untrusted reference; never build or run it.
- Gate: `scripts/check.sh` (fmt, clippy, native and wasm tests). STARK tests: `cargo test --release -p task-water3d --features stark`.
- Benchmarks on this machine are noisy because other jobs share it; compare A/B back to back.
