# UMIS proof flow, end to end (read-only analysis)

Repo: `/u/proj/umis`, branch `nu`. Every path below is relative to the repo root. Line numbers are from the current tree.

---

## 0. Cast and data types

| Item | Where | Notes |
|---|---|---|
| `ProofBackend` trait (`id`, `is_async`, `identifies_prover`, `prove`, `verify -> NodeId`) | `crates/umis-proof/src/lib.rs:142-158` | Both flags default to `false`. |
| `MockBackend` (the proof is a borsh `MockProof{prover, sig}` over `stmt.address()`) | `umis-proof/src/lib.rs:161-196` | `identifies_prover = true`, `is_async = false`. |
| `Water3dStark` (Plonky3 0.8, KoalaBear, batch-STARK) | `crates/task-water3d/src/air.rs:431-596` | `is_async = true` (`:507`), `identifies_prover = false`, and `verify` returns `NodeId::default()` (`:594`). |
| `Witness` / `PublicData` / `LeafData` / `ProveJob` | `umis-proof/src/lib.rs:28-38 / 43-52 / 56-60 / 64-75` | `ProveJob` is self-contained (task_id, params, genesis_seed, prev_state, inputs) so a worker can run it. |
| `JobRunner` (caches instances per task, params and seed) | `umis-proof/src/lib.rs:108-140` | |
| `proof_leaf`, `statement_of`, `proof_bmt_root`, `check_leaf_chain` | `umis-proof/src/lib.rs:199-256` | `proof_address = bmt::file_address(proof bytes)`. |
| `ProofLeaf`, `ProofCommitment`, `TransitionStatement` seg records | `crates/umis-core/src/records.rs:49, 91, 160` | |
| `ProofShare{leaf, proof, block_hash}` | `crates/umis-protocol/src/types.rs:90-95` | |
| `ProofCommitEnvelope{pc, sig, leaves, proofs, data: LeafData}` | `types.rs:126-134` | |
| `BeaconBody.proof_commitments: Vec<ProofCommitEnvelope>` | `types.rs:246-250` | The full proofs and LeafData travel inside beacon blocks. |
| Topics `zone/<z>/proof` and `zone/<z>/out`; `Gossip::ProofCommit` / `ProofShare` | `types.rs:277, 289, 322-324` | |
| `Event::ComputeDone`, `Action::Compute(Box<ProveJob>)`, `NodeConfig::max_jobs` | `crates/umis-protocol/src/node/mod.rs:130, 181, 62/76` | `max_jobs` defaults to `usize::MAX`. |
| Ctx memos: `proofs` (verified pairs), `addresses` (proof bmt address) | `crates/umis-protocol/src/ctx.rs:63-70, 114-166` | |

---

## 1. ProveJobs: creation, queueing, dispatch, execution, return

### 1.1 Creation

`ZoneEngine::make_shares(ctx, chain, key, h, all, next_job)` at `zone/engine.rs:593-658` creates the jobs.

- It walks `e.slot_roots`, one entry per slot the block covers. A slot is "mine" if `all` is set or if `ranks::priority_prover(a, z, j, minter) == me` (`engine.rs:615`).
- `priority_prover` (`ranks.rs:95-100`) ranks the zone's members plus all full nodes by `H(seed‖"prove"‖z‖j‖id)` at the anchor state, and skips the minter unless it is the only candidate.
- Duplicates are suppressed through `self.shares` (proofs already held) and `self.proving` (jobs in flight), keyed by `(block_hash, slot)` (`:618`).
- **Async backend** (`is_async`): `ProveOut::Job(ProveJob{…, prev_state: state.clone(), inputs: inputs.clone()})` (`:620-632`). Each job carries a full copy of the 64 KiB state and the input payloads.
- **Inline backend** (mock): `ctx.backend.prove` runs immediately, and the result is stored as a share and returned as `ProveOut::Share` (`:633-647`).
- **Multi-slot blocks** (a missed slot, so `from_slot < slot`): `inst.step` re-derives each intermediate state, with the same inputs for every slot (`:650-655`).

Three call sites:
1. `after_accept`: every accepted zone block (`node/duties.rs:262-290`, call at `:288`), with `all=false`.
2. `mint`: the minter's own block (`duties.rs:708-713`), with `all=false`. The minter is excluded from priority, so this normally yields nothing.
3. Delegate fallback in `zone_duties` (`duties.rs:652-666`): when `build_proof_commitment` returns `None` under an async backend, the delegate (`zc.commitment.signer == me`) calls `make_shares(..., all=true)` for every block of the commitment.

### 1.2 Queue, prioritization, dispatch

- `dispatch_proofs` (`duties.rs:293-305`) publishes `Share`s on **both** `ZoneProof(z)` and `ZoneOut(z)`, and pushes `Job`s into `job_queue` (`node/mod.rs:227`).
- `pump_jobs` (`duties.rs:311-353`) does the following, in order:
  - builds `committed`, the blocks of pooled zone commitments that have no proof commitment yet (`:317-322`), and `canonical`, the best-tip ancestors of every engine (`:323-332`);
  - **drops** jobs whose block is neither committed nor `valid_under` the head, and counts them in `stats.jobs_dropped` (`:333-342`);
  - sorts by `(!committed, !canonical, from_slot, zone, id)` (`:343-345`);
  - emits `Action::Compute` while `jobs.len() < max_jobs` (`:347-351`). The in-flight map is `jobs: BTreeMap<u64,(ZoneId,ProveJob)>` (`mod.rs:226`).
- `max_jobs` is set to 1 on native nodes (`umis-cli/src/node_cmd.rs:68`) and on browser nodes that use a worker prover (`umis-web/src/simnode.rs:146`). Everywhere else it stays `usize::MAX`: browser `?prover=inline` and SimNet.

### 1.3 Execution

- **Native**: `Runtime` matches `Action::Compute` and, if an executor is set, calls `Executor::submit`; otherwise it runs `umis_proof::run_job` inline on the event loop (`umis-node/src/lib.rs:251-259`).
  - `ThreadExecutor` (`lib.rs:99-135`) is a dedicated rayon pool with `UMIS_PROVER_THREADS` threads, default 2 (`:113`). It wraps a `JobRunner`.
  - The executor shares `node.ctx.backend` (`node_cmd.rs:78-81`), so the STARK `setups` cache (`air.rs:434, 456-480`) is common to proving and verifying.
  - Plonky3's `p3-maybe-rayon` runs on the *current* pool inside `pool.spawn`, so one job gets 2 threads.
- **Browser**: `JsExecutor::submit` (`umis-web/src/prover.rs:47-62`) borsh-encodes the job and calls the JS `prover` function.
  - `web/src/nodes.ts:74-77` wires that function to `provers().prove(genesisJson, secret, job)`.
  - `ProverPool` (`web/src/prover.ts:34-47`) runs up to `min(4, cores/2)` workers shared by the page's nodes.
  - `prover.worker.ts:18-26` calls the wasm `proveJob` (`prover.rs:23-40`). That function caches a `JobRunner` per `(genesis hash, node id)` in a thread-local, so each worker has its own STARK setup.
  - Every job sends the full genesis JSON and the node's **secret key in hex** to the worker. The STARK prover ignores the key (`air.rs:511`, `_prover`), so it does not need to be sent at all.
- **SimNet**: `net.rs:278-285` runs `run_job` inline and delivers `Msg::Computed` after `prove_delay_ms`.

### 1.4 Return

- `select_biased!` (`umis-node/src/lib.rs:323-327`) polls network events first, then `done_rx` (which feeds `Event::ComputeDone`), then control, with timers last.
- `on_compute_done` (`duties.rs:355-374`) removes the job, re-pumps the queue, calls `eng.job_done` (`engine.rs:661-667`; this clears `proving` and stores the share **without verifying it**), increments `stats.proved`, and gossips on both `ZoneProof` and `ZoneOut`.
- On an error, `proving` is cleared and nothing is retried until another `make_shares` call happens.

---

## 2. ProofShare gossip

- **Created** in `make_shares` (inline backends) and `job_done` (async backends), as above.
- **Received** via `on_gossip` (`node/mod.rs:452-462`), which checks `s.leaf.zone == z`:
  - No engine for `z` (a pure consumer subscribed to `ZoneOut(p)`): `Validation::Accept` **with no check**, so the message is relayed.
  - Otherwise `ZoneEngine::on_share` (`engine.rs:670-703`) decides between Accept and Ignore. It never returns Reject, so peers are never penalized for bad shares.
- **`on_share` steps**:
  1. Drop the share if the block is unknown. Shares are not buffered, so a share that arrives before its block is lost (`:671`).
  2. Drop it if `(block_hash, slot)` is already in `shares`. This is **first-writer-wins** and is checked before any verification (`:673-675`).
  3. Check that the leaf's `state_root` and `outputs_root` equal this node's re-executed slot roots, and that `proof_address == ctx.proof_address(proof)` (`:677-682`).
  4. Verify:
     - Mock: `ctx.verify_proof(stmt, PublicData::default())`, i.e. the signature (`:696`). `identifies_prover` is **not** checked here, so a share signed by any key is stored.
     - STARK: `share_public` (`:862-881`) returns `PublicData` only for **single-slot blocks** whose parent state is held. Then the STARK verify runs **synchronously on the event loop** through the `Ctx` memo (`:689`): about 11 ms at n_loc=16, and `verify_batch` may fan out on rayon's global pool natively. Otherwise the result is `None => true` (`:690`): **the share is stored and relayed unverified** (multi-slot blocks, or a missing parent state).
  5. Insert the share.
- **Pull path**: `SyncRequest::GetProofs` is served (`node/sync.rs:59-69`, with no bound on the size of the range), but no code ever sends it.

---

## 3. ProofCommitment assembly, the wire, and beacon verification

### 3.1 Timing and who builds it

- The minter of a commit period's last slot is the delegate. It sends the zone commitment right after minting (`duties.rs:716-721`, then `commit_now`).
- `zone_duties` (`duties.rs:622-667`) handles proof commitments: for each pooled zone commitment `(z, ce)` without a pooled proof commitment:
  - the delegate tries `build_proof_commitment` immediately, with `self_prove` true from `t_next + B/8`;
  - **any other node** builds one after `t_next + B/4` (`:636`), but only from shares it already holds, since it never self-proves.

### 3.2 `build_proof_commitment` (`engine.rs:800-837`)

- For each covered block and each slot `j ∈ [from_slot, slot]` it takes `shares[(h, j)]`. If one is missing:
  - async backend: it returns `None`, which triggers the delegate job fallback described in §1.1;
  - mock with `self_prove`: it proves inline using `all=true`.
- It does **not** re-verify the shares it collects, so one bad stored share makes the whole proof commitment invalid.
- `ProofCommitment{zone, beacon_slot, leaf_count, proof_bmt_root = list_root(leaves), signer}`, signed with `key.sign(pc.address())`.
- `LeafData` (only when `is_async`, `leaf_data` at `:839-860`):
  - `states`: the parent state of the first block, then the state after every slot. Intermediate states of multi-slot blocks are recomputed with `inst.step`. That makes `leaves+1` full states.
  - `inputs`: one `Vec<(PortId,Bytes)>` per leaf. In a multi-slot block the same block input map is repeated for every slot.

### 3.3 Sizes for n_loc=16 (devnet: B=16 ticks, tick 250 ms, S_z = 4, 8, 4, 16, k_sub=4, shift=12)

- Rows: `max(4·4096, 2^12, 256)` = 16384 = 2^14 (`air.rs:113-120`).
- A state is 4096 cells × 16 B = **64 KiB**. A halo face is 256 cells × 16 B = **4 KiB**.
- A proof is about **261 KB** with the benchmark FRI preset (ARCHITECTURE.md table). The test preset is smaller.
- Leaves per proof commitment: n_z = B/S_z, so 4, 2, 4 and 1 (11 leaves per beacon slot across the 4 zones). Each genesis zone has about 2 neighbour links.

| zone (S_z) | proofs | LeafData states | LeafData inputs | proof commitment total |
|---|---|---|---|---|
| 0 (4) | 4×261 KB ≈ 1.04 MB | 5×64 KiB = 320 KiB | 4×2×4 KiB = 32 KiB | ≈ 1.4 MB |
| 1 (8) | 2 × 261 KB ≈ 0.52 MB | 192 KiB | 16 KiB | ≈ 0.73 MB |
| 2 (4) | ≈ 1.04 MB | 320 KiB | 32 KiB | ≈ 1.4 MB |
| 3 (16) | 261 KB | 128 KiB | 8 KiB | ≈ 0.40 MB |
| **per beacon slot (4 s)** | ≈ 2.9 MB | ≈ 0.94 MiB | ≈ 88 KiB | **≈ 3.9 MB** |

How many times each proof crosses the network:
1. The share goes out on `zone/<z>/proof`.
2. (Removed 2026-11-21, ROADMAP B3.) The share used to go out again on `zone/<z>/out`, where consumers never used it;
   shares now travel on `zone/<z>/proof` only and a share on the output topic is ignored.
3. The proof commitment is gossiped on `commit`.
4. The proof commitment is embedded again in the beacon block, which full nodes fetch again during sync.

That is roughly 3.4 MB/s of proof payload per mesh hop on a 4-zone devnet before mesh amplification (`mesh_n=4`). The `ZoneCommit` gossip also attaches a 64 KiB checkpoint (`duties.rs:740`).

### 3.4 Beacon verification

The entry point is `BeaconState::apply_body` (`beacon/state.rs:478-517`): housekeeping, then txs, then zone commitments, then proof commitments in `(zone, slot)` order, then `resolve`, `global_rollback` if needed, `settle` and `prune`.

`apply_proof_commitment` (`state.rs:987-1076`) runs these checks:
- The signer `can_act_for(z)` and the signature is valid (`:996-997`).
- An **Optimistic** commitment exists for `beacon_slot` (`:999-1001`).
- `leaf_count == pce.leaves.len() == pce.proofs.len() == to_slot - from_slot` (`:1003-1007`). This means exactly one leaf per slot.
- `check_leaf_chain` (`:1008`): same zone and params, `to == next.from`, `state_root == next.prev_state_root`.
- The chain ends equal the commitment's `prev_state_root`, `state_root`, `from_slot` and `to_slot` (`:1009-1012`).
- If `is_async`: `states.len() == leaves+1` and `inputs.len() == leaves` (`:1014-1018`).
- `PublicData` per leaf (`:1027-1040`): `params` = the zone params, `genesis_seed`, `prev_state = states[i]`, `next_state = states[i+1]`, `inputs = input_maps[i]`, and `input_refs` = `c.inputs[b.input_start .. +input_count]`, which is the covering block's InputRefs. Mock gets `PublicData::default()`.
- `ctx.warm_proofs(&warm)` (`:1041-1050`, `ctx.rs:135-145`) runs all leaves with rayon `par_iter` on the **global** pool, natively and only when there is more than one item. On wasm it does nothing, so verification is sequential on the page thread.
- A sequential loop (`:1051-1069`) checks, per leaf:
  - `zone` and `params_hash`;
  - `to_slot == from_slot + 1`;
  - `proof_address == ctx.proof_address(proof)`;
  - the leaf lies inside a committed block, with `leaf.inputs_commitment == block.inputs_commitment`;
  - for the block's last slot only, `state_root` and `outputs_root` equal the block's roots;
  - `ctx.verify_proof(...)`, which is memoized;
  - if `identifies_prover` (mock only), the prover can act for the zone at the slot's anchor or now.
- Finally `proof_bmt_root` must match the recomputed root (`:1071`). The commitment becomes `Proven`.

How `Water3dStark::verify_inner` (`air.rs:550-595`) binds the public data:
1. It needs all of `params`, `seed`, `prev`, `next` and `inputs`; otherwise it returns `Invalid`.
2. `setup(params, seed)` must give the statement's `params_hash`.
3. The statement must cover exactly one slot.
4. `check_roots`: `bmt::file_address(prev/next)` must equal `prev_state_root` / `state_root`, and the sizes must match (`:483-500`).
5. `outputs(x_out)` hashes to `outputs_root`.
6. `list_root_of(input_refs) == inputs_commitment`. Every ref's payload address must match `inputs[link_id]`, and `inputs.len() == refs.len()`.
7. Rebuild the periodic columns and `public = [tide(j), keccak(params_hash‖slot‖X_in‖X_out‖halos) as 16×u16]` (`claim_data` at `:302-345`, digest at `:338`).
8. `postcard::from_bytes::<BatchProof>`, then `verify_batch(config, [air], proof, [public], prover_data.common)`.

What the memo key covers: `(stmt.address(), proof_address)` (`ctx.rs:155`). This is sound only because every piece of public data is bound to a root inside the statement. Failed verifications are **not** memoized (`ctx.rs:159`), so a bad proof commitment is re-verified on every `apply_body` attempt during block building.

### 3.5 Inclusion, L, D and settlement

- **Block building** (`duties.rs:497-581`):
  - With `L=0` (mock), a zone commitment is included only together with its proof commitment (`:520-541`).
  - Then any pooled proof commitment whose commitment is still Optimistic is tried one at a time, each with a full `apply_body` (`:543-563`).
  - `with_pending` (`state.rs:822-845`) applies the same rule when the delegate projects the head state.
- **MissingProof**: `housekeeping` (`state.rs:548-560`) invalidates the first Optimistic commitment with `included_at + L < e` (`RollbackReason::MissingProof`).
  - Devnet (`genesis_cmd.rs:66-68`, `L=1`): a zone commitment included at `e0` needs its proof commitment in block `e0` or `e0+1`.
  - Proving for the last zone slot of `ce` starts at about `t_next`, so the budget is about one beacon slot (4 s).
- **Any invalidation triggers a global rollback** of every active zone (`state.rs:1225-1243` → `global_rollback`). One missed proof rolls back the whole lattice.
- **Settlement** (`settle`, `state.rs:1458-1507`): a commitment settles when it is `Proven`, `included_at + D ≤ e` (D = `anchor_depth`, default 2), `unresolved == 0` (cross-links resolved by `resolve`, `:1414-1452`), its previous commitment is settled, and its producers' commitments are settled. These conditions are computed as a greatest fixed point.
- What D does here:
  - Anchors are taken at `e − D` (`beacon/chain.rs:151-170`), which fixes the ranks and the priority prover of each zone slot.
  - Under mock, D is the **only** window for re-execution fault evidence before a commitment becomes irreversible: `discard_block` skips settled commitments (`state.rs:1215-1219`), so a fault found after settlement only slashes.
  - Genesis requires `U > L + D + 1` (`genesis.rs:211`).

---

## 4. `is_async` / `identifies_prover`; mock vs Plonky3; unverified paths

Every branch on these flags:

- `is_async`:
  - `engine.rs:620` (job vs inline share);
  - `engine.rs:684` (share verification needs PublicData);
  - `engine.rs:815` (no inline self-prove);
  - `engine.rs:834` (build LeafData);
  - `duties.rs:652` (delegate dispatches jobs);
  - `ctx.rs:137` (parallel warm);
  - `state.rs:1014` (require and use LeafData).

  In other words, "async" is used as a stand-in for "needs public data". These are two different properties.
- `identifies_prover`: checked only at `state.rs:1060`. The zone-level `on_share` does not check it.

How validation differs between the backends:

- **Zone members**: they re-execute every block whatever the backend, because `on_block(..., reexecute=true)` is called from `duties.rs:204` (code at `engine.rs:394-410`). A mismatch produces `FaultBody::InvalidTransition{block, parent_state}`. Under Plonky3 the proof therefore gives members **no** savings.
- **Consumers**: they verify only output-bundle structure (`mod.rs:522-544`, `exec::check_attachment`). Producer shares reach them on `ZoneOut` but are accepted and relayed unverified (`mod.rs:461`). Their safety comes from beacon cross-link resolution.
- **Beacon**:
  - Mock: a proof commitment needs only signatures and prover membership. Correctness rests on members' re-execution plus `FaultTx` within D.
  - Plonky3: every leaf is STARK-verified.
- **Fault evidence**: `apply_fault` / `InvalidTransition` **always re-executes** (`state.rs:1095-1105`, `exec::reexecute_block`). The plan's Plonky3 variant ("a valid proof of the correct transition that ends at a different root") is not implemented.
- **Small AIR / `step()` divergence**: `air::halos` (`air.rs:294-299`) maps a wrong-length payload to a zero face, while `Water3d::decode_inputs` (`task-water3d/src/lib.rs:268-283`) returns an error. The beacon could therefore mark a commitment Proven over a block whose `step()` fails. The input is still bound to `inputs_commitment`, so this is low-severity, but the two definitions should agree.

Paths that accept a proof without verifying it:
1. `on_share` for multi-slot blocks, or when the parent state is missing (`engine.rs:688-691`, `None => true`). The share is stored first-writer-wins and relayed with `Accept`.
2. `on_share` for nodes with no engine for the zone (`mod.rs:461`).
3. Mock `on_share`: any validly signed share is accepted, with no prover check.
4. `job_done` stores this node's own proofs unverified (`engine.rs:661-667`). That is reasonable for an honest node.
5. `on_proof_commit` (`mod.rs:511-520`) checks only `ctx.verify(pc.signer, …)`. It does not check `can_act_for` or the leaves, and it is **first-writer-wins per `(zone, beacon_slot)`**. `zone_duties` stops building once `pc_pool` holds the key (`duties.rs:626-627`), and the pool entry is only dropped at `ce+4` (`duties.rs:96-110`).

Taken together: one junk share or one junk proof commitment from any key fills the slot, the beacon refuses it, nobody replaces it, the `MissingProof` deadline passes, and a **global rollback** follows. No proof that fails verification ever reaches beacon state. The cost is liveness and DoS exposure, not safety.

Related malleability: `LeafData` and the proof bytes inside a beacon block are not committed by the header (`body_roots` hashes only `pc.address()`, `state.rs:1552-1556`). They are only checked against the leaf roots. A relayer can therefore produce a same-hash, failing variant of a block. The orphan buffer deduplicates by hash (`duties.rs:33-35`).

---

## 5. Genesis and backend selection

- `Genesis.proof_backend: String`, `stark_config_id: Option<String>` (`umis-core/src/genesis.rs:136-138`). `validate()` (`:196-250`) checks **neither** of them, and does not check that `L ≥ 1` when plonky3 is selected.
- `task_water3d::air::backend_for` (`air.rs:605-611`) maps `"mock"` to `MockBackend` and `"plonky3"` to `Water3dStark::from_config_id` (`:444-450`). That function accepts `koalabear` (`FriPreset::Benchmark`: blowup 2, 100 queries, 16 PoW bits) and `koalabear-test` (4 queries, no PoW), with `koalabear` as the default when the id is `None`. The presets are defined in `umis-stark/src/config.rs:11-37` and `:80-116` (KoalaBear, degree-4 extension, Poseidon2-16 MMCS, `TwoAdicFriPcs`).
- Callers: `umis-cli/src/node_cmd.rs:19-21`, `umis-web/src/simnode.rs:141`, `umis-web/src/prover.rs:33`, `umis-simnet/src/lib.rs:123`.
- `devnet_genesis` (`task-water3d/src/devnet.rs:45-73`) **hard-codes `proof_backend: "mock"`** (`:67-68`), with `n_loc=16` (`:31`), slot ticks `[4,8,4,16]` (`:9`), and an 8×8×2 lattice.
- `umis genesis` (`umis-cli/src/genesis_cmd.rs:35-41, 63-73`) defaults to `--proof-backend plonky3 --stark-config koalabear` and sets `proof_lag=1` when plonky3 is chosen.
- Remaining params:
  - `ChainParams::default` (`genesis.rs:52-70`): `tick_ms 250`, `B 16`, `D 2`, `K_f 2`, `L 0`, `U 20`.
  - `Water3dParams::default_for` (`task-water3d/src/lib.rs:89-97`): `n_loc 16`, `k_sub 4`, `shift 12`.
  - `validate_params` (`lib.rs:202-212`) allows `n_loc ∈ {4,8,16}`, `k_sub` a power of two ≤ 16, `shift ∈ 8..=16`.
- SimNet uses `n_loc 4` (`umis-simnet/src/lib.rs:41`) and forces `koalabear-test` for plonky3 (`:118-121`).

---

## 6. Simulation step and inputs

- **Minting**: `ZoneEngine::mint` (`engine.rs:436-585`).
  - For each `links_at(z, e_b)`, it picks the freshest causal producer bundle from `InputStore` (`zone/inputs.rs:40-66`). The rule is `(j_p+1)·S_p ≤ from·S_c`, excluding discarded, doomed or uncommitted forks, and ties go to the lower rank and then the lower hash (`engine.rs:459-512`). It otherwise holds the last input, and multi-slot blocks always hold.
  - It then calls `exec::execute(inst, pstate, input_map(refs, attachments), from, j)` (`exec.rs:63-79`), which runs `inst.step` and `outputs` once per slot with **fixed inputs**.
  - `input_map` keys payloads by `link_id`, which equals the consumer's input port (`exec.rs:57-60`). The AIR reads ports `0..6` as halo directions (`air.rs:294-299`).
- **Validation**: `engine.rs:394-410` → `exec::reexecute_block`.
- **Proving**: `make_shares` uses `inst.step` for intermediate states. `Water3dStark::prove_with` (`air.rs:522-548`) runs `generate_trace` (`:348-418`), which mirrors `stencil::substep` and is checked bit for bit by the `trace_matches_step` test.
- **Statement per slot**: `exec::slot_statement` (`exec.rs:136-147`). Every slot of a block shares the block's `inputs_commitment`.
- **Physics**: `Water3d::step` (`task-water3d/src/lib.rs:285-293`) → `stencil::substep` (`stencil.rs`). The `DomainSim` reference is in `umis-task/src/sim.rs` (used by the oracle).

---

## 7. SimNet `stark` scenario

- `scenarios::stark` (`umis-simnet/src/scenarios.rs:501-523`) uses `plonky3` with `koalabear-test`, `L=1`, 4 members per zone, `prove_delay_ms=300`, and runs 20 to 40 slots.
- It checks the generic oracle, then requires **at least one** Proven or Settled commitment and **zero rollbacks**.
- The oracle (`oracle.rs`) checks no proofs at all, despite the comment "leaves verified" at `oracle.rs:93`. It checks linkage, physical-time alignment, liveness, replay of committed chains, slashing and rollback reach.
- Caveats:
  - All nodes share **one** `Ctx` (`lib.rs:123-126`), so the proof memo and STARK setups are global: each proof is verified once for the whole network, and per-node verification cost and event-loop latency are not modelled.
  - Proving is free in sim time apart from the fixed delay.
  - `max_jobs` is unlimited.
  - `stark` is **not** in `tests/scenarios.rs`, so it runs only from the CLI.
  - No adversarial scenario (corrupt share, junk proof commitment, withheld proof) runs under plonky3. `faults` uses mock with L=1 (`scenarios.rs:228-293`).

---

## 8. Change touch list (field, AIR, PCS, proof format, aggregation/recursion)

### 8.1 Places that must change

| # | File:line | Why |
|---|---|---|
| 1 | `crates/umis-stark/src/config.rs:11-116` | Field, extension, hash/MMCS, FRI params, presets. A new PCS (e.g. Circle or WHIR) or field goes here. |
| 2 | `crates/umis-stark/Cargo.toml:10-29`, `task-water3d/Cargo.toml:11, 20-27` | Exact `=0.8.0` Plonky3 pins, the `parallel` feature, postcard. Upgrading Plonky3 changes `p3_batch_stark` and `p3_lookup` APIs. |
| 3 | `task-water3d/src/air.rs:43-56` (imports, `Config`, `Val`) | Hard-wired to `koala_bear::{Config,Val}`. Generalizing needs a type parameter or a new backend type. |
| 4 | `air.rs:61-102` (column maps, `N_PERIODIC=32`, `N_PUBLIC=17`), `:104-140` (`Shape`, height rule `max(k_sub·C, 2^s, 256)`) | AIR layout. Any change alters proofs and the verifying key. |
| 5 | `air.rs:161-284` (`BaseAir`/`Air` eval, buses `MEM_BUS`, `REM_BUS`, `BYTE_BUS`) | Constraints, LogUp lookups, degree ≤ 3 (tied to `log_blowup=1`). |
| 6 | `air.rs:302-345` `claim_data` | Periodic columns, plus the **keccak digest of periodic data as 16×u16 public values**. This exists because batch-STARK does not absorb periodic data. A prover that absorbs it, or an in-circuit hash, removes it. The 16-bit limb encoding assumes a field above 2^16. |
| 7 | `air.rs:348-418` `generate_trace` | Witness generation. The field range argument assumes `|q| ≤ 2^15` and `Σw ≤ 2^s` stay below p/2. A smaller field breaks it, and a larger one allows wider fixed-point values. |
| 8 | `air.rs:421-480` `ZoneSetup` / `setup` | `ProverData::from_airs_and_degrees_with_lookup_budgets(..., &[3], 1)` is the preprocessed commitment and verifying key, cached per `(keccak(params), seed)` with no eviction. |
| 9 | `air.rs:522-548` `prove_with` (`postcard::to_allocvec(BatchProof)`), `:550-595` `verify_inner` (`postcard::from_bytes`, `verify_batch`) | Proof format and serialization. Changing either changes `proof_address`, golden tests and the memo keys. A recursion or aggregation layer would replace or wrap this. |
| 10 | `air.rs:507` `is_async`; `:594` returns `NodeId::default()` | Backend flags. A fast verifier, or a prover identity bound into the proof, changes the protocol branches listed in §4. |
| 11 | `air.rs:444-450` `from_config_id`; `:605-611` `backend_for` | New `stark_config_id` / backend ids. Add validation in `umis-core/src/genesis.rs:196-250`. |
| 12 | `umis-proof/src/lib.rs:43-60` (`PublicData`, `LeafData`), `:142-158` (trait) | The public-data model is "full states plus input payloads". Aggregation needs a multi-statement `verify`, i.e. a trait method that takes a list of statements, or a commitment-level proof. |
| 13 | `umis-proof/src/lib.rs:199-256` (`proof_leaf`, `proof_bmt_root`, `check_leaf_chain`), `umis-core/src/records.rs:49, 91, 160` | One leaf per slot, `proof_address = bmt(proof bytes)`, and record tags and versions. A per-commitment aggregate proof changes `ProofLeaf` / `ProofCommitment` (e.g. an `agg_proof_address`) and their seg layout versions. |
| 14 | `umis-protocol/src/beacon/state.rs:987-1076` | Hard rules: `leaf_count == to−from`, `leaf.to_slot == from_slot+1`, `states.len()==leaves+1`, per-leaf `PublicData` construction, `identifies_prover` branch, warm then sequential verify. All of this is rewritten for aggregation or recursion. |
| 15 | `umis-protocol/src/zone/engine.rs:593-658, 661-667, 670-703, 800-881` | Share and job creation per slot, share verification (`share_public` is single-slot only), proof commitment assembly, `leaf_data` (full states). Aggregation would add a job type that merges shares into the commitment proof. |
| 16 | `umis-protocol/src/node/duties.rs:293-374, 622-667` | Job queue and priorities, the `B/8` and `B/4` timings tuned to about 400–700 ms per proof, and the delegate fallback. |
| 17 | `umis-protocol/src/ctx.rs:63-70, 114-166` | Memo keyed `(stmt.address, proof_address)`. Soundness relies on the statement binding every public input. Parallel warm uses the rayon global pool. |
| 18 | `umis-protocol/src/types.rs:90-134, 246-250, 376` | Wire structs: `ProofShare`, `ProofCommitEnvelope{proofs, data}`, `BeaconBody`, `SyncResponse::Proofs`. Changing these changes the borsh layout of the gossip and sync codec. |
| 19 | `umis-proof/src/lib.rs:64-75` `ProveJob`; `umis-web/src/prover.rs:23-62`; `web/src/prover.ts`, `prover.worker.ts`; `umis-node/src/lib.rs:91-135` | Executor plumbing: job encoding, worker cache key `(genesis hash, node id)`, `UMIS_PROVER_THREADS`. |
| 20 | `umis-net/src/behaviour.rs:40` (`max_transmit_size` 32 MiB), `umis-net/src/codec.rs:12,14` (request 1 MiB, response 64 MiB), `umis-protocol/src/node/sync.rs:22` (8 MiB per batch), `node/mod.rs:243` (`MAX_BATCH=32`) | Size limits. A beacon block is about 4 MB with 4 zones at n_loc=16. 32 MiB allows about 8× more zones or leaves before gossip silently fails. The 8 MiB sync cap means about 2 blocks per response. `GetProofs` has no range bound. |
| 21 | Tests and benchmarks: `task-water3d/tests/stark.rs`, `tests/wasm.rs`, `tests/golden-proof-4.txt` (pinned proof address), `examples/bench_air.rs`; `umis-stark/tests/*`, `examples/bench_toy.rs`; `air.rs:613-752` unit tests | Golden proof bytes and addresses change with any format, config or AIR change. |
| 22 | `umis-cli/src/genesis_cmd.rs:35-41, 66-73`; `task-water3d/src/devnet.rs:67-68`; `umis-simnet/src/lib.rs:31-46, 118-121`; `scenarios.rs:501-523` | Backend and config selection, `L`, sim scenario. |
| 23 | Docs: `docs/ARCHITECTURE.md` "Proofs" section; `.plan/simunation-poc-plan.md` §4 | Numbers, deviations ("no recursive aggregation"). |

### 8.2 Hard-coded assumptions

1. **One proof per zone slot**, enforced in four places: `stmt.to_slot == from_slot+1` (`air.rs:529, 563`; `state.rs:1050`), `leaf_count == to−from` (`state.rs:1003-1007`), and per-slot jobs and shares (`engine.rs:613-656`).
2. **LeafData ships full states** (64 KiB each at n_loc=16) and full input payloads, because the verifier evaluates periodic columns from raw cells and the state roots are keccak/Swarm BMTs that the AIR does not prove. Removing this requires either an in-circuit commitment to `X_in`/`X_out` (e.g. a Poseidon2 root carried in the statement) or recursion.
3. **Keccak digest of periodic data** as 16 public 16-bit limbs (`air.rs:330-344`).
4. **Postcard serialization of `BatchProof<Config>`** (`air.rs:547, 590`). The proof bytes are an untyped `Vec<u8>` everywhere else, and `proof_address` is the bmt address of these exact bytes.
5. `is_async` is used as a stand-in for "needs PublicData" and "verify in parallel" (§4).
6. Verifiers re-derive the zone's preprocessed commitment from `(params, genesis seed)` instead of reading a committed verifying key. There is no VK in genesis or in zone records.
7. A single global backend per genesis (`Ctx.backend`). The plan's per-`(task_id, backend_id)` registry is not used, and the backend is water3d-specific (`backend_for` lives in `task-water3d`).
8. Prover identity is only meaningful for mock (`identifies_prover`). STARK shares are anonymous, so there is no accountability or reward hook for provers.
9. Timings: delegate self-prove at `B/8`, others at `B/4` (`duties.rs:636-639`), `L=1`, `max_jobs=1`, 2 prover threads. These are tuned to n_loc=16 at about 400 ms native / 700 ms wasm.
10. Size limits as in row 20 of §8.1. No proof-size cap exists at the protocol level: a 30 MB "proof" in a share is accepted for postcard parsing and hashing.

---

## 9. Key risks and inefficiencies

1. **[Fixed 2026-11-19]** (nodes check a received proof commitment against the head state plus the zone's pending
   commitments before pooling it, keep up to 8 unchecked candidates per key (one per signer), evict candidates that
   fail at block building or on a new head, and `with_pending` no longer drops a zone commitment whose pooled proof
   commitment fails; zone engines track shares they made or verified (`trusted`): an unverifiable share never stops
   self-proving and a verified one replaces it; mock shares need a zone-member prover; failed share verifications
   are `Reject`ed. SimNet `junk-proofs` and `life-stark-junk` run a full node that sends junk proof commitments and
   shares for every commitment.) **First-writer-wins plus missing validation leads to a global rollback (liveness DoS).** Unverified shares (multi-slot blocks, mock shares from any key, nodes without an engine) and proof commitments checked only for their signature both occupy their slot permanently. The beacon refuses them, `MissingProof` fires after L, and `global_rollback` rolls back every zone. Fix by:
   - verifying before storing, buffering several candidates per key, or replacing a pooled item when beacon `apply` fails;
   - checking `can_act_for` and the leaves in `on_proof_commit`;
   - checking `identifies_prover` in mock `on_share`;
   - returning `Reject` for provably bad items.
2. **STARK verification on the event loop.** `on_share` verifies synchronously (about 11 ms per share). `warm_proofs` blocks the loop while it uses the global rayon pool, which competes with the "2-thread prover" isolation. In wasm, every beacon block's leaves are verified sequentially on the page thread, about 120 ms per block for 11 leaves at n_loc=16.
3. **Bandwidth.** Each proof crosses the network about 3 times: the share topic, the proof commitment gossip and the beacon block (the duplicate on `zone/<p>/out` was removed 2026-11-21). Beacon blocks carry about 1 MiB of LeafData states per 4 s.
4. **Re-execution is still mandatory for members under Plonky3**, so proofs serve only the beacon. Fault evidence is re-execution only.
5. **Failed verifications are not memoized**, so block building re-verifies a bad proof commitment on every trial `apply_body`.
6. **The SimNet `stark` scenario shares one Ctx and one memo, is not in CI, and has no adversarial coverage under plonky3.** The oracle does not check proofs.
7. **Smaller issues**:
   - the secret key is posted to workers for no reason;
   - `GetProofs` has no client and no range bound;
   - the AIR's halo zero-fallback differs from `decode_inputs`;
   - genesis does not validate `proof_backend`, `stark_config_id` or `L`;
   - the `setups` and worker caches are unbounded;
   - LeafData and proof bytes are not committed by the beacon header (same-hash block variants).
