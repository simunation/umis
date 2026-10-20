# Simunation POC: implementation plan

## Context
This is a greenfield project. `/home/nugaon/prog/simunation` is empty and no Rust toolchain is installed yet; Node 24 and Docker are present.

**Goal:** one Rust P2P client that runs natively and in the browser (wasm) from the same code. Its nodes form a **block lattice**:
- **Zones** are mini-chains. Each runs a pluggable simulation task under leader-based minting consensus, with its own slot period.
- Every zone commits validity proofs of its work into a shared **beacon chain**. The beacon chain defines global relative time.
- The first task is a **3D water-current simulation** seeded from a random genesis. The volume is **split spatially across zones**. Zones stream boundary faces to each other **directly over libp2p**, and every output travels with its proof. The task interface must also accept different tasks per zone later (weather, human behaviour, …).

**Decisions confirmed with the user:**
- Zones are a spatial split of the water volume.
- Cross-zone inputs go **directly between zones over libp2p**, together with the producer's proof; they do not wait for the beacon chain. Nodes connect to nodes of linked zones as well as to their own zone.
- **Inputs are usable immediately.** Proof checks run in parallel. On failure, the affected zones roll back to the state committed in an earlier beacon block, and the faulty producer is removed from its zone and has its stake slashed on the beacon chain. Stake and slashing are therefore part of the POC.
- Browser↔browser traffic goes through a libp2p relay first; direct WebRTC comes later.
- Plonky3 STARK proofs on every node, with a mock backend built first.
- **Zones can be created at runtime** at a free location of the zone topology, i.e. a point plus a layer (see §1 "Zone creation"):
  - the proposal is registered on the beacon chain;
  - the nodes of every adjacent zone vote on their own zone chain, and all adjacent zones must accept;
  - each zone's decision travels to the beacon chain in its commitments, where the final outcome is recorded and the new zone is activated.
- **Several zones can share one spatial point, one per layer.** A layer is a kind of model, e.g. `water` or a future `atmosphere`. The code is generic over layers and couplings between layers. The POC uses only the `water` layer.

### Answer to "UDP/QUIC, or better alternatives?"
- **Browsers cannot open raw UDP or QUIC sockets.** The only UDP-based browser transports are:
  - **WebRTC data channels** (SCTP/DTLS over UDP, with ICE NAT traversal);
  - **WebTransport** (QUIC/HTTP3, client→server only). rust-libp2p has no native WebTransport listener, so we do not use it.
- **Resulting stack:**
  - native↔native: **QUIC**, with DCUtR hole punching;
  - browser→native: **WebRTC-direct** (UDP, no CA certificate needed);
  - browser↔browser: **circuit relay v2** for now. rust-libp2p 0.57 lacks private-to-private `/webrtc`, and PR #5978 is unmerged with known bugs. It goes behind a cargo feature later.
- **Alternatives considered:**
  - **iroh 1.x:** great QUIC and hole punching natively, but browsers are relay-only over WebSocket.
  - **matchbox:** real browser↔browser WebRTC with a configurable signalling server, but no gossip, discovery or request-response.
  - **Conclusion:** libp2p is still the best fit. It is the only stack with gossipsub, request-response, rendezvous and relay working on both native and wasm.

## Notation (read first)
Protocol time is counted in integer **ticks** since genesis. Wall-clock values are in milliseconds. $H(\cdot)$ is keccak256 and $\Vert$ is byte concatenation. Every symbol used later in this document is defined here.

There are only two kinds of time window:
- a **beacon slot**, $B$ ticks long, numbered $e$;
- a **zone slot**, $S_z$ ticks long, numbered $j$.

A slot is the fixed time window in which one elected node may produce one block, of the beacon chain or of zone $z$. There is no separate "epoch" concept.

### Time and slots
| Symbol | Meaning | POC default |
|---|---|---|
| $G$ | Genesis time: the wall-clock moment (Unix ms) at which tick 0 starts. Stored in the genesis file. | set by `umis-cli genesis` |
| $tick_{ms}$ | Length of one tick in ms, the smallest unit of protocol time. | 250 ms |
| $\mathit{now}$ | A node's local wall-clock time, in Unix ms. | – |
| $\tau$ (tau) | Current tick index: $\tau = \lfloor (\mathit{now} - G) / tick_{ms} \rfloor$. | – |
| $B$ | **Beacon slot length** in ticks, i.e. the beacon chain's block interval $tb_{e+1} - tb_e$. A **beacon slot** is one fixed window of $B$ ticks; at most one beacon block is produced in it, by that slot's elected proposer. $B$ is a length; the slot's number is $e$. | 16 ticks (4 s) |
| $e$ | **Beacon slot index**: the beacon slot a tick falls in, $e = \lfloor \tau / B \rfloor$. Slot $e$ covers ticks $[e \cdot B,\ (e+1) \cdot B)$. The block produced in slot $e$ is "beacon block $e$". | – |
| $e_{now}$ | The beacon slot a node is currently in, $\lfloor \tau / B \rfloor$. | – |
| $e_b$ | The beacon slot of the beacon block that includes a given transaction or commitment. | – |
| $t_e$ | Wall-clock start of beacon slot $e$: $G + e \cdot B \cdot tick_{ms}$. | – |
| $tb_e$ | Beacon time at the start of beacon slot $e$, in ticks: $e \cdot B$. These are the $tb_0$, $tb_1$ of the original request. | – |
| $z$ | Zone id: 0, 1, 2, … Genesis zones are numbered first; each zone created later gets the next unused id. | genesis zones 0–3 |
| $S_z$ | **Slot length of zone $z$** in ticks. One zone slot is one step of that zone's work, computed by one minter. Genesis requires $S_z$ to divide $B$ or $B$ to divide $S_z$. | 4, 8, 4, 16 ticks (1 s, 2 s, 1 s, 4 s) |
| $j$ | Zone slot index within a zone, counted from genesis. Slot $j$ of zone $z$ covers ticks $[j \cdot S_z,\ (j+1) \cdot S_z)$. | – |
| $tz_j$ | Zone time at the start of slot $j$, in ticks: $j \cdot S_z$. These are the $tz_0$, $tz_1$ of the original request. | – |
| $k_z$ | For a **dense** zone ($S_z \le B$): the number of zone slots per beacon slot, $B / S_z$. | 4, 2, 4, 1 |
| $m_z$ | For a **sparse** zone ($S_z > B$): the number of beacon slots per zone slot, $S_z / B$. | 2 (test variant $S_z = 32$) |
| $P_z$ | Commit period of zone $z$: how many beacon slots one commitment covers. A dense zone ($S_z \le B$) commits every beacon slot, so $P_z = 1$. A sparse zone ($S_z > B$) commits once per zone slot, so $P_z = m_z = S_z / B$. Example: $S_z = 1$, $B = 3$ gives $P_z = 1$; $S_z = 32$, $B = 16$ gives $P_z = 2$. | – |
| $n_z$ | Zone slots covered by one commitment, which is also the number of proof leaves in its BMT. Dense: $n_z = k_z = B / S_z$. Sparse: $n_z = 1$. Example: $S_z = 1$, $B = 3$ gives $n_z = 3$; $S_z = 32$, $B = 16$ gives $n_z = 1$. | – |
| $j^*$ | The zone slot boundary a commitment for beacon slot $e$ ends at: $j^* = (e+1) \cdot B / S_z$. | – |
| $X_z(j)$ | State of zone $z$ at the start of slot $j$, i.e. after $j$ steps. $X_z(0)$ is the genesis state. Written $X(j)$ when the zone is clear from context. | – |
| $dt_{tick}$ | Simulated (physical water) time per tick. | genesis parameter |
| $k_{sub}$ | Fixed number of simulation substeps per zone slot, a power of two. | 4 |
| $\Delta t$ | Physical time of one substep: $S_z \cdot dt_{tick} / k_{sub}$. | – |

### Consensus parameters
| Symbol | Meaning | POC default |
|---|---|---|
| $D$ | **Anchor depth**: how many beacon slots back a node looks to find its reference beacon block, the anchor. It must satisfy $D \ge K_f$. See "Anchors" in §1. | 2 |
| $\mathrm{anchor}(e)$ | The **anchor** of beacon slot $e$: the newest canonical beacon block from slot $e - D$ or earlier. It supplies the seed and the zone member lists that decide minters and proposers during beacon slot $e$. Example with $D = 2$: in beacon slot 10 the anchor is block 8, or block 7 if slot 8 had no block. | – |
| $a(j)$ | The anchor used by zone slot $j$, i.e. the anchor of the beacon slot that zone slot $j$ starts in: $\mathrm{anchor}(\lfloor j \cdot S_z / B \rfloor)$. | – |
| $K_f$ | **Finality depth**: a beacon block is final once it has $K_f$ descendants. Requiring $D \ge K_f$ makes every anchor final. | 2 |
| $r$ | A rank: a node's position in a sorted rank list, where 0 is the primary. | – |
| $R$ | Highest fallback minter rank in a zone slot, so ranks $0 \ldots R$ may mint. | 2 |
| $R_b$ | Highest fallback beacon-proposer rank. | 2 |
| $W_z$ | **Zone fallback window** in ms: $W_z = S_z \cdot tick_{ms} / (R+2)$. Rank $r$ may mint only after $r \cdot W_z$ ms into the slot, and only if no valid block has been seen yet. The slot is cut into $R+2$ equal windows: $R+1$ are start times for ranks $0 \ldots R$, and the extra one, together with the last rank's own window, gives the last fallback (rank R) time to compute its block and broadcast it before the next slot starts. Example: $S_z = 4$ (1000 ms), $R = 2$ gives $W_z = 250\,\mathrm{ms}$; ranks start at 0, 250 and 500 ms, leaving 500 ms for rank 2. | 250 ms when $S_z = 4$ |
| $W_b$ | **Beacon fallback window** in ms. The same rule as $W_z$, applied to the beacon slot after a quarter-slot pause (during which zone commitments from the previous beacon slot arrive): $W_b = (\tfrac{3}{4} \cdot B \cdot tick_{ms}) / (R_b+2)$. Rank $r$ proposes at $t_e + B \cdot tick_{ms} / 4 + r \cdot W_b$. Example: $B = 16$ (4000 ms), $R_b = 2$ gives $W_b = 750\,\mathrm{ms}$; ranks start at 1000, 1750 and 2500 ms, leaving 1500 ms for rank 2. | 750 ms |
| $L$ | **Proof lag** in beacon slots: how far a ProofCommitment may trail its ZoneCommitment on the beacon chain. | 0 (mock), 1 (Plonky3) |
| $\delta$ (delta) | Clock tolerance: messages dated more than $\delta$ ms in the future are buffered. | 500 ms |
| $E_{hb}$ | **Heartbeat timeout**, counted in beacon slots, not seconds. Every member sends one small signed `Heartbeat` tx per beacon slot to show it is online. A node with no heartbeat included for $E_{hb}$ consecutive beacon slots is removed from membership, without slashing. In seconds that is $E_{hb} \cdot B \cdot tick_{ms}$; with the defaults, $10 \cdot 4\,\mathrm{s} = 40\,\mathrm{s}$. | 10 beacon slots (40 s) |
| `genesis.seed` | 32 random bytes in the genesis file. All random genesis parameters (the current field, tides, dye sources) and the first beacon seed are derived from it. | `umis-cli genesis --seed random` |
| $seed_e$ | Beacon randomness carried by the beacon block of slot $e$: $H(\mathit{parent.seed} \Vert \mathit{sig}_{proposer}(\text{"seed"} \Vert e))$. $\mathrm{seed}(a)$ means the seed stored in beacon block $a$. | – |
| `node_id` | A node's identity: its ed25519 public key, which is the same key as its libp2p PeerId. | – |
| $\mathrm{members}_z(a)$ | The members of zone $z$ according to the beacon state at block $a$. | – |
| `genesis8` | First 8 hex characters of the genesis hash. It prefixes gossip topics and rendezvous namespaces. | – |

### Cross-zone inputs
| Symbol | Meaning |
|---|---|
| $z_p$, $z_c$ | The **producer** zone, which sends an output, and the **consumer** zone, which uses it as an input. |
| $S_p$, $S_c$ | Slot lengths of the producer and consumer zones ($S_{z_p}$, $S_{z_c}$). |
| $j_p$, $j_c$ | A producer zone slot and a consumer zone slot. |

### Faults, rollback and stake (§1 "Faults, rollback and slashing")
| Symbol | Meaning | POC default |
|---|---|---|
| optimistic / proven / settled / invalid | The four states of a ZoneCommitment on the beacon chain. **Optimistic**: included, not yet proven. **Proven**: its ProofCommitment was accepted. **Settled**: proven, and every commitment it took inputs from is settled too; a settled commitment can never be rolled back. **Invalid**: discarded by a rollback. | – |
| `deps` | The dependencies of a ZoneCommitment: for each input link, the newest producer block it consumed (`src_zone`, `src_slot`, `src_block_hash`). Because inputs are monotonic, this one entry per link says exactly which producer blocks the commitment depends on. | – |
| $e_{fault}$ | The beacon slot of the block that recorded a fault or a missed proof deadline. | – |
| $j_r$ | The first zone slot that a zone recomputes after a rollback: the first slot after its rollback checkpoint. | – |
| $\mathrm{stake}(n)$ | The stake locked by node n on the beacon chain, in devnet test units. | – |
| $\mathit{min\_stake}$ | The minimum stake a node must lock to become a zone member. | 100 |
| $\mathit{slash\_frac}$ | The fraction of a faulty node's stake taken away for a proven fault. | 100 % |
| $\mathit{reward\_frac}$ | The share of the slashed amount paid to the node that reported the fault. The rest is burned. | 10 % |
| $U$ | **Unbonding delay** in beacon slots: after a node leaves, its stake stays locked, and slashable, for $U$ beacon slots. This is longer than any rollback window, so a node cannot escape punishment by leaving. | 20 |

### Zone creation (§2 "Zone creation")
| Symbol | Meaning | POC default |
|---|---|---|
| point | An **attachment point**: one position in the zone topology where a zone can live. For water3d it is a lattice coordinate $(p_x, p_y, p_z)$, meaning one block of $n_{loc}^3$ cells. A point can hold several zones, at most one per layer. | – |
| layer | One kind of model that can occupy a point, e.g. `water` or a future `atmosphere`. Every task declares the layer it occupies. A point holds **at most one zone per layer**: zones of different layers can share a point, but two water zones never can. Genesis lists the allowed layers. | only `water` in the POC |
| location | One layer at one point, written $(\text{point}, \text{layer})$. A zone's full address in the topology. **One point can hold many zones, one in each of its layers.** A location is a single layer of a point, so it holds one zone at most. Occupancy, locking and the zone registry are keyed by location, never by point alone. Example: $((1,1,0), \text{water})$ and $((1,1,0), \text{atmosphere})$ are two different locations at the same point and can hold two different zones. | – |
| coupling | A genesis rule saying which ports of two different layers connect when their zones share a point, e.g. a future `water.top ↔ atmosphere.bottom`. Without a coupling, co-located zones of different layers are independent and not adjacent. | none in the POC |
| $A$ | The **adjacent zones** of a proposal: the active zones that would be linked to the new zone. There are two kinds: (1) zones of the **same layer on neighbouring points**, which for water3d are the face neighbours, up to 6; (2) zones of **other layers on the same point**, if a coupling connects them. Every zone in $A$ must accept. | – |
| $\mathrm{voters}_a$ | The voters of adjacent zone $a$: a snapshot of $a$'s member list taken in the beacon block that registered the proposal. Members who join later do not vote on it. | – |
| $q_{vote}$ | Approval threshold inside one adjacent zone. The zone accepts once at least two thirds of $\mathrm{voters}_a$ voted yes. It rejects as soon as so many voted no that two thirds can no longer be reached. Example: 4 voters → 3 yes accept; 2 no reject. | 2/3 |
| $e_{reg}$ | The beacon slot of the block that registered the proposal. | – |
| $V$ | **Voting window** in beacon slots. Voting closes at $e_{reg} + V + \max P_a$, where $\max P_a$ is the longest commit period among the adjacent zones, so sparse zones have time to commit their decision. | 10 (40 s) |
| $e_{dec}$ | The beacon slot of the block where the final outcome was recorded: the block holding the last accept, or the first reject. | – |
| $S_{new}$, $P_{new}$ | Slot length and commit period of the proposed zone. | – |
| $e_{act}$ | **Activation slot**: the beacon slot in which the new zone starts. It is the first beacon slot $\ge e_{dec} + D$ that is a multiple of $P_{new}$. Waiting $D$ slots puts the decision into everyone's anchor. | – |
| $j_{start}$ | The new zone's first zone slot: $e_{act} \cdot B / S_{new}$. | – |

### Water simulation (§3)
| Symbol | Meaning | POC default |
|---|---|---|
| $n_{loc}$ | Cells per axis in one zone, so a zone has $n_{loc}^3$ cells. | 16 (4096 cells) |
| $Z_x, Z_y, Z_z$ | Number of **genesis** zones along each axis. They occupy the points $(0..Z_x,\ 0..Z_y,\ 0..Z_z)$. | 2, 2, 1 |
| lattice bounds | The largest point coordinates any zone may occupy, set in genesis. New zones may be created anywhere inside the bounds. | $8 \times 8 \times 2$ points |
| $x$ | A cell position in global grid coordinates. | – |
| $\bar{U}(x)$ | Background current velocity at x, fixed by genesis. | – |
| $N_v$ | Number of random vortex tubes and jets that build $\bar{U}$. | 8 |
| $v$ | Water velocity in a cell (3 components), part of the evolving state. | – |
| $c$ | Dye (tracer) concentration in a cell, the 4th state value. | – |
| $q$, $q'$ | Any one of a cell's 4 state values, before and after a substep. | – |
| $\nu$ (nu), $\kappa$ (kappa) | Diffusion coefficients of velocity (viscosity) and of dye. | genesis |
| $\lambda$ (lambda) | Rate at which velocity relaxes toward the forced current. | genesis |
| $\mu$ (mu) | Dye decay rate. | genesis |
| $\mathrm{src}(x)$ | Dye source strength at x. | genesis |
| $\mathrm{tide}(j)$ | Tide factor for zone slot $j$. It is an integer table with a random phase and period from genesis, and it modulates the forcing. | – |
| $w_k(x)$ | Integer stencil weight for offset $k$ at cell $x$. There are 7 offsets: the cell itself and its 6 neighbours. | – |
| $F(x)$ | Integer forcing shape at x: $\lambda \cdot \bar{U}(x)$ for velocity, $\mathrm{src}(x)$ for dye. | – |
| $b(x,j)$ | Forcing term during slot $j$: $\mathrm{tide}(j) \cdot F(x)$. | – |
| $s$ | Fixed-point shift: each update divides by $2^s$ and rounds down. | 12 |

### Proof (AIR) symbols (§4)
| Symbol | Meaning |
|---|---|
| $t$ | Substep index inside one zone slot, $0 \ldots k_{sub}-1$. |
| $i$ | Cell index inside a zone, $0 \ldots n_{loc}^3-1$. |
| $d$ | A direction: one of $\pm x, \pm y, \pm z$. |
| $\mathrm{nbr}_d(i)$ | Index of cell i's neighbour in direction d. |
| $\mathrm{rem}$ | Remainder of the floor division by $2^s$, range-checked to $[0, 2^s)$. |
| $\mathrm{mult}$ | Read multiplicity: how many rows read a given $(t, i)$ value (at most 7). |
| $X_{in}$, $X_{out}$ | Zone state at the start and end of the proven slot, i.e. $X_z(j)$ and $X_z(j+1)$. |
| $p$ | Prime modulus of the Plonky3 field. |

## Architecture overview
```
            ┌───── Beacon chain (one block per beacon slot, B = 16 ticks = 4 s) ─────┐
 beacon blk │ seed, membership txs, heartbeats,                                     │
    (slot e)│ ZoneCommitments{state_root,outputs_root,inputs_root},                 │
            │ ProofCommitments{proof_bmt_root,n_z}, zones_root, proofs_root         │
            └───────▲───────────────▲───────────────▲───────────────▲──────────────┘
                    │ commit/slot   │               │               │
   zone0 S_z=4 ■■■■ zone1 S_z=8 ■ ■ zone2 S_z=4 ■■■■ zone3 S_z=16 ■  (sparse S_z=32: m_z=2)
   k_z=4 proofs→BMT k_z=2 →BMT      k_z=4 →BMT       k_z=1
   each zone slot: rank-0 minter steps the sim; helpers prove slots in parallel
   halo faces ⇄ streamed directly between neighbour-zone nodes over libp2p, each with its proof;
   zone blocks record which producer block every input came from (InputRef); beacon cross-checks
```
- **There are two data paths.**
  - The **beacon chain is the control plane.** It carries global time, randomness (the seed), membership and leader schedules (through anchors), and per-beacon-slot commitments that finalize zone chains.
  - **Cross-zone data flows directly over libp2p.**
    - A zone's nodes also connect to nodes of every zone it is linked to.
    - Producers push each output together with the producing block's header and its validity proof.
    - Each consuming zone block records exactly which producer blocks it consumed (`InputRef`s).
- **A zone's state is a deterministic function of (genesis, its own canonical zone chain).** Zones therefore have real chains (parent hash and fork choice), which beacon commitments finalize.
- **Cross-link check:** when a consumer commits, the beacon chain verifies that every `InputRef` lies on that producer's committed chain.
- **Optimistic execution with rollback:** inputs are used on arrival and verified in parallel. A failed check rolls the affected zones back to an earlier committed state, and a provable fault slashes the producer.
- **The zone set is not fixed.** Genesis defines the first zones. New zones are created at free locations (point + layer) through the vote-and-activate flow in §1 "Zone creation", and the beacon state holds the authoritative zone registry.
- **The core is sans-IO**, written as `Node::handle(now_ms, Event) -> Vec<Action>`. libp2p, wasm, the clock and compute workers are thin adapters around it. The same core runs in a **deterministic simulated network** (SimNet) before any real networking exists.

## Workspace layout (Cargo workspace, edition 2024, pinned stable ≥ 1.88)
```
Cargo.toml  rust-toolchain.toml  .cargo/config.toml  genesis/devnet.json
crates/
  umis-core      ids, `seg` codec (segment-aligned, for everything committed), borsh (transport only),
                 keccak256, `bmt` (Rust port of bmt-js 3.0.2: chunk + chunked-file
                 BMT, carrier chunks, inclusion proofs), tick/slot arithmetic,
                 genesis (load/generate), ed25519 signing (same 32-byte key as libp2p identity)
  umis-task      ZoneTask trait, TaskRegistry, port/link model, layers/locations/couplings + Topology,
                 fixed-point lib `Fx`
  task-water3d   water current task: genesis field gen, step, halo/ports; `air` module (feature "stark")
  umis-proof     ProofBackend trait, TransitionStatement, MockBackend, proof-BMT assembly
  umis-stark     shared Plonky3 config (field/hash/FRI), LogUp memory + range-check helpers
  umis-protocol  sans-IO node: beacon state transition, zone engine, validation, ranks,
                 finality, membership/heartbeats, proof jobs, commitments,
                 zone registry + zone creation (proposals, votes, decisions, activation)
  umis-simnet    deterministic multi-node harness: simulated net/clock, scenarios, oracle, invariants
  umis-net       libp2p adapter: transports per role/target, behaviours, codecs, topics, rendezvous
  umis-node      runtime: drives umis-protocol with net + clock + compute executor (tokio | wasm)
  umis-cli       native bin: `genesis`, `keygen`, `node`, `bootstrap` (relay+rendezvous+http), `sim`
  umis-web       wasm cdylib (wasm-bindgen): `SimNode` class (many per page), prover Web Worker
web/             Vite + TypeScript UI: node cards, lattice explorer, three.js 3D current view
scripts/         devnet.sh (bootstrap + N native nodes), check.sh (native + wasm32 check/clippy/test)
```

## 1. Protocol spec (umis-core / umis-protocol)

### Time (all integer ticks since genesis; symbols are defined in "Notation")
- **Basic quantities:** the current tick is $\tau = \lfloor (\mathit{now} - G) / tick_{ms} \rfloor$ and the current beacon slot is $e = \lfloor \tau / B \rfloor$. Zone $z$ slot $j$ covers ticks $[j \cdot S_z,\ (j+1) \cdot S_z)$.
- **Why the beacon slot exists:** it is the beacon chain's block interval and the network's shared time grid (the "relative time that the beacon chain prescribes"). It does four jobs:
  1. **Block schedule.** Each beacon slot $e$ has one elected proposer, with fallbacks. Fixed slots let every node work out whose turn it is from the clock alone, without a voting round.
  2. **Synchronization points.** Zone slots are aligned to it ($S_z$ divides $B$, or $B$ divides $S_z$). At every $tb_e = e \cdot B$, all zones have therefore simulated exactly the same physical time.
  3. **Commitment cadence.** A zone commits once per beacon slot (a sparse zone every $m_z$ beacon slots). The $k_z$ zone-slot proofs inside one beacon slot form the zone's proof BMT. This is the request's "zone time denser than the beacon time" case, and it is why the work between $tb_e$ and $tb_{e+1}$ is provable.
  4. **Unit for protocol delays.** Anchor depth $D$, finality depth $K_f$, proof lag $L$ and heartbeat timeout $E_{hb}$ are all counted in beacon slots.
- **Slot ratios:** genesis requires $B \bmod S_z = 0$ (a dense zone, with $k_z = B / S_z$ zone slots per beacon slot) or $S_z \bmod B = 0$ (a sparse zone, where one slot spans $m_z = S_z / B$ beacon slots).
- **Commit period:**
  - dense zone: $P_z = 1$ and $n_z = k_z = B / S_z$;
  - sparse zone: $P_z = m_z = S_z / B$ and $n_z = 1$.
  - Beacon slot $e$ is a commit slot for zone $z$ iff $(e+1) \bmod P_z = 0$, so a dense zone commits in every beacon slot. Its commitment carries $X_z(j^*)$, with $j^* = (e+1) \cdot B / S_z$.
- **Physical time:** each zone slot advances simulated time by $S_z \cdot dt_{tick}$, in $k_{sub}$ substeps of $\Delta t$ each. All zones therefore sit at the same physical time at every commit boundary.
- **Anchors (the reference beacon block):**
  - **What it provides.** To decide who mints a zone slot or proposes a beacon block, every node needs the same **seed** (which sets the rank order) and the same **member list** of each zone. Both live in beacon blocks. The anchor is the beacon block all nodes take them from.
  - **The rule.** During beacon slot $e$, everything uses $\mathrm{anchor}(e)$, the newest canonical beacon block from slot $e - D$ or earlier. It stays fixed for the whole beacon slot. Zone slot $j$ uses $a(j) = \mathrm{anchor}(\lfloor j \cdot S_z / B \rfloor)$, the anchor of the beacon slot it starts in.
  - **Example** ($D = 2$): in beacon slot 10 the anchor is block 8. If slot 8 produced no block, it is block 7.
  - **Why not the newest beacon block?**
    1. **It might still change.** A fresh beacon block can be replaced by a competing block until it is final, which takes $K_f$ more blocks. Nodes using different versions would disagree about who the minter is. Requiring $D \ge K_f$ makes every anchor final, so it can never change.
    2. **Everyone has it in time.** A block $D$ slots old (8 s with the defaults) has reached every node, including slow browser tabs. Each node can compute the minter and proposer schedule for the coming beacon slot in advance.
  - **Trade-off.** A larger $D$ is safer but slower to react: a join, a removal or a new seed only takes effect $D$ beacon slots after it lands on the beacon chain. With the defaults that is about 8 s.
- **Cross-zone input selection** (the data plane is described in §1 "Cross-zone data plane"). Inputs are **optimistic**: an output received from a neighbour zone over libp2p is **usable immediately**. For each input link, the minter of consumer slot $j_c$ picks the freshest producer block, from producer slot $j_p$, that passes these cheap checks:
  - **legitimate:** the producer header is signed by the producer zone's elected minter for slot $j_p$ (according to the producer's anchor), and the payload matches its inclusion proof under `outputs_root`;
  - **causal:** $(j_p+1) \cdot S_p \le j_c \cdot S_c$, so the consumer never uses a producer state from after the start of its own step;
  - **monotonic:** it is not older than the last input consumed on that link.
- **Deeper checks run in parallel:**
  - the producer block's proof or re-execution;
  - whether it stays on the producer's committed chain;
  - equivocation.

  If one fails, the affected zones roll back, and a faulty producer is removed and slashed (§1 "Faults, rollback and slashing").
- **Fallback when nothing newer is acceptable:** the minter reuses the last consumed input ("hold last value"). A dead or slow neighbour therefore never stalls the consumer. Before any input exists on a link (e.g. right after a neighbour zone is created), the task's empty-neighbour boundary is used; for water3d that is still water.
- **Typical staleness:** about one producer slot plus network delay. Proving does not delay inputs.
- **Clock:** the local clock only decides *when to act*. Messages more than $\delta$ in the future are buffered, and a late but valid message is still valid. NTP-lite offset estimation (`time` request-response) asks native peers only.
- **POC defaults**, matching the chosen layout:
  - $tick_{ms} = 250$; $B = 16$ (4 s);
  - zones 0–3 use $S_z = \{4, 8, 4, 16\}$ (1 s, 2 s, 1 s, 4 s), giving $k_z = \{4, 2, 4, 1\}$;
  - $D = K_f = 2$ beacon slots (8 s), $R = R_b = 2$ fallback ranks, $\delta = 500\,\mathrm{ms}$, $E_{hb} = 10$ beacon slots (40 s);
  - $\mathit{min\_stake} = 100$ test units, $\mathit{slash\_frac} = 100\%$, $\mathit{reward\_frac} = 10\%$, $U = 20$ beacon slots (80 s);
  - $L = 0$ beacon slots for the mock backend and $L = 1$ beacon slot (4 s) for Plonky3;
  - a sparse variant, a zone with $S_z = 32$ ($m_z = 2$, one commit every 2 beacon slots), is covered in tests and SimNet.

### Ranks, roles and timelines
- **Zone rank:** sort $\mathrm{members}_z(a(j))$ by ascending $H(\mathrm{seed}(a(j)) \Vert \text{"zone"} \Vert z \Vert j \Vert \mathit{node\_id})$. Native full nodes are appended as a tail of fallback minters so a zone that empties keeps going. Rank $r$ (with $r \le R$) may mint only after $r \cdot W_z$ ms into the slot, if no valid block has been seen.
- **Beacon proposer:** sort all members by ascending $H(\mathrm{seed}(\mathrm{anchor}(e)) \Vert \text{"beacon"} \Vert e \Vert \mathit{node\_id})$. Rank 0 proposes at $t_e + B \cdot tick_{ms} / 4$, and rank $r$ (with $r \le R_b$) at $t_e + B \cdot tick_{ms} / 4 + r \cdot W_b$.
- **Seed:** $seed_e = H(\mathit{parent.seed} \Vert \mathit{sig}_{proposer}(\text{"seed"} \Vert e))$. A proposer can grind it, which is accepted for the POC; a VRF comes later.
- **Zone node in slot $j$:**
  - rank 0 selects its inputs, computes $X(j+1)$ (the "real work"), and gossips a ZoneBlock with the state blob to its own zone;
  - it publishes an **OutputBundle** (header, port payloads, proof once ready) on `zone/<z>/out` for consuming zones;
  - members validate the block, by re-execution in mock mode, or optimistically now and by proof verification later.
- **Commit slots:** the last zone slot inside a commit slot also produces a **ZoneCommitment**:
  - it is sent by rank 0, who is the user's "chosen zone node for the beacon chain";
  - any member may submit it after a rank timeout;
  - the first valid one wins, so the delegate is not a single point of failure.
- **Helper proving:**
  - Each slot's transition proof has a priority prover: the member with the lowest $H(\mathrm{seed}(a(j)) \Vert \text{"prove"} \Vert z \Vert j \Vert \mathit{node\_id})$ among `can_prove` members, excluding that slot's minter when possible.
  - Anyone may submit a valid **ProofShare**. In a dense zone, $k_z$ helpers therefore prove in parallel.
  - The commit slot's delegate assembles a **ProofCommitment**: a bmt list root over the $n_z$ proof leaves in slot order (span $= 32 \cdot n_z$).

### Data types (keccak256, ed25519; committed records use the segment-aligned `seg` encoding described below)
- **`ZoneBlock`:** a `ZoneBlockHeader` `{zone, slot, parent, anchor_hash, params_hash, prev_state_root, state_root, outputs_root, inputs_commitment = bmt list root over the InputRef addresses in link order, votes_root, minter}`, the minter's `sig` over the header address, and the body: `inputs: Vec<InputRef>`, `votes: Vec<ZoneVote>` and a state blob (≤64 KB). Votes don't change the simulation state; the block only records them, which makes the zone's tally part of its chain.
- **`InputRef`:** `{link_id, src_zone, src_slot, src_block_hash, src_outputs_root, payload_address}`. It points at one port of one producer block, at two levels of the same tree:
  - **`payload_address`:** the bmt file address of the **one port payload** this link consumes, e.g. water3d's 4 KB `+x` face.
  - **`src_outputs_root`:** the producer block's `outputs_root`, the bmt list root over the payload addresses of **all** its output ports, in port order. For water3d that is 6 faces: `+x, −x, +y, −y, +z, −z`.
  - **How they are checked:** a consumer checks that `payload_address` sits at its link's port index under `src_outputs_root`, using a 7-segment chunk inclusion proof. It doesn't need the other ports' data.
  - **Redundancy:** `src_outputs_root` is a copy of the value in the producer's signed header, which `src_block_hash` already commits to, and must equal it. The copy lets anyone check the payload against the `InputRef` alone, before fetching the header (e.g. in `InvalidInputRef` evidence).
- **`OutputBundle`** (gossip on `zone/<z>/out`, and also served by request-response): `{header: ZoneBlockHeader, ports: [(port_index, payload, chunk_inclusion_proof)], proof | proof_pending}`. Each inclusion proof shows the payload address sits at `port_index` under `outputs_root`. When the proof is produced later, it follows as a `ProofShare` on the same topic.
- **`TransitionStatement`:** `{zone, params_hash, from_slot, to_slot, prev_state_root, state_root, inputs_commitment, outputs_root}`; `params_hash` commits to the task id and its params. The proof covers the inputs actually consumed, through `inputs_commitment`.
- **`ZoneCommitment`:** `{zone, beacon_slot, tip_block_hash, state_root, prev_state_root, outputs_root, inputs_root = bmt list root over the InputRef leaves of the covered zone slots, in (zone slot, link) order, deps_root, decisions_root, signer}` plus `sig` over its address. The lists `deps: Vec<Dep>` (one `{link_id, src_zone, src_slot, src_block_hash}` per link) and `decisions: Vec<ZoneDecision>` travel with it in the beacon block body, and their list roots commit to them.
  - `deps` lets the beacon chain track which producer blocks each commitment depends on, so it can compute rollbacks.
  - Outputs are no longer inlined into the beacon chain; they travel peer to peer.
  - Its gossip envelope on `commit` carries the state blob, which is the checkpoint. It also carries the signed votes behind every decision, so beacon validators can recount them.
- **`ProposeZone`** (beacon tx): `{point, layer, task_id, params, slot_ticks = S_new, founders: Vec<(node_id, founder_sig)>, proposer, sig}`. `layer` must be the layer the task declares.
  - Founders are registered nodes that agree to move into the new zone and run it; each signs the proposal. At least 1 is required.
  - `proposal_id` is the `seg` address of the ProposeZone record.
- **`ZoneVote`** (gossip on `zone/<a>/vote`, recorded in zone a's blocks): `{proposal_id, zone = a, accept: bool, voter, sig}`.
- **`ZoneDecision`** (inside a ZoneCommitment): `{proposal_id, accept: bool, yes, no, votes_root}`. `votes_root` is the bmt list root over the counted votes in voter order.
- **`FaultEvidence`** (beacon tx): `{kind, reporter, sig}`, where `kind` is one of:
  - `InvalidTransition{signed_block, parent_checkpoint, input_payloads}`;
  - `InvalidInputRef{signed_block, input_index}`;
  - `Equivocation{header_a, header_b}`.

  See §1 "Faults, rollback and slashing".
- **`Grant`** (beacon tx, devnet only): `{to, amount, faucet_sig}`. It credits test stake and must be signed by the faucet key named in genesis.
- **`ProofCommitment`:** `{zone, beacon_slot, proof_bmt_root, leaf_count = n_z}`. The leaves must chain (`stmt[i].state_root == stmt[i+1].prev_state_root`) and their ends must match consecutive commitments.
- **BMT: identical to [bmt-js](https://github.com/nugaon/bmt-js)** (v3.0.2, commit `21ed3d0`), i.e. Swarm's chunk BMT. `umis-core::bmt` is a line-by-line Rust port, with conformance tests against the JS library.
  - **Chunk:**
    - the payload is ≤ 4096 bytes, zero-padded to 4096, and split into 128 segments of 32 bytes;
    - each level hashes adjacent pairs with $\mathrm{keccak256}(\mathit{left} \Vert \mathit{right})$ (no domain-separation bytes) until 7 levels give the 32-byte BMT root;
    - the span is 8 bytes little-endian and holds the payload byte length, limited to $< 2^{32}$ as in bmt-js;
    - **chunk address** = $\mathrm{keccak256}(\mathit{span} \Vert \mathit{bmt\_root})$.
    - Both `maxPayloadSize` and `spanLength` stay configurable, as in bmt-js, but every simunation root uses the defaults 4096 and 8.
  - **Chunked file** (payloads over 4 KB):
    - the payload is split into 4096-byte leaf chunks; an empty payload gives one empty chunk;
    - each intermediate chunk's payload is up to 128 child chunk addresses, and its span is the sum of the child spans;
    - **carrier chunk rule:** if a level has $\mathit{count} \bmod 128 = 1$ (and count > 1), the last chunk is popped and carried up. It is appended at the first higher level where $\mathit{len} \bmod 128 \ne 0$;
    - the root chunk's address is the **file address**.
  - **Inclusion proofs:**
    - within a chunk, a proof is the 7 sister segments, bottom-up (`inclusionProof` / `rootHashFromInclusionProof`);
    - within a file, a proof is a list of `{span, sisterSegments}` per chunk level (`fileInclusionProofBottomUp` / `fileAddressFromInclusionProof` / `getBmtIndexOfSegment`, including the carrier-chunk edge case).
  - **How simunation uses it:**
    - **List roots** (`proof_bmt_root`, `zones_root`, `proofs_root`, `inputs_root`, `outputs_root`): the payload is the concatenation of 32-byte leaf segments in canonical order (slot order for proof leaves; zone id, link id or port index for the others). The root is the address of `makeChunkedFile(payload)`, which is a single chunk while there are $\le 128$ leaves. The span is $32 \cdot \mathit{leaf\_count}$, so the root binds the leaf count.
    - **Leaf segments:** every leaf is the address of a `seg`-encoded record. For example, a proof leaf is the address of a `ProofLeaf` record, which holds the statement fields plus the bmt file address of the proof bytes. Any field of a leaf can therefore be proven with segment proofs alone (see "Commitment encoding").
    - **`state_root`:** the file address of the raw, fixed-layout state blob, which has no borsh length prefixes. For water3d that is 64 KiB, i.e. 16 leaf chunks plus 1 intermediate chunk.
    - **Port payloads:** each payload has its own file address. `outputs_root` is the list root over those addresses in port order, so a consumer verifies a port by recomputing the payload address and checking its chunk inclusion proof under `outputs_root`.
  - **Swarm compatibility:** every root is computed exactly like an unencrypted Swarm `/bytes` reference, so checkpoints and proof bundles can later be stored on Swarm under the same addresses.
- **`BeaconBlock`:** `{slot, parent, proposer, seed, txs[Join|Leave|Heartbeat|ProposeZone|FaultEvidence|Grant], zone_commitments[], proof_commitments[], zones_root, proofs_root, state_root, sig}`.
  - `zones_root` and `proofs_root` are BMTs ordered by zone id. Together with each zone's proof BMT they form the lattice commitment for the beacon interval $[tb_e, tb_{e+1})$.
  - **Beacon state:**
    - the **zone registry**, keyed by location: for each zone, its location $(\text{point}, \text{layer})$, `task_id`, `params_hash`, $S_z$, links and `active_from` beacon slot;
    - the member registry with zone assignment;
    - `last_commit[z]`, `last_proven[z]`, `last_heartbeat`;
    - for every recent ZoneCommitment, its status (optimistic / proven / settled / invalid) and its `deps`, kept back to the last settled commitment of each zone;
    - $\mathrm{stake}(n)$, locked or unbonding;
    - the keys of faults already punished, `(minter, zone, zone slot)`, kept only for $U$ beacon slots, to stop double slashing;
    - rollback records `{zone, checkpoint commitment, j_r, e_fault}`;
    - **proposals**: `proposal_id → {location, A, voters_a per adjacent zone, e_reg, deadline, decisions so far, status: Voting | Accepted{e_act} | Rejected | Expired}`;
    - `next_zone_id`.
  - **Joins:** a Join locks at least $\mathit{min\_stake}$ of the node's stake.
    - Devnet stake comes from a **faucet**: genesis names a faucet key, and the bootstrap node offers a faucet endpoint, used by the UI's "get test stake" button, that submits `Grant` txs.
    - Assignment is deterministic: empty zones first, then the least-populated zone, ties broken by hash.
    - Stake has no real value in the POC, and new keys can still be created (Sybil); the faucet rate-limits per IP.
  - **Leave:** membership ends through the anchor rule, and the stake unlocks after $U$ beacon slots.
  - **Effective timing:** a membership change included in the beacon block of slot $e_b$ takes effect for beacon slots whose anchor slot is $\ge e_b$.
  - **Removal:**
    - nodes are removed after $E_{hb}$ beacon slots without a heartbeat, but never the last member of a zone or a genesis validator, and without slashing;
    - a node with a proven fault is removed from its zone and slashed, unconditionally (§1 "Faults, rollback and slashing").

### Commitment encoding (segment-aligned `seg` codec)
Everything that is committed, hashed into a root, signed or inclusion-proven uses the **`seg` encoding**. Its layout follows the 32-byte segments of the BMT, so any single field can be proven with one segment and its sister segments.

**Why not borsh.** Borsh writes fields back to back: little-endian integers, no padding, and a 4-byte length prefix before every `Vec` or `String`. Fields therefore straddle segment boundaries. Example: in `{zone: u32, slot: u64, parent: [u8;32]}`, `parent` occupies bytes 12–44, which spans segments 0 and 1. Proving it takes two segment proofs plus parsing, and a hash like $\mathrm{keccak256}(\mathrm{borsh}(\mathit{record}))$ can only be checked against the full preimage. Borsh stays in use **only for transport**: gossip envelopes, request-response messages, and anything never committed.

**Rules of the `seg` encoding:**
1. **Whole segments.** A record is a sequence of 32-byte segments with a fixed layout per record type. Every field has a fixed segment index and byte offset, and no field ever crosses a segment boundary.
2. **32-byte values get one segment each.** This covers hashes, roots, addresses, `node_id`s (ed25519 public keys) and seeds.
3. **Small scalars are packed together.** Integers, bools and enum tags share a segment, normally segment 0, and the record's type tag and version come first. They are big-endian, so a smart contract reading the segment as a `uint256` finds each field at a fixed bit position.
4. **Canonical.** Unused bytes must be zero, and decoders reject anything else, so every record has exactly one valid encoding.
5. **No inline lists.** A variable-length list (InputRefs, deps, votes, decisions, founders, txs) is never placed inside a record. The record holds the list's **bmt list root** in one segment, and the count in a scalar. The list items are themselves `seg` records, and their addresses are the leaves.
6. **Record address.** A record's address is the bmt chunk address of its encoding, and every record fits in one chunk ($\le 128$ segments). This address replaces every former $\mathrm{keccak256}(\mathrm{borsh}(\mathit{record}))$: it is the leaf in list roots, the block hash, the `proposal_id`, and so on.
7. **Signatures stay outside.** A signature is not part of the record. The signer signs the record's address, and the record's type tag inside it acts as domain separation. The 64-byte signature travels next to the record.
8. **Raw data blobs** (zone state, port payloads, proof bytes) have a fixed element layout with no length prefixes; their length is the bmt span. Element sizes divide 32, so elements never straddle segments: a water3d cell is 4 `i32` values, $4 \cdot 4 = 16$ bytes, i.e. 2 cells per segment. Their root is the bmt file address.

**Layouts.** Segment 0 holds the packed scalars, big-endian, after a 1-byte type tag and a 1-byte version. Each following segment holds one 32-byte value.

| Record | Segment 0 scalars | Segments 1… | Size |
|---|---|---|---|
| `ZoneBlockHeader` (its address is the block hash) | zone u32, slot u64, input_count u16, vote_count u16 | parent, anchor_hash, params_hash, prev_state_root, state_root, outputs_root, inputs_commitment, votes_root, minter | 10 segments |
| `InputRef` | link_id u32, src_zone u32, src_slot u64 | src_block_hash, src_outputs_root, payload_address | 4 |
| `ProofLeaf` (statement + proof) | zone u32, from_slot u64, to_slot u64 | params_hash, prev_state_root, state_root, inputs_commitment, outputs_root, proof_address | 7 |
| `ZoneCommitment` | zone u32, beacon_slot u64, dep_count u16, decision_count u16 | tip_block_hash, prev_state_root, state_root, outputs_root, inputs_root, deps_root, decisions_root, signer | 9 |
| `Dep` | link_id u32, src_zone u32, src_slot u64 | src_block_hash | 2 |
| `ProofCommitment` | zone u32, beacon_slot u64, leaf_count u32 | proof_bmt_root, signer | 3 |
| `ZoneVote` | zone u32, accept u8 | proposal_id, voter | 3 |
| `ZoneDecision` | accept u8, yes u16, no u16 | proposal_id, votes_root | 3 |
| `ProposeZone` | point ($3 \times$ i32), slot_ticks u32, founder_count u16 | layer_id, params_hash, founders_root, proposer | 5 |
| `BeaconBlockHeader` (its address is the beacon block hash) | slot u64, tx_count u32, zc_count u16, pc_count u16 | parent, seed, proposer, txs_root, zones_root, proofs_root, state_root | 8 |

`params_hash` is the address of the zone's `{task_id, layer, params}` record, so `task_id` itself needn't appear in headers or statements. `proof_address` is the bmt file address of the proof bytes.

**What a proof costs.** Proving any field from a record's address takes 7 sister segments plus the 8-byte span, i.e. 232 bytes. Proving a record's address inside a list root of up to 128 leaves costs the same.

Example: prove zone $z$'s `state_root` at beacon block $e$, starting from the beacon block hash. The chain is header → `zones_root` → that zone's `ZoneCommitment` address → its segment 3. That is $3 \times 232 \approx 700$ bytes, with no preimages and no parsing; every value sits whole in its own segment.

**Implementation.** `umis-core::seg` provides:
- a `seg_record!` macro that generates `encode`, `decode`, `address` and a per-field `prove_field` for each record type, plus a const layout table;
- compile-time assertions that no field crosses a segment boundary and that every record fits in one chunk;
- a shortcut when hashing small records: zero-padding subtrees use precomputed zero hashes, so hashing a 10-segment record costs about 10 + 7 keccak calls instead of 127. The result is identical to bmt-js.

### Validation, fork choice, finality
- **ZoneBlock:**
  - `parent` is known (otherwise buffer it and sync) and `prev_state_root == parent.state_root`;
  - $\mathit{anchor\_hash} = a(j)$;
  - the minter rank is within the allowed fallback window and the signature is valid;
  - every `InputRef` passes the cheap checks:
    - the producer header is signed by a legitimate producer minter for `src_slot`, checked against the producer's own anchor;
    - the payload's bmt file address matches `payload_address` and has a valid chunk inclusion proof under `src_outputs_root`;
    - it is causal and monotonic per link.
    
    Its proof and its place on the producer's chain are *not* awaited. They are checked in parallel, and a failure leads to a rollback (§1 "Faults, rollback and slashing").
  - `inputs_commitment` matches;
  - the roots match re-execution or a proof. A mismatch is fault evidence;
  - every `ZoneVote` is valid:
    - the proposal is in `Voting` status according to $a(j)$;
    - the voter is in $\mathrm{voters}_a$ and the signature is valid;
    - it is the voter's first vote on that proposal in this chain.
- **ZoneCommitment:**
  - e is a commit slot for z, and $e < e_b$, where $e_b$ is the slot of the beacon block that includes the commitment;
  - commitments are contiguous ($e = \mathit{last\_commit}.e + P_z$) and link from `prev_state_root`;
  - the blobs hash to their roots and the signer is a zone member or full node;
  - no entry in `deps` points to a producer block that is already invalid. A commitment that does is itself invalid, which makes contamination stop at the beacon chain;
  - **cross-link check, run continuously:** the commitment is accepted optimistically. Each time a producer commitment arrives, every consumer commitment whose `deps` reference that producer range is checked against it. A referenced block that is not on the producer's committed chain (an orphaned input) triggers a rollback of the consumer, without slashing;
  - **every `ZoneDecision`** recounts correctly from the votes in the envelope, all of which lie in the committed range. It reaches $q_{vote}$ for accept, or makes accept impossible for reject, and it is the zone's first decision on that proposal.
- **ProposeZone**, checked when it is included: see the registration rules in §1 "Zone creation".
- **FaultEvidence**: the beacon state transition verifies the evidence itself. It checks the signatures and, for an invalid transition, re-executes that single zone slot. Invalid evidence is dropped. Evidence is accepted only for blocks from the last $U$ beacon slots, and only once per fault key `(minter, zone, zone slot)`, so the same fault is never slashed twice.
- **ProofCommitment:** the matching commitment already exists, $\mathit{leaf\_count} = n_z$, the leaves chain, every proof verifies and the BMT root matches.
- **BeaconBlock** for slot $e$: $\mathit{parent.slot} < e \le e_{now} + 1$, the proposer rank is $\le R_b$, the seed formula holds, txs are valid, and `state_root == apply(parent_state, block)`.
- **Fork choice:** the beacon chain with the most blocks wins. Ties go to the lower proposer rank at the first divergence, then the lower hash.
- **Finality:** `FinalityRule::DepthK(K_f)`, behind a trait so a BFT attestation gadget can replace it later.
- **Zone fork choice:** the longest chain descending from the zone's latest valid commitment wins. Ties go to the lower minter rank, then the lower hash. This only settles short races between fallback minters inside one slot.
- **Rollbacks:** larger reversals, caused by invalid or orphaned inputs, faulty blocks or missing proofs, go through the beacon-driven rollback in §1 "Faults, rollback and slashing". Nothing below a **settled** commitment is ever reverted.

### Messages
- **Gossip topics,** prefixed `/simu/<genesis8>/`:
  - `beacon/block`, `beacon/tx`, `commit` (Zone/ProofCommitment);
  - `zone/<z>/block` and `zone/<z>/proof`, for the zone's own members;
  - `zone/<z>/out`: OutputBundles and their ProofShares, subscribed to by every node of every zone that consumes from z;
  - `zone/<z>/vote`: ZoneVotes from z's voters. The zone's minters include them in blocks.
- **Request-response `/simu/sync/1`** (borsh codec): `Status`, `Time`, `BeaconBlocksBySlot{from,count}`, `BeaconBlockByHash`, `GetCheckpoint{zone, beacon_slot|latest}`, `ZoneBlocks{zone,from,to}`, `ZoneHeaders{zone,from,to}`, `GetOutputs{zone, slot|latest}`, `GetProofs{zone, zone_slot|beacon_slot}`, `GetProposal{proposal_id}`, `GetVotes{zone, proposal_id}`.

### Cross-zone data plane
- **Bridge peer:** every node keeps **one** connection to a node of each zone it consumes from, found through the rendezvous namespace `zone/<src>`, and subscribes to `zone/<src>/out`. Producer-zone nodes accept and serve these consumer peers.
  - One is enough for correctness: every output carries a validity proof, so a bridge peer cannot feed wrong data undetected.
  - If the bridge peer goes quiet or disconnects, the node keeps using the last input ("hold last value") and connects to another producer-zone node from rendezvous.
- **Push:** right after minting, the producer gossips its OutputBundle. The proof follows as soon as a helper finishes it.
- **Pull:** consumers also pull with `GetOutputs` or `GetProofs` if gossip missed something. `ZoneHeaders` lets them follow the producer's chain and detect orphaned inputs early.
- **Payload size:** water3d moves a face of $16 \times 16 \times 4$ values (4 KB) per link per producer slot, which is cheap enough to push every slot.
- **Data retention:** every node keeps each zone's checkpoints back to its last settled commitment (~64 KB each). These are the possible rollback targets, and joiners can sync from anyone.

### Faults, rollback and slashing
Cross-zone inputs are optimistic: a node uses a neighbour's output as soon as it arrives. Checking happens in parallel. When a check fails:
- the affected zones **roll back** to the state committed in an earlier beacon block and recompute;
- a provably faulty producer is **removed from its zone and slashed** on the beacon chain.

Symbols are defined in "Notation → Faults, rollback and stake".

**What runs in parallel with normal minting:**
1. the producer block's proof is verified (Plonky3), or the block is re-executed (mock backend: done by producer-zone members and native full nodes);
2. the cross-link check confirms the block ends up on the producer's committed chain;
3. every node watches for two different signed blocks from the same minter for the same slot.

**Fault kinds:**

| Fault | What happened | Evidence | Punished? |
|---|---|---|---|
| Invalid transition | A minter signed a block whose `state_root` or `outputs_root` is not what `step()` gives from its parent state and recorded inputs | The signed block, the parent checkpoint and the input payloads. The beacon state transition re-executes that one zone slot. With Plonky3, a valid proof of the correct transition that ends at a different root also works. | yes |
| Invalid input reference | A minter recorded an `InputRef` that is not signed by the legitimate producer minter, or that breaks causality or monotonicity | The signed block and the bad `InputRef` | yes |
| Equivocation | A minter signed two different blocks for the same zone slot | The two signed headers | yes |
| Orphaned input | A consumer used a valid producer block that then lost the producer zone's fork choice, e.g. in an honest fallback race | None needed: the cross-link check fails | no, rollback only |
| Missing proof | No valid ProofCommitment arrived within $L$ beacon slots of the ZoneCommitment | None needed: the beacon state sees the deadline pass | no, rollback only |

**Reporting:** any node that detects a punishable fault submits `FaultEvidence` on `beacon/tx`. Nobody has to be trusted, because the beacon chain verifies the evidence itself.

**Punishment**, applied by the beacon block that includes valid evidence (slot $e_{fault}$):
- **Removal:** the faulty node is removed from its zone. There is no ban list: a bad actor could simply create a new key, so slashing is the real deterrent. The same `node_id` may join again, but only by locking a fresh $\mathit{min\_stake}$. The "never remove the last member" rule does not apply: native full nodes take over as fallback minters. The removal takes effect through the anchor rule, from beacon slot $e_{fault} + D$.
- **Slashing:** $\mathit{slash\_frac}$ of its stake is slashed. $\mathit{reward\_frac}$ of that goes to the reporter and the rest is burned. Stake that is still unbonding ($U$) is slashable too.

**Rollback scope.** It is deterministic, and the beacon chain itself is never rewritten.
1. **Faulty zone.** The beacon block at $e_{fault}$ records the bad block. The faulty zone rolls back to its last valid commitment before that block, and every later block in that zone is discarded, because it was built on bad state.
2. **Contamination spreads along the input links.** A block is discarded if its parent is discarded or any of its `InputRef`s points to a discarded block.
   - Every node can evaluate this from the block headers it already has, since headers carry their `InputRef`s; missing headers are fetched with `ZoneHeaders`.
   - On the beacon chain, a commitment whose `deps` reach a discarded block is marked invalid, whether it was already included or arrives later.
3. **Each affected zone** rolls back to its newest commitment from before its first discarded block, i.e. to "the state committed in the previous valid beacon block", and recovers from there. The beacon chain appends a rollback record `{zone, checkpoint commitment, j_r}` for it.

An orphaned input or a missing proof triggers the same steps 2–3 for the zones involved, without punishment.

**Recovery (catching up).**
- From beacon slot $e_{fault} + D$, when the fault record is in everyone's anchor, every zone checks its own chain against the contamination rule above. In each affected zone, the minter of the current slot produces one **recovery block**. All affected zones recover in the same beacon slot.
- The recovery block recomputes every slot from $j_r$ up to the current slot, starting from the checkpoint. On each input link it uses the last valid input from before the checkpoint ("hold last value").
- Every node can compute the same result, so the zone is immediately back at the correct physical time and normal minting continues.
- The recovery block is proven like any other block, with one proof leaf per recomputed slot.
- Blocks minted between $e_{fault}$ and $e_{fault} + D$ on top of discarded state are themselves discarded by the recovery.

**Bound.** A fault is found by the proof deadline at the latest, so a rollback reaches back at most about $L + D + 1$ beacon slots. It never goes past a settled commitment, and $U$ is chosen longer than this window.

**Worked example** (defaults: $B = 16$, $D = 2$, $L = 1$; zone 1 has $S_z = 8$, i.e. 2 zone slots per beacon slot):
1. **Beacon slot 50:** zone 1's minter M signs an invalid block for zone slot 101. Neighbour zones 0 and 3 pick up its faces from their next slots, which start in beacon slot 51.
2. **Detection:** H, an honest member of zone 1, re-executes slot 101, sees the wrong `state_root`, and submits `FaultEvidence`.
3. **Beacon block 51** ($e_{fault} = 51$): M is slashed and removed from zone 1. Zone 1's rollback point is its commitment for beacon slot 49, i.e. state $X_1(100)$, so $j_r = 100$.
4. **Contamination:** zones 0 and 3 used M's block in blocks minted during beacon slot 51. Those blocks are discarded, and their commitments for slot 51, which arrive in beacon block 52, are marked invalid. Both zones roll back to their commitments for beacon slot 50.
5. **Beacon slot 53** ($e_{fault} + D$): zones 1, 0 and 3 each produce a recovery block and continue normally. M no longer appears in zone 1's minter ranking.

### Zone creation (joining a new zone at a point)
This is the lifecycle of a new zone: **propose → register → vote on each adjacent zone's chain → decide on the beacon chain → activate**. Symbols are defined in "Notation → Zone creation".

1. **Propose.**
   - Any registered node submits a `ProposeZone` tx on `beacon/tx`, naming:
     - the location: a point and a layer, which must be the layer the task declares;
     - the task and its params;
     - the slot length $S_{new}$, which must divide $B$ or be divisible by $B$;
     - at least one founder. Founders are nodes that will move into the new zone and run it, and each one co-signs.
   - Negotiation in the POC is a yes/no on a fixed proposal. After a rejection the proposer may submit a new proposal with different parameters.
2. **Register (beacon chain).** The beacon proposer includes the tx. The beacon state transition accepts it only if all of these hold:
   - the point is inside the lattice bounds, the requested layer of that point is still free (other layers of the point may already have zones), and it is not **locked**. Two pending proposals may not be for the same location, or for two locations that would be linked to each other. Two new zones can therefore never become linked without voting on each other;
   - at least one adjacent zone exists, so the network grows contiguously, and every adjacent zone has at least one member;
   - the task is registered, `validate_params` passes and $S_{new}$ fits the slot-ratio rule;
   - every founder is registered, is not a founder in another pending proposal, and is not the last member of its current zone;
   - the proposer has at least $\mathit{min\_stake}$ locked and no other pending proposal, and fewer than 4 proposals are pending network-wide (a spam limit, because stake has no real value yet).
   
   The state then records:
   - $A$;
   - a $\mathrm{voters}_a$ snapshot for each a ∈ A;
   - $e_{reg}$;
   - $\mathit{deadline} = e_{reg} + V + \max P_a$;
   - status `Voting`.
3. **Vote (each adjacent zone's own chain).**
   - **When voting opens:** at beacon slot $e_{reg} + D$, when the registering block is in everyone's anchor and therefore final. Votes for proposals not yet in the anchor are invalid, so every node validates zone blocks against the same knowledge.
   - **How a node decides:** each voter runs the same automatic checks:
     - the new zone's ports are compatible with the facing ports of its own zone (same payload schema);
     - the slot ratio is valid;
     - its own zone stays within link and bandwidth limits.

     Its **vote policy** then decides: `auto` (accept when the checks pass; the POC default), `manual` (a UI button or `umis-cli vote`), or `reject_all`.
   - **How votes are recorded:** the voter gossips a signed `ZoneVote` on `zone/<a>/vote`, and the zone's minters include pending votes in their next blocks.
   - **When the zone has decided:** at the first zone block where the recorded votes reach $q_{vote}$ (accept) or make it unreachable (reject).
4. **Decide (beacon chain).**
   - Each adjacent zone's next ZoneCommitment carries its `ZoneDecision`, with the signed votes in the gossip envelope. Beacon validators recount them.
   - The beacon state updates the proposal as follows:
     - **Accepted** as soon as every zone in $A$ has committed an accept. That block's slot is $e_{dec}$, and $e_{act}$ is computed.
     - **Rejected** as soon as any adjacent zone commits a reject.
     - **Expired** if the deadline passes first, e.g. because an adjacent zone is offline.
   - Rejected and Expired release the lock, and the founders stay where they are.
5. **Activate (at beacon slot $e_{act}$).**
   - The registry gains zone `next_zone_id` at the location, with links to every zone in $A$ in both directions: face ports to same-layer neighbours, and coupled ports to co-located zones of other layers.
   - The founders become its members. Because $e_{act} \ge e_{dec} + D$, the anchor of $e_{act}$ already lists them.
   - Native full nodes add it to their fallback-minter duties.
   - The new zone's first slot is $j_{start}$. Its first state is `initial_state(params, genesis.seed, j_start)`, which every node can compute, so its root is known without any data transfer.
   - Adjacent zones add the new input link from their first zone slot starting at or after $tb_{e_{act}}$. They use the empty-neighbour boundary until the new zone's first output arrives; like every input, it is used immediately and verified in parallel.
   - Topics `zone/<new>/*` and rendezvous namespace `zone/<new>` come into use, and other nodes may join the zone later through normal Joins.

**Worked example** (defaults: $D = 2$, $V = 10$, every zone dense, so every $P = 1$, and every zone in the `water` layer):
- **Setup:** genesis zones 0–3 sit at points (0,0,0), (1,0,0), (0,1,0) and (1,1,0). Zone 4 was created earlier at (2,0,0). A proposal for point (2,1,0) is made; its adjacent zones are zone 3 at (1,1,0) and zone 4 at (2,0,0).
- **Beacon slot 100:** beacon block 100 registers the proposal. $\mathit{deadline} = 100 + 10 + 1 = 111$.
- **Beacon slot 102:** voting opens.
- **Zone 3 votes:** zone 3 has 4 voters, so it needs 3 yes votes. Its third yes vote lands in a zone-3 block during beacon slot 102. Zone 3's commitment for slot 102 carries "accept" and is included in beacon block 103.
- **Zone 4 votes:** zone 4's accept is included in beacon block 104. Every adjacent zone has now accepted, so $e_{dec} = 104$ and $e_{act} = 106$.
- **Beacon slot 106:** zone 5 starts minting at $j_{start} = 106 \cdot 16 / S_{new}$. Zones 3 and 4 switch that face from still water to zone 5's outputs once its first outputs arrive.

**Several layers at one point** (prepared in code, not used by the POC devnet):
- Suppose genesis adds a future `atmosphere` layer with the coupling `water.top ↔ atmosphere.bottom`.
- A proposal for location $((1,1,0), \text{atmosphere})$ is then adjacent to:
  - water zone 3 at $((1,1,0), \text{water})$, through the coupling;
  - any atmosphere zones on neighbouring points, of which there are none yet.
- Zone 3's voters must accept, and after activation the two zones exchange surface data every slot like any other link.
- A second proposal for $((1,1,0), \text{water})$ is refused, because that location is occupied.

## 2. Task abstraction (umis-task)
```rust
pub trait ZoneTask: Send + Sync + 'static {
    const TASK_ID: &'static str;            // "water3d@1"
    const LAYER: &'static str;              // "water": the layer this task occupies at a point
    type Params: BorshSerialize + BorshDeserialize;   // zone-specific, from genesis
    type State;  type Inputs;  type Outputs;
    fn ports(p: &Self::Params) -> Vec<PortSpec>;       // named in/out ports, with payload schema ids
    fn validate_params(p: &Self::Params) -> Result<()>; // checked when a zone is proposed
    fn initial_state(p: &Self::Params, seed: &Hash, start_slot: u64) -> Self::State; // genesis zones: start_slot = 0
    fn empty_neighbour_input(p: &Self::Params, port: PortId) -> Bytes; // boundary used while a link has no input
    fn decode_inputs(p: &Self::Params, ports: &BTreeMap<PortId, Bytes>) -> Result<Self::Inputs>;
    fn step(p: &Self::Params, s: &Self::State, i: &Self::Inputs, slot: u64) -> Self::State; // deterministic, integer-only
    fn outputs(p: &Self::Params, s: &Self::State) -> BTreeMap<PortId, Bytes>;
    fn encode_state / decode_state / state_root(...)
}
```
- **Type erasure and registration:** `DynZoneTask` erases these types so the protocol engine works with bytes. `TaskRegistry` maps a `task_id` string to `(Box<dyn DynZoneTask>, Vec<Box<dyn ProofBackend>>)`.
- **Topology, layers and couplings:** genesis names one topology, which says where zones can live and which points neighbour each other. It also lists the allowed layers and the couplings between layers. Zones are placed at **locations** $(\text{point}, \text{layer})$.
  ```rust
  pub trait Topology {
      type Point: BorshSerialize + BorshDeserialize + Ord;
      fn in_bounds(&self, p: &Self::Point) -> bool;
      fn adjacent(&self, p: &Self::Point) -> Vec<Self::Point>;  // neighbouring points
      // ports linking two same-layer zones on neighbouring points a → b
      fn neighbour_links(&self, layer: &LayerId, a: &Self::Point, b: &Self::Point) -> Vec<LinkSpec>;
  }
  pub struct Location<P> { pub point: P, pub layer: LayerId }
  pub struct Coupling { pub layer_a: LayerId, pub port_a: PortId, pub layer_b: LayerId, pub port_b: PortId }
  // the single source of truth for adjacency and wiring:
  fn links_between<T: Topology>(t: &T, couplings: &[Coupling], a: &Location<T::Point>, b: &Location<T::Point>) -> Vec<LinkSpec>;
  ```
  - **The linking rule:** two locations are linked when they are in the same layer on neighbouring points (`neighbour_links`), or in different layers on the same point with a coupling between them (the coupling's ports, whose payload schemas must match).
  - **Every place uses `links_between`:** the adjacency set $A$, the lock check, activation wiring and genesis wiring. Nothing in the protocol assumes one zone per point.
  - **What the POC ships:** `Lattice3d` (integer points, face adjacency, face-port links), one layer `water` and no couplings. Later heterogeneous networks (weather above ocean) add layers and couplings in genesis, or a graph topology with named attachment ports.
- **Genesis wiring:** genesis zones name a location (point + layer), a `task_id` with params, and $S_z$. Their links come from `links_between`.
- **Generic links:** a link is `{to_zone, to_port, from_zone, from_port}`. The registry holds them for every active zone, so arbitrary DAG or cyclic wiring is possible for future tasks.

## 3. Water current task (task-water3d), ZK-friendly by construction
- **Determinism rules:**
  - consensus paths use **integer fixed-point only**: no floats, no libm transcendentals, fixed iteration counts, sequential reductions;
  - the genesis RNG is `rand_chacha` seeded with $H(\mathit{genesis.seed} \Vert \text{"water3d"})$;
  - CI checks that native and wasm give the same state hash.
- **Global domain:**
  - The domain is a lattice of points, each a block of $n_{loc}^3$ cells. Genesis fills $Z_x \times Z_y \times Z_z$ of them, e.g. $2 \times 2 \times 1$ zones of $16^3$ cells, giving $32 \times 32 \times 16$ cells. Zone creation can fill more points, up to the lattice bounds.
  - The domain is **not periodic**, because the set of occupied points changes over time.
  - water3d occupies the `water` layer. Its top face port (+z) is the natural place for a future coupling to an `atmosphere` layer at the same point.
  - A face toward an empty point, or toward the seabed or surface, uses the **empty-neighbour boundary**: still, dye-free water ($v = 0$, $c = 0$). It acts like an open sea at rest beyond the simulated region.
- **Background current** $\bar{U}(x)$, fixed from genesis:
  - It is the discrete curl of a vector potential built from $N_v$ random vortex tubes and jets (compact polynomial bumps), plus a drift. It is therefore **exactly discretely divergence-free** (incompressible) and continuous across zones, because it is evaluated in global coordinates.
  - $\mathrm{tide}(j)$ modulates the forcing over time.
- **State per cell:** velocity $v$ (3 components) and dye $c$, each an i32 fixed-point value bounded by $\pm 2^{15}$.
- **Linear (Oseen-type) update per substep** ($(\bar{U} \cdot \nabla)^{up}$ is upwind advection along the background current and $\nabla^2$ is the 7-point Laplacian):
  - $v' = v + \Delta t \cdot [-(\bar{U} \cdot \nabla)^{up} v + \nu \nabla^2 v + \lambda \cdot (\mathrm{tide}(j) \cdot \bar{U} - v)]$
  - $c' = c + \Delta t \cdot [-(\bar{U} \cdot \nabla)^{up} c + \kappa \nabla^2 c + \mathrm{tide}(j) \cdot \mathrm{src}(x) - \mu c]$
  - The upwind direction is known from the sign of $\bar{U}$, so after folding in $\Delta t$, $\nu$, $\kappa$, $\lambda$ and $\mu$ each update is $q'(x) = \left\lfloor \left( \sum_{k \in 7pt} w_k(x) \cdot q(x+k) + b(x,j) \right) / 2^s \right\rfloor$. The weights $w_k(x)$ are fixed, non-negative integers with $\sum_k w_k(x) \le 2^s$, which keeps the update monotone and bounded.
  - This form gives a linear constraint per cell plus a range check on the remainder, with no in-circuit comparisons.
  - Genesis validates the CFL and diffusion-number bounds.
- **Ports:** 6 face ports. The output is a 1-cell-thick face of all 4 fields; the input is the neighbour's opposite face, used as the halo. With no neighbour, the halo is the empty-neighbour boundary.
- **Initial state of a zone:** $v = \mathrm{tide}(j_{start}) \cdot \bar{U}(x)$ (the forced current at that moment) and $c = 0$. It is computed from `genesis.seed` and the zone's point, so a zone created at runtime starts consistent with the global current field and needs no state transfer.
- **Deferred:** the nonlinear self-advection term $(v \cdot \nabla) v$ (degree-2 constraints) and a pressure projection, as future flags.
- **`umis-cli sim`** runs the whole domain in one process without consensus, using an idealized input schedule where every consumer takes the causally freshest producer output. It is a physics reference. Network runs are instead checked by replaying the canonical zone chains with their recorded `InputRef`s.

## 4. Proofs (umis-proof, umis-stark, task-water3d::air)
- **Trait:** `ProofBackend { id(); prove(stmt, witness) -> ProofBytes; verify(stmt, public_data, proof) }`. It is keyed per `(task_id, backend_id)`, and genesis selects the backend.
- **`MockBackend` (first):** the proof is the prover's signature over the statement's `seg` address.
  - In its own zone, verification re-executes from the parent state plus the recorded inputs.
  - For cross-zone inputs, consumers accept the mock "proof" on the producer's signature. Invalid blocks are caught by producer-zone members and native full nodes, who re-execute every slot and submit `FaultEvidence`.
  - It exercises jobs, ProofShares, OutputBundles, BMT assembly and ProofCommitments end to end.
- **Plonky3 backend: dependencies and parameters.**
  - Pin every `p3-*` crate to `=0.8.0`; releases come about every 3 weeks and the API changes. The set is `p3-batch-stark` (required: LogUp only works there, and it accepts a single AIR) plus `p3-uni-stark` (`StarkConfig`), `p3-lookup`, `p3-air`, `p3-field`, `p3-matrix`, `p3-fri`, `p3-merkle-tree`, `p3-poseidon2`, `p3-symmetric`, `p3-commit`, `p3-dft`, `p3-challenger`, and the chosen field crate.
  - Config: Poseidon2 sponge/compression MMCS, `TwoAdicFriPcs`, `DuplexChallenger`. FRI starts from the `new_benchmark` preset (log_blowup 1, 100 queries, 16 PoW bits).
  - It produces validity proofs only. `HidingFriPcs` exists if a future private task needs hiding.
  - **The field is decided by the 7a benchmark.** On native, KoalaBear/BabyBear have AVX2/NEON packing. In wasm, only Goldilocks has a simd128 path, and its 64-bit range also gives the fixed-point values headroom. Genesis pins `stark_config_id`.
    - *Decided (7a): KoalaBear.* For the toy AIR (50 columns), native proving is within ~10% for both fields. In wasm, KoalaBear proves 1.5–1.8× faster (617 ms vs 946 ms at $2^{14}$ rows; 2.06 s vs 3.78 s at $2^{16}$), and its proofs are smaller. The water stencil fits the 31-bit range ($|q| \le 2^{15}$, $\sum w \le 2^s$). `stark_config_id` is `koalabear` (benchmark FRI) or `koalabear-test`.
  - Keep `parallel` off on wasm, on for native.
- **7a, toy AIR spike:**
  - A 1D stencil in `p3-batch-stark` with a local LogUp memory bus, a range-check lookup, and periodic columns carrying public state.
  - It is proven natively and in a wasm Web Worker. Plonky3 CI never builds the prover for wasm, so this is the first real check.
  - Measure prove and verify time and proof size for $2^{14}$ and $2^{16}$ rows $\times$ ~50 columns. No published numbers exist.
- **7b, water3d stencil AIR (tall layout):**
  - **Rows:** one per (substep $t$, cell $i$), proving one zone slot $j$. Height is $k_{sub} \times n_{loc}^3$ ($4 \times 4096$); both are powers of two. About 50 main columns: `q[4]` (the cell's 4 values at substep $t$), `nbr[6][4]` (neighbour values), `q_next[4]` (values at $t+1$), `rem[4]`, and output limbs.
  - **Preprocessed columns:** weights $w_k(x)$, forcing shape $F(x)$, boundary selectors `is_bnd_d`, the read multiplicity $\mathrm{mult}$, and the substep selectors `first` ($t = 0$) and `last` ($t = k_{sub}-1$).
  - **Neighbour reads use a LogUp memory argument** (`PermutationCheckBus`). Each row sends $(t+1, i, q_{next})$ with `Count::bounded(mult, 7)`. Each row receives $(t, i, q)$, plus $(t, \mathrm{nbr}_d(i), \mathit{nbr}[d])$ for every in-zone neighbour.
  - **Public data goes in periodic columns** (period $n_{loc}^3$), since 0.8 has no verifier-side LogUp term:
    - `X_in[4]`: rows with `first` constrain $q = X_{in}$ and send $(0, i, q)$;
    - `X_out[4]`: rows with `last` constrain $q_{next} = X_{out}$ and send nothing;
    - `halo_d[4]` per direction d: boundary rows read `halo_d` instead of the bus;
    - the verifier evaluates these columns natively from the state and halo blobs, which it already checks against `prev_state_root`, `state_root` and `inputs_commitment`.
  - **Transcript binding:** periodic columns are not absorbed into the Fiat–Shamir transcript, so the public values carry $[H(X_{in}), H(X_{out}), H(\mathit{halo}), \mathrm{tide}(j), \mathit{params\_hash}]$.
  - **Range checks:** $\mathrm{rem} \in [0, 2^s)$ and the output limbs (bounding $|q'| < 2^{15}$) are checked by a LogUp lookup into a $2^8$ or $2^{12}$ table column.
  - **Arithmetic bounds:** every numerator $\sum_k w_k(x) \cdot q(x+k) + b(x,j)$ must stay below $p/2$. This is checked at genesis for the chosen field. For multiplicity soundness, $7 \times \mathit{height} < p$ must hold.
  - **Property tests:** AIR witness generation must match `step()` bit for bit, and tampered traces (a cell, the halo, $X_{out}$, a remainder, a multiplicity) must fail.
  - **Fallbacks:** (1) a ~20-line fork of the batch-stark verifier that accepts an external LogUp terminal offset; (2) a wide layout (one row per substep, full state as columns) for $8^3$ zones.
- **Proof-lag mechanics:**
  - consumers do **not** wait for the producer's slot proof. They use the output at once and verify the proof whenever it arrives. A proof that fails leads to `FaultEvidence`; one that never arrives leads to a missing-proof rollback (§1 "Faults, rollback and slashing");
  - $L$ is the proof deadline: a ProofCommitment must reach the beacon chain within $L$ beacon slots of its ZoneCommitment;
  - beacon validators verify proofs before accepting a ProofCommitment;
  - browsers prove in a dedicated Web Worker; native nodes use `spawn_blocking` (Plonky3 `parallel` feature on native only).
    - *As built:* native nodes prove on a dedicated 2-thread pool (`UMIS_PROVER_THREADS`). Proving on all cores slowed every node's event loop, rank-0 zone blocks then missed the fallback window, and the resulting forks caused cross-link rollbacks. Browsers use a pool of up to 4 workers shared by a page's nodes.
    - The node runs one job at a time and queues the rest. Jobs for blocks in pending commitments go first (they have the deadline $L$), then the zone's best chain, earliest slot first. Jobs whose block went stale are dropped.
    - Measured per slot (benchmark FRI): $n_{loc}=16$ proves in 397 ms natively on 2 threads and 682 ms in single-threaded wasm, verifies in 11 ms, and the proof is 261 KB. There is no aggregation: beacon validators verify every slot proof, in parallel natively, memoized.

## 5. Networking (umis-net, libp2p 0.57.x)

| Role | Transports | Behaviours |
|---|---|---|
| `bootstrap` (native) | QUIC, WebRTC-direct (`libp2p-webrtc` 0.10.0-alpha via `with_other_transport`), WS | relay **server** (limits raised: circuit duration and bytes, reservations), rendezvous **server**, identify, ping, autonat server, gossipsub + request-response; serves `genesis.json` + `web/dist` over HTTP (axum) |
| `node` (native full node) | QUIC, WebRTC-direct listener, WS | relay client, dcutr, rendezvous client, identify, ping, gossipsub, request-response; follows all zones as fallback minter |
| browser `SimNode` (wasm) | `webrtc-websys` (→ native), `websocket-websys` (fallback) | relay client (listens on `/p2p/<relay>/p2p-circuit`), rendezvous client, identify, ping, gossipsub, request-response |

- **Rendezvous is configurable** as a list of multiaddrs: `--rendezvous` on the CLI, a field or `?rv=` in the web UI.
  - Namespaces: `simu/<genesis8>/all` and `simu/<genesis8>/zone/<z>`.
  - Nodes register their reachable addresses: native nodes their QUIC and WebRTC-direct addresses with certhash; browsers their circuit address. They re-register before the TTL expires.
- **Gossipsub settings:**
  - `max_transmit_size = 4 MiB`;
  - message id = keccak(data);
  - `validate_messages()`, with app-level accept/reject coming from the protocol core, so invalid data never propagates;
  - small mesh parameters.
- **Peering policy:**
  - connect to all members of your own zone, one bridge peer in every zone you consume from (see the cross-zone data plane), a few beacon peers, and the relay;
  - browsers cap their connections, e.g. at 8;
  - a peer reached through a relay circuit is dialled directly when a WebRTC-direct address is available.
- **wasm specifics:**
  - libp2p 0.57 has no `wasm-bindgen` feature;
  - enable `getrandom` `wasm_js` in `umis-web` only;
  - pin `wasm-bindgen-futures = "=0.4.58"` as the libp2p workspace does;
  - use `web-time` and `futures-timer`; never use `std::time::Instant` or tokio timers in wasm paths.
- **Many nodes per page:**
  - each `SimNode` has its own keypair (stored in localStorage as `simu.node.<n>`) and its own Swarm, spawned with `spawn_local`;
  - all Swarms live on the **main thread**, because RTCPeerConnection is Window-only; only compute (proving) goes to workers;
  - nodes in the same page reach each other through the relay.
- **Later:** a `webrtc-b2b` cargo feature with `[patch]` to PR #5978 or a fork, for direct browser↔browser connections.

## 6. Runtime (umis-node) and web UI
- **Runtime:** `Runtime<Net, Clock, Exec>` runs a `futures::select!` loop over swarm events, timers and compute completions. It feeds `Event`s into `umis-protocol` and executes the returned `Action`s (Publish, Request, Respond, SetTimer, Compute{step|prove|verify}, UiEvent).
  - The native build uses tokio; wasm uses `wasm_bindgen_futures`.
  - State is kept in memory, with checkpoints.
- **`umis-web` API:** `new SimNode(config)`, `start()`, `stop()`, `status()`, `onEvent(cb)`, `zoneState(z) -> Float32Array` (fixed-point converted for display only), `lattice(range)`.
- **`web/` UI** (Vite + TS):
  - genesis/rendezvous config and an **Add node** button;
  - per-node cards showing peer id, zone, role this slot, peers, beacon head and finalized height, proofs;
  - a **lattice explorer**: a grid of beacon slots × zones showing commitment and proof roots, with click-through to BMT leaves. It colours each commitment by status (optimistic / proven / settled / invalid) and marks faults, slashes and rollbacks;
  - each node card shows its stake and has a "get test stake" (faucet) button;
  - a **three.js 3D view** stitching zone states in global coordinates: instanced velocity arrows plus a dye point cloud. Free locations of the selected layer are shown as ghost blocks. A layer selector exists, and only `water` is available in the POC.
  - **Create zone:**
    - click a free ghost block (a free location), choose $S_{new}$ and the founders (nodes running in this page), then submit `ProposeZone`;
    - a **Proposals** panel shows each proposal's status and per-adjacent-zone tally;
    - nodes with the `manual` vote policy get Accept/Reject buttons.
- **CLI:** `umis-cli propose-zone --point 2,1,0 --layer water --slot-ticks 4 --founder <node_id>…`, `umis-cli vote <proposal_id> accept|reject`, and `umis-cli proposals`.

## 7. Milestones (each ends green on `scripts/check.sh`)
- **M0, toolchain and skeleton:**
  - install rustup stable, add the `wasm32-unknown-unknown` target, install wasm-pack (or a wasm-bindgen-cli version matching the pin);
  - set up the workspace, `check.sh` (fmt, clippy, test native; check wasm crates for wasm32), and a basic CI script.
- **M1, connectivity spike (de-risks the unknowns early):**
  - a minimal native bootstrap (QUIC, WebRTC-direct, WS, relay, rendezvous) plus a minimal wasm page with 2 Swarms in one tab and 1 in another;
  - each discovers the others through rendezvous and exchanges gossip, browser↔browser through the relay;
  - this confirms multiple Swarms per page work and that the relay client works in wasm.
- **M2, `umis-core`:** types, codec, keccak, the bmt-js port with conformance vectors, time arithmetic, genesis generation and loading, with unit and property tests.
- **M3, `umis-task` + `task-water3d`:** the location/layer/coupling model, fixed-point math, background field, step, ports, `umis-cli sim`, and the native==wasm golden-hash test.
- **M4, `umis-proof` (mock) + `umis-protocol` + `umis-simnet`:** the full lattice protocol, including zone creation, faults, rollback and slashing, validated in SimNet with churn, partitions, delays, adversarial, fault and zone-creation scenarios.
  - Exit: 4 zones × 8 nodes × 200 beacon slots, with all invariants holding for 100 seeds.
- **M5, `umis-net` + `umis-node` + `umis-cli` native devnet:** bootstrap plus 4 native nodes run the lattice over real libp2p, and late joiners sync from checkpoints.
- **M6, `umis-web` + `web/`:** browser nodes join as full participants (they mint and validate), several per tab, with the 3D view, the lattice explorer and the create-zone/proposals UI.
- **M7, Plonky3:**
  - 7a: toy AIR native and in wasm, plus benchmarks;
  - 7b: the water3d AIR, helper proof jobs, $L \ge 1$, a beacon-side verifier and the worker prover;
  - 7c: switch the devnet genesis to the `plonky3` backend (done: `umis-cli genesis` defaults to `plonky3`; SimNet keeps `mock` except for its `stark` scenario).
- **M8, hardening:** metrics and `Status`, README and architecture doc, a demo script, and optionally `webrtc-b2b`.

## 8. Verification
- **BMT conformance with bmt-js:**
  - Port the bmt-js unit-test vectors:
    - the address of `[1,2,3]` = `ca6357a0…4338`;
    - the address of empty bytes = `b34ca8c2…1526`;
    - the "hello world" inclusion-proof segments for indices 0, 64 and 127;
    - span encoding.
  - `scripts/bmt-vectors.mjs` runs `@nugaon/bmt-js@3.0.2` on Node 24 to generate JSON vectors (file addresses plus file inclusion proofs) for edge-case sizes. Those include 0, 1, 31, 32, 4095, 4096, 4097, $128 \cdot 4096$, $128 \cdot 4096 + 2$ (the carrier chunk, the same size as bmt-js's `carrier-chunk-blob`), $128^2 \cdot 4096 + 4096$ and random sizes. The Rust port must match every vector byte for byte.
- **`seg` codec tests:**
  - round-trip encoding;
  - rejection of non-zero padding;
  - for every record type and every field, a `prove_field` proof checks against the record address computed by bmt-js (vectors from `scripts/bmt-vectors.mjs`);
  - the compile-time layout assertions.
- **Unit and property tests:** `cargo test --workspace` (proptest) covers the slot/commit/anchor formulas, rank determinism, fixed-point bounds, the CFL checks, and BMT proof round-trips for random payloads and segment indices.
- **Determinism:** `wasm-pack test --node crates/task-water3d` must reproduce the native golden state hash after N steps. The same check applies to the AIR witness generator.
- **SimNet:** `cargo test -p umis-simnet` and `cargo run -p umis-simnet -- --seeds 100 --scenario churn|partition|adversary|faults|zone-create`.
  - **Zone-creation scenarios:**
    - all adjacent zones accept, and the zone activates exactly at $e_{act}$;
    - one adjacent zone rejects, so the proposal is rejected;
    - an adjacent zone is offline, so the proposal expires at the deadline;
    - a second proposal for the same location, or for a location that would be linked to a pending one, is refused;
    - **several layers:** a test-only second layer (`probe`, run by a tiny test task coupled to water's top face) is created on an occupied water point. The co-located water zone is in $A$ and votes, the two zones exchange data once it is active, and a second `water` zone at that point is refused;
    - the registering beacon block gets reorged out, so no zone is created and its votes are invalid;
    - a sparse adjacent zone decides within the deadline;
    - a founder's browser tab closes before activation.
  - **Fault scenarios** (`--scenario faults`):
    - a minter signs an invalid transition: it is detected, reported, slashed and removed from its zone, and the producer zone plus every contaminated consumer zone roll back and recover;
    - a minter equivocates;
    - a minter records a forged `InputRef`;
    - an honest fallback race orphans a consumed producer block: consumers roll back and nobody is slashed;
    - a proof misses its deadline: rollback, nobody slashed;
    - two faults overlap in time.
  - **Invariants:**
  - honest nodes' finalized beacon chains are prefixes of one another;
  - nodes compute identical anchors, members and ranks per (zone, slot);
  - replaying every canonical zone chain with its recorded `InputRef`s reproduces all state roots;
  - commitments are contiguous, and the commitment for beacon slot $e$ is at physical time $(e+1) \cdot B \cdot dt_{tick}$;
  - every `InputRef` in a settled commitment is legitimate, causal and monotonic, is proven, and lies on the producer's committed chain;
  - an honest node is never slashed, and every injected punishable fault is slashed exactly once;
  - a rollback never reverts a settled commitment and reaches back at most $L + D + 1$ beacon slots;
  - after recovery, every zone's state equals the replay of its canonical chain, and all zones are again at the same physical time at each commit boundary;
  - no valid commitment depends, directly or transitively, on a discarded block;
  - proof leaves chain and their roots verify;
  - liveness: every zone commits within 2 beacon slots while at least one member or full node is online, and a dead neighbour only causes "hold last value", never a stall;
  - with a healthy network, input staleness is about one producer slot plus network delay;
  - nodes converge to the same canonical beacon and zone chains whatever the delivery order;
  - a zone becomes active only if every adjacent zone committed an accept backed by valid signed votes from at least two thirds of its $\mathrm{voters}_a$;
  - all honest nodes agree on the zone registry, including locations, links and members, for every beacon slot;
  - no two zones ever have both the same point and the same layer; zones in different layers of one point are expected;
  - a created zone's initial state root is identical on all nodes, and its neighbours never consume its outputs from before $tb_{e_{act}}$;
  - the same seed gives the same trace hash.
- **ZK:** `cargo test -p task-water3d --features stark` proves and verifies random states. Negative tests tamper with a cell, the halo, $X_{out}$ and a remainder, and all must be rejected. Benchmarks run natively and in wasm.
- **Native devnet:** `scripts/devnet.sh 4` starts a bootstrap and 4 nodes. `umis-cli status --all` must show identical finalized beacon hashes and zone state roots, and killing or restarting a node must keep the lattice live.
- **Browser end to end:**
  - `umis-cli bootstrap --serve-web web/dist`, then open `http://localhost:8080/?rv=<addr>` in two tabs with 2–3 nodes each;
  - check that browser nodes mint and prove, that `chrome://webrtc-internals` shows WebRTC-direct, and that closing a tab doesn't stall its zone;
  - a Playwright spec (`web/e2e/lattice.spec.ts`) automates this check by waiting for each node to mint and for all of them to agree on the finalized hash.

## 9. Risks and future work
- **Risks, with mitigations:**
  - native `libp2p-webrtc` is alpha. Mitigation: browsers fall back to WS or WSS through the relay.
  - wasm-bindgen version pins may clash between libp2p and other dependencies. Mitigation: check `cargo tree -d` in `check.sh`.
  - the Plonky3 API is changing (0.8 → 0.9-rc), so pin `=0.8.0`. Its prover has never been built for wasm in Plonky3's CI. Mitigation: the M7a spike runs before any AIR work, and §4 lists fallbacks.
  - single-threaded wasm proving is slow. Mitigation: one prover per worker, with parallelism across helpers.
  - the physics fidelity is limited (linear model, lagged halos), which is accepted for a POC;
  - optimistic inputs make rollbacks visible: a fault can discard up to about $L + D + 1$ beacon slots of work in several zones. Recovery is a cheap deterministic recomputation, and SimNet measures how often rollbacks happen and how far they spread;
  - devnet stake comes from a faucet and has no value, so slashing demonstrates the mechanism but doesn't yet deter Sybil attacks.
- **Future work:**
  - VRF leader election and a BFT finality gadget;
  - recursive aggregation of each commitment's proof BMT into a single proof, and in-circuit Poseidon2 state roots;
  - data availability for proof bundles and checkpoints on Swarm, where the bmt-js roots already equal the Swarm references;
  - zone-local fast finality (member attestation quorums), so commitments settle sooner and rollback windows shrink;
  - zone lifecycle beyond creation: dissolving, merging and splitting zones, counter-proposals during negotiation, and stake-weighted votes;
  - real token economics: stake with value instead of a faucet, stake-weighted leader election, and slashing for provers who sign bad ProofShares;
  - nonlinear physics, heterogeneous tasks, and ZK hiding for private tasks (e.g. human-behaviour data).
