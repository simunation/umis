# GPU proving: the device-resident life2d prover

## Summary

`umis_gpu::prover::GpuStarkProver` is a generic GPU-resident STARK prover: it proves any Plonky3 uni-STARK AIR over
KoalaBear whose constraints are base-field polynomials (main trace, next row, periodic columns, public values,
first/last/transition selectors; no lookups, preprocessed columns or extension-field constraints) and produces the
**CPU Keccak configuration's proof** (`umis_stark::config::koala_bear_keccak::Config`), bit-compatible: every node
verifies it with the unchanged CPU verifier. The constraint quotient is compiled from the AIR's symbolic
constraints (below). `task_life2d::air::gpu::GpuProver` uses it for advancing life2d slots and adds only life's
trace-generation kernels. RTX 3090 Ti, one proof at a time:

| zone | CPU, Keccak, 8 threads | GPU-resident | GPU vs native step |
|---|---|---|---|
| 64×64, k = 4 | 4.3 ms | **2.9–4.1 ms** | 25× |
| 128×128, k = 8 | 10.4 ms | **4.6–5.3 ms** | 6× |
| 256×256, k = 8 | 18.7 ms | **7.0–7.8 ms** | 2.3× |
| 256×256, k = 16 | 44.7 ms | **12.2–12.8 ms** | 2× |

(quiet machine, ranges over sessions; under load 16–19 the CPU prover triples while the GPU prover stays within
about 1.5×). Four provers sharing the GPU reach about 1.3–2× more proofs per second. The compiled generic quotient
is as fast as the hand-written life2d kernel it replaced (same run, 30 proofs each: 12.24 vs 12.24 ms at k = 16,
7.6–7.8 vs 7.9–8.1 ms at k = 8).

On a 10-node devnet of 256×256 zones whose nodes all prove on the 3090 Ti, every commitment verified; at load
35–43 it had 18 (fork) rollbacks in 60 beacon slots against 36 with CPU proving, and twice as many proven or settled
commitments.

Use: build with `--features cuda` (`umis-cli`: `CLI_FEATURES=cuda scripts/devnet.sh …`), a `koalabear-keccak`
genesis (the default), and run provers with `UMIS_GPU_PROVE=1` (`UMIS_GPU=3090` picks the device by name;
`UMIS_GPU_CPU_THREADS`, default 4, sizes the prover's own CPU pool: more threads hurt on a busy machine).
`UMIS_GPU_PROFILE=1` prints phase timings and program statistics; `UMIS_GPU_JIT=0` interprets the constraint bytecode
instead of compiling it; `UMIS_GPU_GRIND=0` keeps proof-of-work grinding on the CPU.

**P100 preset (2026-11-19).** Genesis ids now use `FriPreset::P100` (blowup 4, fold arity 8, 128 queries, grinding
at the query, out-of-domain and batch sites). The GPU prover needed no change: blowup, arity and the extra
grinding sites come from the config (the batch and OOD grinds also run on the GPU). Back to back on the 3090 Ti,
256×256 k = 1: 3 provers × batch 16 0.62 → 0.77 ms per slot, one proof at a time 3.0 → 3.7 ms. Per batch of 16
(`UMIS_GPU_PROFILE=1`, quiet): absorbing the opened values into the transcript 4.3 ms (CPU), FRI 2.8 ms (CPU), batch
grind 0.45 ms, reduced openings 0.8 ms; the tables below are the earlier `Benchmark` runs.

**Plan: FRI on the GPU (not built).** FRI is now about 13% of a batch on one prover and overlaps the GPU work of
other provers, so it would save at most about 0.1–0.2 ms per slot. Steps, in order: (1) fold the reduced-opening
vector on the device (arity-8 fold kernel over EF4, bit-reversed as `TwoAdicFriFolding`); (2) commit each folded
layer with the existing Keccak leaf/layer kernels (`MerkleTreeMmcs` over EF4 rows of 8, byte-compatible, tested
like `device_tree_equals_cpu_tree`); (3) keep the CPU transcript: observe each layer root, sample beta, grind on
the GPU; (4) answer queries by gathering sibling rows and digest paths from the device trees, as `GpuMmcs` does for
the input trees; (5) emit `FriProof` unchanged and test against `verify_fri`. The larger CPU item, absorbing about
9 500 opened values, is fixed by the proof format.

## Batch proofs (k = 1, the default)

With one generation per slot, a proof's fixed cost (Merkle trees, transcript, FRI, its 16-bit proof-of-work) is
most of it. `GpuStarkProver::prove_batch` makes one `p3_batch_stark` proof over several instances of one AIR, the
same proof `p3_batch_stark::prove_batch` makes on the CPU (same transcript, `BatchProof<Config>`), checked by the
unchanged `verify_batch`; the node's `prove_batch` uses it for the advancing slots of a commitment (INV-LIFE-BATCH-1..3:
head and references as on the CPU). Measured per slot, 256×256 and 64×64 at k = 1 (A6000, the 3090 Ti was busy with
other jobs; load 9–20):

| | native step | 1 proof per slot | batch 4 | batch 16 | 3 provers × batch 16 |
|---|---|---|---|---|---|
| 256×256 | 0.65 ms | 3.6–5.6 ms | 1.7 ms | 1.2 ms | **0.83 ms (1.3× native)** |
| 64×64 | 0.04 ms | 2.1–4.9 ms | | 0.41 ms | 0.5 ms |

Trace layout matters (3 provers × batch 16, 256×256, k = 1, strips/rows per trace row): 8/3 0.75–0.82 ms,
16/3 0.90, 8/2 0.93, 8/5 0.95, 4/2 0.97, 4/3 1.00, 16/4 1.08, 8/4 1.10, 4/4 1.18, 32/2 1.80; at 64×64 they are
within 0.29–0.45 ms (4/2 0.31). `umis-cli genesis` therefore defaults to 8/3 from 256 cells per axis and 4/2 below.

The protocol batches a zone's slots per beacon commitment: 4 at the default 16-tick beacon slot and 4-tick zone
slots, i.e. one generation per second, far below what the prover sustains.

What made the batch cheap:
- **One device matrix per commitment.** Plonky3's MMCS hashes the concatenated rows of all matrices of one height,
  so the instances' trace LDEs (and all quotient chunks) are stored side by side as one matrix: one leaf-hash
  launch, one row gather per FRI query batch, one out-of-domain column dot product per opening point, and one
  FRI-input row dot product per point (with the per-column alpha powers of `TwoAdicFriPcs::open`'s running
  offsets), whatever the number of instances.
- **The quotient of all instances in one launch** (grid = points × instances), written chunk-major so each chunk's
  coset LDE runs on the device (`commit_quotient`); nothing comes back to the host.
- **Proof-of-work on the GPU** (`pow.rs`): Poseidon2 (KoalaBear width 16, round constants from `p3-koala-bear`)
  tests 4·2^bits candidates per launch, 0.25 ms against 4–11 ms on the CPU. `HybridChallenger::grind` uses it
  through `umis_stark::config::koala_bear_keccak::set_grinder`, and accepts the witness only after the CPU
  `check_witness` on a copy of the transcript, else it grinds on the CPU: prover side only, the proof and verifier
  are unchanged. `gpu_poseidon2_equals_cpu_and_grinds` checks the permutation bit for bit and the witnesses.
- **Claims in parallel** (trace generation on the GPU, claim data on the CPU) and the tree's digest layers in one
  download.

Remaining per batch of 16 (256×256): absorbing the ~9 500 opened values into the Poseidon2 transcript, ~3 ms
(sequential, CPU; the proof format fixes it), CPU FRI ~1.5 ms, quotients ~2 ms, claims ~1.5 ms (in parallel), trace
tree ~1 ms. Concurrent provers overlap the CPU and GPU parts. Rule kept from a deadlock found here: no rayon work
while holding the device lock (a pool worker may be waiting for the device).

`gpu_batch_proofs_verify_with_the_cpu_batch_verifier` covers m = 2…8 instances (16–256 cells, k = 1 and 4, absent
bands): the batch verifies after a postcard round trip; a member's changed public value or two swapped members are
rejected; three concurrent batch provers verify.

## Design

How the GPU STARK provers we studied work (SP1 Hypercube, OpenVM's CUDA backend, ICICLE-Stwo, Airbender): the
witness, its low-degree extension, the Merkle trees, the constraint quotient and the FRI layers stay in device
memory; only commitments, out-of-domain values and the queried openings come back. We do the same, but keep
Plonky3's own transcripts and FRI driver on the CPU, which keeps the proof format exact:

| step | where | what |
|---|---|---|
| trace generation | GPU | `life_gen0/life_gen_step/life_fill`: only the extended board (about 74 KB at 256×256) goes up |
| trace coset LDE (blowup 2) | GPU | `lde.cu` (radix-2 DIF stages, bit-identical Montgomery arithmetic) |
| trace Merkle tree | GPU | `resident.cu` Keccak-f[1600] leaves and layers, byte-compatible with `MerkleTreeMmcs` |
| alpha, zeta, PoW | CPU | Plonky3's `StarkProverTranscript` / `PcsProverTranscript` (unchanged) |
| periodic columns on the quotient domain | GPU | coset LDE of the 45 columns (Plonky3 does one CPU DFT per column: 8.7 ms) |
| quotient | GPU | kernel compiled from the AIR's symbolic constraints (below), weight `alpha^(T−1−i)` for constraint i |
| quotient chunks | CPU + GPU | split and LDE on the CPU (2 × 1024 × 4 values), Merkle tree on the GPU |
| out-of-domain values | GPU | Lagrange weights on the low coset (CPU) times the LDE columns (`ext_col_dot`) |
| FRI input | GPU + CPU | alpha-compressed LDE rows (`ext_row_dot`), combined on the CPU as in `TwoAdicFriPcs::open` |
| FRI commit, fold, queries | CPU | `p3_fri::prover::prove_fri` with the device tree as input MMCS (`GpuMmcs`): queried rows are gathered on the GPU, pruned paths built from the downloaded digest layers |

The values computed on the GPU are polynomial evaluations and fixed linear combinations, so they are unique; only
the Merkle trees and the order of transcript operations had to match Plonky3 byte for byte.

Correctness checks (`--features cuda`, run with a GPU): `gpu_lde_equals_cpu`; `device_tree_equals_cpu_tree`
(roots, opened rows, single and pruned multi-paths, trace commitment); `gpu_proofs_verify_with_the_cpu_verifier`
(16…256 cells, with absent halo bands; postcard round trip; a tampered trace is rejected; the GPU-generated trace;
12 concurrent proofs); `generic_gpu_proofs_verify_with_the_cpu_verifier` (a Fibonacci AIR with selectors, public
values and a period-4 periodic column at 2^3, 2^6, 2^10 rows; a false public value is rejected; compiled and
interpreted kernels give identical proofs). Soundness rests on the CPU verifier, which is unchanged.

## The generic constraint kernel

`prover::compile` walks the AIR's `SymbolicExpression` DAG (from `get_symbolic_constraints`) into postfix bytecode,
one sequence per constraint. Plonky3 shares subtrees as `Arc`s (`double()`, `x * x`), so a naive walk is exponential:
nodes referenced more than once are computed once into a slot (`STORE`/`LOAD`). Constraints with the same opcode
sequence (only trace columns, periodic/public indices and constants differ) form one **template**; their arguments
go into a table. The life2d AIR at 256×256, k = 16 has 4719 constraints but only 11 templates.

Each template is then emitted as straight-line CUDA (one SSA value per operation, registers instead of a stack;
arguments that are affine in the instance number, which is most of them, are computed instead of loaded) and
compiled once per AIR shape with NVRTC straight to a CUBIN for the device (`jit.rs`; PTX would fail here because the
toolkit is newer than the driver). The kernel runs one block per quotient point, threads over a template's
instances, so a warp's lanes execute identical instructions on neighbouring columns. Compilation takes 150–270 ms,
once per shape and process; if it fails, the same tables run on a bytecode interpreter (`air_quotient` in
resident.cu).

Quotient time at 256×256, k = 16 (2048 points): bytecode interpreter, one constraint per thread 5.2 ms (divergent
`switch`, stack in local memory); interpreter over templates 2.0 ms; compiled 0.65 ms, as the hand-written kernel.

Water is not proved on the GPU: its proof is a batch STARK with LogUp lookups (`p3-batch-stark`), which needs a
device version of the batch prover and of the lookup argument, not only a constraint kernel.

Where the 256×256 time goes (about 7 ms): trace generation 0.2 ms, trace LDE 0.7 ms (21 radix-2 passes over the
matrix, memory-bound), trace Merkle tree 0.75 ms (2048 leaves, each a sequential 60-permutation Keccak sponge, so
the tree cannot fill the GPU; one Keccak lane per thread in registers was the fastest of the variants tried, a
warp-per-leaf version was shuffle-bound and slower), quotient 0.5 ms, quotient chunks 0.5 ms, out-of-domain values
0.4 ms, absorbing about 4000 opened values into the Poseidon2 transcript 1.3 ms (CPU, sequential, same on the CPU
path), CPU FRI (commit phase, 16-bit query grinding, queries) about 1.5 ms, claim data and setup about 0.5 ms.

Kernel lessons: threads of a warp interpreting different constraints serialise on the opcode `switch` (group
constraints by template); a Keccak state indexed at run time lives in local memory (208-byte stack frame; unrolled block
absorption keeps it in registers, -25%); per-thread loops over rows with dependent loads are latency-bound (column
dot products: one thread per column 250–330 µs, 32 row chunks + a second pass 30–90 µs); Plonky3's CPU periodic
table (one DFT per column) cost 8.7 ms and runs as one batched GPU LDE instead.

## Next steps

- FRI commit phase on the GPU (about 1.5 ms per proof) and a faster trace LDE (fused radix-2^k passes).
- Fewer opened values (the transcript absorb grows with the trace width): a narrower trace per cell.
- Fix the A6000 link (below) — not needed by the resident prover, which moves little data, but needed for anything
  that streams traces.

## Legacy: LDE offload only

`umis_gpu::GpuDft` (coset LDE as a Plonky3 `TwoAdicSubgroupDft`) still exists; on these links it was about 2×
slower than the CPU because the trace and its LDE crossed the bus (8 MB up, 16 MB down).

## The PCIe link problem

| GPU | bus | link now | GPU max | slot max | measured |
|---|---|---|---|---|---|
| RTX A6000 | 01:00.0, CPU root port 00:01.0 | **Gen1 ×16 (2.5 GT/s), also under full load** | Gen4 ×16 | Gen5 ×16 | 3.3 GB/s |
| RTX 3090 Ti | 08:00.0, chipset root port 00:1c.4 | Gen3 ×4 under load | Gen4 ×16 | **Gen3 ×4** | 3.3 GB/s |

Board MSI MAG Z690 TORPEDO (MS-7D32), BIOS A.70 (2022-07-04); kernel 6.8.0-138; NVIDIA **open** kernel module
595.99.02 (`NVreg_DynamicPowerManagement=3`).

**A6000: probably a driver bug.** Zero link replays and no AER errors, and at boot the kernel did not report a
limited link (it did for the 3090 Ti), so the link trained at full speed and later fell to Gen1 without retraining
upward, even at 270 W and full clocks. This matches NVIDIA/open-gpu-kernel-modules
[#1010](https://github.com/NVIDIA/open-gpu-kernel-modules/issues/1010): open kernel modules, Z690 among the
boards, link stuck at Gen1, about 2.5 GB/s; open, internal NVIDIA bug 5953547, no fix yet (as of 2026-11). All reports
there are Blackwell/Ada; none is for Ampere. Similar:
[Ubuntu report](https://discourse.ubuntu.com/t/rtx-5070-pcie-link-speed-stuck-at-gen-1-on-ubuntu-24-04-open-kernel-modules-580-x/77366).

**3090 Ti: the slot.** Its root port is wired Gen3 ×4 (about 3.9 GB/s theoretical). The board has one CPU ×16 slot
(the A6000's), so this card cannot do better here.

### Fixes to try (need root; 3 and 4 disturb jobs on the GPU)

1. Reboot, load the GPU and check `nvidia-smi --query-gpu=pcie.link.gen.current --format=csv` (expect 4). If it is
   fine after boot, it is the dynamic downshift: apply 2.
2. Keep the link from switching: `sudo nvidia-smi -pm 1; sudo nvidia-smi -i 0 -lmc 8001,8001` (lock the memory
   clock; a little more idle power).
3. Retrain at Gen4 without rebooting: `sudo setpci -s 00:01.0 CAP_EXP+30.w=0004:000F` (target 16 GT/s), then
   `sudo setpci -s 00:01.0 CAP_EXP+10.w=0020:0020` (retrain); check `sudo lspci -vv -s 01:00.0 | grep LnkSta`.
   Reports say it may drop again.
4. Proprietary instead of open kernel module (Ampere supports both), e.g. `nvidia-driver-595` without `-open`,
   or first `options nvidia NVreg_DynamicPowerManagement=0x00` in `/etc/modprobe.d/nvidia.conf`; reboot.
5. BIOS: update from A.70; set PCI_E1 / PEG max link speed to Gen4 (not Auto). Rarely enough alone.

Then rerun `pcie_bench` (expect about 20–25 GB/s on Gen4 ×16) and `bench_life_air` with `LIFE_HASH=keccak-gpu`.

