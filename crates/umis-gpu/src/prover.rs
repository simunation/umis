//! Generic GPU-resident uni-STARK prover for KoalaBear AIRs with base-field
//! constraints (no lookups, no preprocessed columns, no ZK).
//!
//! The steps of `p3_uni_stark::prove`, with the trace LDE, the Merkle trees, the
//! quotient, the out-of-domain values and FRI's inputs on the GPU
//! ([`crate::resident`]); Plonky3's transcripts and FRI prover drive the protocol,
//! so the result is the CPU Keccak configuration's [`Proof`]
//! (`umis_stark::config::koala_bear_keccak::Config`), verified unchanged.
//!
//! The quotient kernel is compiled from the AIR's own symbolic constraints (once per
//! AIR shape, [`crate::jit`]). [`GpuStarkProver::prove_batch`] makes one
//! `p3_batch_stark` proof over several instances, as `p3_batch_stark::prove_batch`.

use crate::resident::{self, Challenge, DevMat, GpuMmcs, GpuTree, Periodic, Program, Val};
use p3_air::symbolic::{BaseEntry, BaseLeaf, SymbolicAirBuilder, SymbolicExpr, SymbolicExpression};
use p3_air::{Air, BaseAir};
use p3_batch_stark::{BatchCommitments, BatchOpenedValues, BatchProof, BatchProverTranscript, BatchShape};
use p3_commit::PolynomialSpace;
use p3_dft::Radix2DitParallel;
use p3_field::coset::TwoAdicMultiplicativeCoset;
use p3_field::{Field, PrimeCharacteristicRing};
use p3_fri::{BatchMultiOpening, FriParameters, FriProof};
use p3_matrix::Matrix;
use p3_matrix::dense::RowMajorMatrix;
use p3_uni_stark::{
    AirLayout, Commitments, OpenedValues, Proof, StarkGenericConfig, StarkOpeningLayout, StarkProverTranscript,
    StarkShape, get_log_num_quotient_chunks_for_domain, get_symbolic_constraints,
};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use umis_stark::config::FriPreset;
use umis_stark::config::koala_bear_keccak::{ChallengeMmcs, Config, ValMmcs, config, fri_params};

type Com = p3_symmetric::MerkleCap<Val, [u64; 4]>;

/// Compiles an AIR's symbolic base-field constraints to postfix bytecode.
pub fn compile<A: Air<SymbolicAirBuilder<Val>> + BaseAir<Val>>(air: &A, layout: AirLayout) -> Result<Program, String> {
    let cons = get_symbolic_constraints::<Val, A>(air, layout);
    if let Some(n) = air.num_constraints()
        && n != cons.len()
    {
        return Err(format!(
            "{n} constraints declared, {} base-field constraints found (lookups are not supported)",
            cons.len()
        ));
    }
    const STACK: usize = 48;
    const SLOTS: usize = 64;
    type Node = SymbolicExpression<Val>;
    // References of every node within one constraint (shared `Arc`s).
    fn count(e: &Arc<Node>, refs: &mut HashMap<*const Node, usize>) {
        let n = refs.entry(Arc::as_ptr(e)).or_insert(0);
        *n += 1;
        if *n > 1 {
            return;
        }
        match &**e {
            SymbolicExpr::Leaf(_) => {}
            SymbolicExpr::Add { x, y, .. } | SymbolicExpr::Sub { x, y, .. } | SymbolicExpr::Mul { x, y, .. } => {
                count(x, refs);
                count(y, refs);
            }
            SymbolicExpr::Neg { x, .. } => count(x, refs),
        }
    }
    struct Emitter<'a> {
        code: Vec<u32>,
        refs: &'a HashMap<*const Node, usize>,
        slots: HashMap<*const Node, usize>,
    }
    impl Emitter<'_> {
        fn op(o: u32, arg: usize) -> Result<u32, String> {
            if arg >= 1 << 28 {
                return Err("index too large".into());
            }
            Ok(o | (arg as u32) << 4)
        }
        /// Emits `e`; a node referenced more than once is computed once into a slot.
        fn emit(&mut self, e: &Node, ptr: *const Node) -> Result<usize, String> {
            if let Some(&slot) = self.slots.get(&ptr) {
                self.code.push(Self::op(13, slot)?);
                return Ok(1);
            }
            let depth = match e {
                SymbolicExpr::Leaf(leaf) => {
                    match leaf {
                        BaseLeaf::Variable(v) => {
                            let w = match v.entry {
                                BaseEntry::Main { offset: 0 } => Self::op(0, v.index)?,
                                BaseEntry::Main { offset: 1 } => Self::op(1, v.index)?,
                                BaseEntry::Periodic => Self::op(2, v.index)?,
                                BaseEntry::Public => Self::op(3, v.index)?,
                                other => return Err(format!("unsupported variable {other:?}")),
                            };
                            self.code.push(w);
                        }
                        BaseLeaf::Constant(c) => {
                            self.code.push(4);
                            self.code.push(crate::lde::raw(*c));
                        }
                        BaseLeaf::IsFirstRow => self.code.push(5),
                        BaseLeaf::IsLastRow => self.code.push(6),
                        BaseLeaf::IsTransition => self.code.push(7),
                    }
                    return Ok(1);
                }
                SymbolicExpr::Add { x, y, .. } | SymbolicExpr::Sub { x, y, .. } | SymbolicExpr::Mul { x, y, .. } => {
                    let dx = self.emit(x, Arc::as_ptr(x))?;
                    let dy = self.emit(y, Arc::as_ptr(y))?;
                    self.code.push(match e {
                        SymbolicExpr::Add { .. } => 8,
                        SymbolicExpr::Sub { .. } => 9,
                        _ => 11,
                    });
                    dx.max(dy + 1)
                }
                SymbolicExpr::Neg { x, .. } => {
                    let d = self.emit(x, Arc::as_ptr(x))?;
                    self.code.push(10);
                    d
                }
            };
            if self.refs.get(&ptr).copied().unwrap_or(0) > 1 {
                let slot = self.slots.len();
                if slot >= SLOTS {
                    return Err(format!("constraint needs more than {SLOTS} shared sub-expressions"));
                }
                self.slots.insert(ptr, slot);
                self.code.push(Self::op(12, slot)?);
            }
            Ok(depth)
        }
    }
    // Templates: the opcode sequence with trace/periodic/public indices and constants
    // replaced by argument numbers; instances: (arguments, constraint index).
    let mut templates: Vec<(Vec<u32>, Vec<(Vec<u32>, u32)>)> = Vec::new();
    let mut index: HashMap<Vec<u32>, usize> = HashMap::new();
    for (ci, c) in cons.iter().enumerate() {
        let root = Arc::new(c.clone());
        let mut refs = HashMap::new();
        count(&root, &mut refs);
        let mut em = Emitter { code: Vec::new(), refs: &refs, slots: HashMap::new() };
        let depth = em.emit(&root, Arc::as_ptr(&root))?;
        if depth > STACK {
            return Err(format!("constraint needs a stack of {depth} (max {STACK})"));
        }
        let (mut tpl, mut args) = (Vec::new(), Vec::new());
        let mut words = em.code.into_iter();
        while let Some(w) = words.next() {
            match w & 15 {
                0..=3 => {
                    tpl.push(w & 15 | (args.len() as u32) << 4);
                    args.push(w >> 4);
                }
                4 => {
                    tpl.push(4 | (args.len() as u32) << 4);
                    args.push(words.next().ok_or("truncated constant")?);
                }
                _ => tpl.push(w),
            }
        }
        let g = *index.entry(tpl.clone()).or_insert_with(|| {
            templates.push((tpl, Vec::new()));
            templates.len() - 1
        });
        templates[g].1.push((args, ci as u32));
    }
    let (mut code, mut groups, mut flat) = (Vec::new(), Vec::new(), Vec::new());
    for (tpl, inst) in templates {
        let na = inst[0].0.len();
        groups.extend_from_slice(&[code.len() as u32, (code.len() + tpl.len()) as u32, na as u32, inst.len() as u32]);
        groups.push(flat.len() as u32);
        code.extend_from_slice(&tpl);
        for a in 0..na {
            flat.extend(inst.iter().map(|(args, _)| args[a]));
        }
        flat.extend(inst.iter().map(|&(_, c)| c));
    }
    Ok(Program::new(code, groups, flat, cons.len()))
}

/// The GPU-resident prover (one per process; it is `Sync`).
pub struct GpuStarkProver {
    cfg: Config,
    fri: FriParameters<ChallengeMmcs>,
    mmcs: GpuMmcs,
    dft: Radix2DitParallel<Val>,
    /// The prover's CPU parts (transcript, FRI, small host loops) are short: a few
    /// threads do them best, and a small pool keeps them fast on a busy machine.
    pool: rayon::ThreadPool,
    /// Per AIR shape (caller's key): bytecode and log2 of the quotient chunk count.
    programs: Mutex<HashMap<u64, (Arc<Program>, usize)>>,
    /// Compile constraint kernels with NVRTC (else interpret the bytecode);
    /// `UMIS_GPU_JIT=0` turns it off.
    jit: bool,
}

/// One instance of a batch proof: its AIR, the AIR's shape key (same key, same
/// constraints), its trace LDE on the device and its public values.
pub struct GpuInstance<'a, A> {
    pub air: &'a A,
    pub key: u64,
    pub lde: DevMat,
    pub public: Vec<Val>,
}

/// The CPU configuration's FRI proof.
type CpuFriProof = FriProof<Challenge, ChallengeMmcs, Val, Vec<BatchMultiOpening<Val, ValMmcs>>>;

/// Same fields, same types but the input MMCS label: the CPU proof type.
fn cpu_fri_proof(p: resident::GpuFriProof<ChallengeMmcs>) -> CpuFriProof {
    FriProof {
        batch_pow_witness: p.batch_pow_witness,
        commit_phase_commits: p.commit_phase_commits,
        commit_pow_witnesses: p.commit_pow_witnesses,
        input_openings: p
            .input_openings
            .into_iter()
            .map(|o| BatchMultiOpening { opened_values: o.opened_values, opening_proof: o.opening_proof })
            .collect(),
        commit_phase_openings: p.commit_phase_openings,
        final_poly: p.final_poly,
        query_pow_witness: p.query_pow_witness,
    }
}

fn layout_of<A: BaseAir<Val>>(air: &A) -> AirLayout {
    AirLayout {
        main_width: air.width(),
        num_public_values: air.num_public_values(),
        num_periodic_columns: air.num_periodic_columns(),
        ..Default::default()
    }
}

fn profiler(prof: bool, label: &'static str) -> impl Fn(&str) {
    let t0 = std::time::Instant::now();
    move |what: &str| {
        if prof {
            eprintln!("  gpu {label} {what}: {:?}", t0.elapsed());
        }
    }
}

impl GpuStarkProver {
    pub fn new(preset: FriPreset) -> Self {
        let threads = std::env::var("UMIS_GPU_CPU_THREADS").ok().and_then(|v| v.parse().ok()).unwrap_or(4);
        let pool = rayon::ThreadPoolBuilder::new().num_threads(threads).build().expect("thread pool");
        if std::env::var("UMIS_GPU_GRIND").as_deref() != Ok("0") && crate::gpu_available() {
            umis_stark::config::koala_bear_keccak::set_grinder(crate::pow::grind);
        }
        Self {
            cfg: config(preset),
            fri: fri_params(preset),
            mmcs: GpuMmcs::default(),
            dft: Radix2DitParallel::default(),
            pool,
            programs: Mutex::new(HashMap::new()),
            jit: std::env::var("UMIS_GPU_JIT").as_deref() != Ok("0"),
        }
    }

    /// Interprets the constraint bytecode instead of compiling it (tests, comparisons).
    pub fn interpreted(mut self) -> Self {
        self.jit = false;
        self
    }

    pub fn log_blowup(&self) -> usize {
        self.fri.log_blowup
    }

    /// Coset LDE of a host trace on `GENERATOR * K` (bit-reversed rows), on the device.
    pub fn trace_lde(&self, trace: &RowMajorMatrix<Val>) -> Result<DevMat, String> {
        resident::trace_lde(trace, self.fri.log_blowup, Val::GENERATOR)
    }

    /// Commits a host trace (coset LDE on `GENERATOR * K` and Merkle tree on the GPU).
    pub fn commit_trace(&self, trace: &RowMajorMatrix<Val>) -> Result<GpuTree, String> {
        resident::commit_trace(trace, self.fri.log_blowup, Val::GENERATOR)
    }

    /// Proves `air` over a host trace.
    pub fn prove<A>(
        &self,
        air: &A,
        key: u64,
        trace: &RowMajorMatrix<Val>,
        public: &[Val],
    ) -> Result<Proof<Config>, String>
    where
        A: Air<SymbolicAirBuilder<Val>> + BaseAir<Val> + Sync,
    {
        let tree = self.commit_trace(trace)?;
        self.prove_committed(air, key, tree, trace.height(), public)
    }

    /// Proves `air` over a trace already committed on the GPU (`trace_tree` holding
    /// one matrix, from [`Self::commit_trace`] or a custom trace kernel; LDE on
    /// `GENERATOR * K`, rows bit-reversed). `key` identifies the AIR's shape (same key,
    /// same constraints) for the constraint-kernel cache.
    pub fn prove_committed<A>(
        &self,
        air: &A,
        key: u64,
        trace_tree: GpuTree,
        degree: usize,
        public: &[Val],
    ) -> Result<Proof<Config>, String>
    where
        A: Air<SymbolicAirBuilder<Val>> + BaseAir<Val> + Sync,
    {
        self.pool.install(|| self.prove_inner(air, key, trace_tree, degree, public))
    }

    /// One `p3_batch_stark` proof over several instances of one height (`degree`
    /// trace rows), verified by `p3_batch_stark::verify_batch` with the CPU config.
    pub fn prove_batch<A>(
        &self,
        instances: Vec<GpuInstance<'_, A>>,
        degree: usize,
    ) -> Result<BatchProof<Config>, String>
    where
        A: Air<SymbolicAirBuilder<Val>> + BaseAir<Val> + Sync,
    {
        self.pool.install(|| self.prove_batch_inner(instances, degree))
    }

    /// The constraint program of `air` (cached under `key`) and log2 of its quotient chunk count.
    fn program<A>(&self, air: &A, key: u64, log_degree: usize) -> Result<(Arc<Program>, usize), String>
    where
        A: Air<SymbolicAirBuilder<Val>> + BaseAir<Val>,
    {
        if air.preprocessed_width() > 0 {
            return Err("preprocessed columns are not supported".into());
        }
        if let Some(p) = self.programs.lock().expect("programs").get(&key).cloned() {
            return Ok(p);
        }
        let layout = layout_of(air);
        let mut prog = compile(air, layout)?;
        prog.allow_jit = self.jit;
        let trace_domain = TwoAdicMultiplicativeCoset::new(Val::ONE, log_degree).ok_or("trace domain")?;
        let lq = get_log_num_quotient_chunks_for_domain::<Val, A>(air, layout, trace_domain, 0);
        if std::env::var_os("UMIS_GPU_PROFILE").is_some() {
            eprintln!(
                "  program: {} constraints in {} templates ({} template words, {} argument words)",
                prog.n_constraints,
                prog.groups.len() / 5,
                prog.code.len(),
                prog.args.len()
            );
        }
        let p = (Arc::new(prog), lq);
        self.programs.lock().expect("programs").insert(key, p.clone());
        Ok(p)
    }

    /// The committed quotient chunks of `airs` (instances of one AIR shape, trace LDEs
    /// side by side in `lde`, `inst_width` columns each). Every constraint is a
    /// base-field constraint, so constraint i gets weight alpha^(T - 1 - i)
    /// (`ConstraintLayout::decompose_alpha`).
    #[allow(clippy::too_many_arguments)]
    fn quotient_tree<A>(
        &self,
        airs: &[&A],
        prog: &Program,
        lde: &DevMat,
        inst_width: usize,
        publics: &[Vec<Val>],
        log_degree: usize,
        log_chunks: usize,
        alpha: Challenge,
    ) -> Result<GpuTree, String>
    where
        A: BaseAir<Val>,
    {
        let m = airs.len();
        let degree = 1usize << log_degree;
        let trace_domain = TwoAdicMultiplicativeCoset::new(Val::ONE, log_degree).ok_or("trace domain")?;
        let log_q = log_degree + log_chunks;
        let quotient_domain = trace_domain.create_disjoint_domain(1 << log_q);
        let sels = trace_domain.selectors_on_coset(quotient_domain);
        let mut apw: Vec<Challenge> = alpha.powers().take(prog.n_constraints).collect();
        apw.reverse();
        let per: Vec<_> = airs.iter().map(|a| a.periodic_columns()).collect();
        let n_per = per[0].len();
        if per.iter().any(|p| p.len() != n_per) {
            return Err("instances differ in periodic columns".into());
        }
        let columns: Vec<Vec<Val>> = per.iter().flat_map(|p| p.iter().cloned()).collect();
        let table;
        let periodic = if n_per == 0 {
            Periodic::None
        } else if columns.iter().all(|c| c.len() == degree) {
            Periodic::FullPeriod { columns: &columns, shift: quotient_domain.shift() }
        } else {
            let mut v = vec![Val::ZERO; (1 << log_q) * n_per * m];
            for (k, p) in per.iter().enumerate() {
                let t = p3_fri::build_periodic_lde_table_two_adic(&self.dft, p, &trace_domain, &quotient_domain);
                for r in 0..1usize << log_q {
                    for c in 0..n_per {
                        v[(r * m + k) * n_per + c] = *t.get(r, c);
                    }
                }
            }
            table = v;
            Periodic::Table { values: &table, width: n_per * m }
        };
        let flat_pub: Vec<Val> = publics.iter().flatten().copied().collect();
        let sel = [&sels.is_first_row[..], &sels.is_last_row[..], &sels.is_transition[..], &sels.inv_vanishing[..]];
        let values = resident::air_quotient(
            lde,
            inst_width,
            m,
            prog,
            periodic,
            n_per,
            &flat_pub,
            sel,
            &apw,
            1 << (log_q - log_degree),
            log_q,
            log_chunks,
        )?;
        let shifts: Vec<Val> = quotient_domain.split_domains(1 << log_chunks).iter().map(|d| d.shift()).collect();
        resident::commit_quotient(values, m, log_q, log_chunks, self.fri.log_blowup, &shifts)
    }

    fn prove_inner<A>(
        &self,
        air: &A,
        key: u64,
        trace_tree: GpuTree,
        degree: usize,
        public: &[Val],
    ) -> Result<Proof<Config>, String>
    where
        A: Air<SymbolicAirBuilder<Val>> + BaseAir<Val> + Sync,
    {
        let lap = profiler(std::env::var_os("UMIS_GPU_PROFILE").is_some(), "prove");
        let log_degree = degree.trailing_zeros() as usize;
        let (prog, log_chunks) = self.program(air, key, log_degree)?;
        let num_quotient_chunks = 1usize << log_chunks;
        let trace_domain = TwoAdicMultiplicativeCoset::new(Val::ONE, log_degree).ok_or("trace domain")?;
        let mut challenger = self.cfg.initialise_challenger();
        let trace_commit: Com = trace_tree.commitment();
        let mut transcript = StarkProverTranscript::<_, Val, Challenge>::new(
            &mut challenger,
            StarkShape::new::<Val, A>(
                air,
                0,
                log_degree,
                log_degree,
                num_quotient_chunks,
                false,
                self.cfg.ood_proof_of_work_bits(),
            ),
        );
        let alpha: Challenge = transcript.constraint_phase::<Com>(trace_commit.clone(), None, public);
        lap("constraint phase");
        let steps = self.quotient_tree(
            &[air],
            &prog,
            &trace_tree.mat,
            air.width(),
            &[public.to_vec()],
            log_degree,
            log_chunks,
            alpha,
        );
        let quotient_tree = match steps {
            Ok(t) => t,
            Err(e) => {
                transcript.abort();
                return Err(e);
            }
        };
        let commitments =
            Commitments { trace: trace_commit, quotient_chunks: quotient_tree.commitment(), random: None };
        lap("quotient commit");

        let (zeta, ood_pow_witness) = transcript.ood_phase::<Com>(commitments.quotient_chunks.clone(), None);
        let ood_pow_witness = ood_pow_witness.unwrap_or(Val::ZERO);
        let zeta_next = trace_domain.next_point(zeta).ok_or("next point")?;
        let main_next = !air.main_next_row_columns().is_empty();
        let trace_points = if main_next { vec![zeta, zeta_next] } else { vec![zeta] };
        let rounds = vec![(&trace_tree, vec![trace_points]), (&quotient_tree, vec![vec![zeta]; num_quotient_chunks])];
        let opening = transcript.delegate(|ch| resident::open(&self.fri, &self.mmcs, rounds, ch));
        let (opened, fri_gpu) = match opening {
            Ok(o) => o,
            Err(e) => {
                transcript.abort();
                return Err(e);
            }
        };
        transcript.finish();
        lap("open");

        let layout = StarkOpeningLayout::new(false);
        let opened_values = OpenedValues {
            trace_local: opened[layout.trace][0][0].clone(),
            trace_next: main_next.then(|| opened[layout.trace][0][1].clone()),
            preprocessed: None,
            quotient_chunks: opened[layout.quotient].iter().map(|v| v[0].clone()).collect(),
            random: None,
        };
        Ok(Proof {
            commitments,
            opened_values,
            opening_proof: cpu_fri_proof(fri_gpu),
            degree_bits: log_degree,
            ood_pow_witness,
        })
    }

    fn prove_batch_inner<A>(
        &self,
        instances: Vec<GpuInstance<'_, A>>,
        degree: usize,
    ) -> Result<BatchProof<Config>, String>
    where
        A: Air<SymbolicAirBuilder<Val>> + BaseAir<Val> + Sync,
    {
        use p3_batch_stark::proof::OpenedValuesWithLookups;
        let lap = profiler(std::env::var_os("UMIS_GPU_PROFILE").is_some(), "batch");
        let n = instances.len();
        if n == 0 {
            return Err("empty batch".into());
        }
        let log_degree = degree.trailing_zeros() as usize;
        let lde_height = degree << self.fri.log_blowup;
        if instances.iter().any(|i| i.lde.height != lde_height || i.lde.width != i.air.width()) {
            return Err("batch instances need one height and their AIR's width".into());
        }
        if instances.iter().any(|i| i.key != instances[0].key) {
            return Err("batch instances need one AIR shape (key)".into());
        }
        let progs = [self.program(instances[0].air, instances[0].key, log_degree)?];
        let trace_domain = TwoAdicMultiplicativeCoset::new(Val::ONE, log_degree).ok_or("trace domain")?;
        let airs: Vec<&A> = instances.iter().map(|i| i.air).collect();
        let publics: Vec<Vec<Val>> = instances.iter().map(|i| i.public.clone()).collect();
        let pub_vals: Vec<&[Val]> = publics.iter().map(|p| p.as_slice()).collect();
        let shape = BatchShape {
            trace_widths: airs.iter().map(|a| a.width()).collect(),
            public_value_counts: pub_vals.iter().map(|p| p.len()).collect(),
            preprocessed_widths: vec![0; n],
            has_preprocessed_commitment: false,
            num_lookup_instances: 0,
            lookup_pow_bits: self.cfg.lookup_proof_of_work_bits(),
            has_randomization_commitment: false,
            ood_pow_bits: self.cfg.ood_proof_of_work_bits(),
        };
        let mut challenger = self.cfg.initialise_challenger();
        let mut transcript = BatchProverTranscript::<_, Val, Challenge, Com>::new(&mut challenger, shape);
        transcript.instance_bindings(&vec![log_degree; n]);
        let main_tree = match resident::commit_mats(instances.into_iter().map(|i| i.lde).collect()) {
            Ok(t) => t,
            Err(e) => {
                transcript.abort();
                return Err(e);
            }
        };
        lap("trace commit");
        let main_commit = main_tree.commitment();
        transcript.main_phase(main_commit.clone(), &pub_vals);
        transcript.preprocessed_phase(None);
        // No instance declares a lookup: the lookup phase draws and grinds nothing.
        let alpha: Challenge = transcript.permutation_phase(None, &[]);

        let (prog, lc) = &progs[0];
        let nch = 1usize << lc;
        let ranges: Vec<std::ops::Range<usize>> = (0..n).map(|k| k * nch..(k + 1) * nch).collect();
        let width = airs[0].width();
        let steps = self
            .quotient_tree(&airs, prog, &main_tree.mat, width, &publics, log_degree, *lc, alpha)
            .map(|t| (t, ranges));
        lap("quotients");
        let (quotient_tree, ranges) = match steps {
            Ok(t) => t,
            Err(e) => {
                transcript.abort();
                return Err(e);
            }
        };
        let quotient_commit = quotient_tree.commitment();
        lap("quotient commit");
        transcript.quotient_phase(quotient_commit.clone(), None);
        let (zeta, ood_pow_witness): (Challenge, Val) = transcript.ood_phase();
        let zeta_next = trace_domain.next_point(zeta).ok_or("next point")?;
        let nexts: Vec<bool> = airs.iter().map(|a| !a.main_next_row_columns().is_empty()).collect();
        let trace_points: Vec<Vec<Challenge>> =
            nexts.iter().map(|&nx| if nx { vec![zeta, zeta_next] } else { vec![zeta] }).collect();
        let n_chunks = ranges.last().map_or(0, |r| r.end);
        let rounds = vec![(&main_tree, trace_points), (&quotient_tree, vec![vec![zeta]; n_chunks])];
        let opening = transcript.delegate(|ch| resident::open(&self.fri, &self.mmcs, rounds, ch));
        let (opened, fri_gpu) = match opening {
            Ok(o) => o,
            Err(e) => {
                transcript.abort();
                return Err(e);
            }
        };
        transcript.finish();
        lap("open");

        let layout = StarkOpeningLayout::new(false);
        let instances = (0..n)
            .map(|k| OpenedValuesWithLookups {
                base_opened_values: OpenedValues {
                    trace_local: opened[layout.trace][k][0].clone(),
                    trace_next: nexts[k].then(|| opened[layout.trace][k][1].clone()),
                    preprocessed: None,
                    quotient_chunks: ranges[k].clone().map(|c| opened[layout.quotient][c][0].clone()).collect(),
                    random: None,
                },
                permutation_local: vec![],
                permutation_next: vec![],
            })
            .collect();
        Ok(BatchProof {
            commitments: BatchCommitments {
                main: main_commit,
                permutation: None,
                quotient_chunks: quotient_commit,
                random: None,
            },
            opened_values: BatchOpenedValues { instances },
            opening_proof: cpu_fri_proof(fri_gpu),
            lookup_terminals: vec![None; n],
            degree_bits: vec![log_degree; n],
            lookup_pow_witness: None,
            ood_pow_witness,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use p3_air::{AirBuilder, WindowAccess};
    use std::borrow::Cow;

    /// Fibonacci with boundary constraints on public values and a periodic column:
    /// a' = b, b' = a + b (transition), a = pub0, b = pub1 (first row), b = pub2 (last row),
    /// and c = a * per (c a third column, per a periodic column of period 4).
    #[derive(Clone)]
    struct Fib {
        per: Vec<Vec<Val>>,
    }

    impl BaseAir<Val> for Fib {
        fn width(&self) -> usize {
            3
        }
        fn num_public_values(&self) -> usize {
            3
        }
        fn num_periodic_columns(&self) -> usize {
            self.per.len()
        }
        fn periodic_columns(&self) -> Cow<'_, [Vec<Val>]> {
            Cow::Borrowed(&self.per)
        }
    }

    impl<AB: AirBuilder<F = Val>> Air<AB> for Fib {
        fn eval(&self, builder: &mut AB) {
            let main = builder.main();
            let (cur, next) = (main.current_slice(), main.next_slice());
            let (a, b, c): (AB::Expr, AB::Expr, AB::Expr) = (cur[0].into(), cur[1].into(), cur[2].into());
            let (an, bn): (AB::Expr, AB::Expr) = (next[0].into(), next[1].into());
            let pubs: Vec<AB::Expr> = builder.public_values().iter().map(|x| (*x).into()).collect();
            let per: AB::Expr = builder.periodic_values()[0].into();
            builder.when_first_row().assert_eq(a.clone(), pubs[0].clone());
            builder.when_first_row().assert_eq(b.clone(), pubs[1].clone());
            builder.when_transition().assert_eq(an, b.clone());
            builder.when_transition().assert_eq(bn, a.clone() + b.clone());
            builder.when_last_row().assert_eq(b, pubs[2].clone());
            builder.assert_eq(c, a * per);
        }
    }

    fn fib_trace(n: usize, per: &[Val]) -> (RowMajorMatrix<Val>, Vec<Val>) {
        let (mut a, mut b) = (Val::ZERO, Val::ONE);
        let mut v = Vec::new();
        for i in 0..n {
            v.extend([a, b, a * per[i % per.len()]]);
            (a, b) = (b, a + b);
        }
        let last = v[3 * (n - 1) + 1];
        (RowMajorMatrix::new(v, 3), vec![Val::ZERO, Val::ONE, last])
    }

    #[test]
    fn generic_gpu_proofs_verify_with_the_cpu_verifier() {
        if !crate::gpu_available() {
            eprintln!("no GPU; skipped");
            return;
        }
        let g = GpuStarkProver::new(FriPreset::Benchmark);
        let interp = GpuStarkProver::new(FriPreset::Benchmark).interpreted();
        let cpu = config(FriPreset::Benchmark);
        let period: Vec<Val> = [3u64, 1, 4, 1].iter().map(|&x| Val::from_u64(x)).collect();
        for log_n in [3, 6, 10] {
            let air = Fib { per: vec![period.clone()] };
            let (trace, public) = fib_trace(1 << log_n, &period);
            let proof = g.prove(&air, 1000 + log_n as u64, &trace, &public).unwrap();
            p3_uni_stark::verify(&cpu, &air, &proof, &public).unwrap_or_else(|e| panic!("2^{log_n}: {e:?}"));
            // the compiled and the interpreted kernel give the same proof
            let p2 = interp.prove(&air, 1000 + log_n as u64, &trace, &public).unwrap();
            assert_eq!(format!("{:?}", proof.commitments), format!("{:?}", p2.commitments), "2^{log_n}");
            assert_eq!(format!("{:?}", proof.opened_values), format!("{:?}", p2.opened_values), "2^{log_n}");
            // a false public value is rejected
            let mut bad = public.clone();
            bad[2] += Val::ONE;
            let forged = g.prove(&air, 1000 + log_n as u64, &trace, &bad).unwrap();
            assert!(p3_uni_stark::verify(&cpu, &air, &forged, &bad).is_err());
            eprintln!("Fibonacci 2^{log_n}: generic GPU proof verifies, false claim rejected");
        }
    }
}
