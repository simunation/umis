//! GPU coset LDE for KoalaBear (`cuda/lde.cu`).

use cudarc::driver::{CudaContext, CudaFunction, CudaSlice, CudaStream, LaunchConfig, PushKernelArg};
use cudarc::nvrtc::Ptx;
use p3_dft::{Radix2DitParallel, TwoAdicSubgroupDft};
use p3_field::{Field, PrimeCharacteristicRing, TwoAdicField};
use p3_koala_bear::KoalaBear;
use p3_matrix::Matrix;
use p3_matrix::bitrev::{BitReversalPerm, BitReversedMatrixView};
use p3_matrix::dense::RowMajorMatrix;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

type F = KoalaBear;

const CUBIN: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/lde.cubin"));
const CUBIN_RESIDENT: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/resident.cubin"));

/// The GPU: one context and stream, every kernel, cached twiddles.
pub(crate) struct Device {
    pub(crate) stream: Arc<CudaStream>,
    dif: CudaFunction,
    unpermute: CudaFunction,
    pub(crate) leaf_hash: CudaFunction,
    pub(crate) copy_block: CudaFunction,
    pub(crate) p2_grind: CudaFunction,
    pub(crate) p2_permute_states: CudaFunction,
    pub(crate) p2_hash_rows: CudaFunction,
    pub(crate) p2_hash_chunks: CudaFunction,
    pub(crate) concat_cols: CudaFunction,
    pub(crate) p2_compress: CudaFunction,
    pub(crate) compress_layer: CudaFunction,
    pub(crate) gather_rows: CudaFunction,
    pub(crate) ext_col_dot_part: CudaFunction,
    pub(crate) ext_col_dot_sum: CudaFunction,
    pub(crate) ext_row_dot: CudaFunction,
    pub(crate) fri_acc: CudaFunction,
    pub(crate) air_quotient: CudaFunction,
    pub(crate) life_gen0: CudaFunction,
    pub(crate) life_gen_step: CudaFunction,
    pub(crate) life_fill: CudaFunction,
    /// log n -> (w_n^i, w_n^-i) for i < n/2, on the device.
    twiddles: HashMap<usize, (CudaSlice<u32>, CudaSlice<u32>)>,
    /// Pinned host staging buffers by length (Merkle uploads run at full PCIe speed from them).
    pub(crate) pinned: HashMap<usize, Vec<cudarc::driver::PinnedHostSlice<u32>>>,
    pub(crate) ctx: Arc<CudaContext>,
}

pub(crate) fn raw(x: F) -> u32 {
    // SAFETY: `MontyField31` is `repr(transparent)` over its canonical u32 Montgomery value.
    unsafe { std::mem::transmute::<F, u32>(x) }
}

fn device_index() -> Option<usize> {
    let want = std::env::var("UMIS_GPU").unwrap_or_else(|_| "A6000".into());
    let n = CudaContext::device_count().ok()? as usize;
    (0..n)
        .find(|&i| CudaContext::new(i).ok().and_then(|c| c.name().ok()).is_some_and(|name| name.contains(&want)))
        .or((n > 0).then_some(0))
}

static DEVICE: OnceLock<Option<Mutex<Device>>> = OnceLock::new();

fn init() -> Result<Device, String> {
    let e = |x: cudarc::driver::DriverError| x.to_string();
    let idx = device_index().ok_or("no CUDA device")?;
    let ctx = CudaContext::new(idx).map_err(e)?;
    let module = ctx.load_module(Ptx::from_binary(CUBIN.to_vec())).map_err(e)?;
    let res = ctx.load_module(Ptx::from_binary(CUBIN_RESIDENT.to_vec())).map_err(e)?;
    Ok(Device {
        stream: ctx.default_stream(),
        dif: module.load_function("dif_stage").map_err(e)?,
        unpermute: module.load_function("scale_unpermute").map_err(e)?,
        leaf_hash: res.load_function("leaf_hash").map_err(e)?,
        copy_block: res.load_function("copy_block").map_err(e)?,
        p2_grind: res.load_function("p2_grind").map_err(e)?,
        p2_permute_states: res.load_function("p2_permute_states").map_err(e)?,
        p2_hash_rows: res.load_function("p2_hash_rows").map_err(e)?,
        p2_hash_chunks: res.load_function("p2_hash_chunks").map_err(e)?,
        concat_cols: res.load_function("concat_cols").map_err(e)?,
        p2_compress: res.load_function("p2_compress").map_err(e)?,
        compress_layer: res.load_function("compress_layer").map_err(e)?,
        gather_rows: res.load_function("gather_rows").map_err(e)?,
        ext_col_dot_part: res.load_function("ext_col_dot_part").map_err(e)?,
        ext_col_dot_sum: res.load_function("ext_col_dot_sum").map_err(e)?,
        ext_row_dot: res.load_function("ext_row_dot").map_err(e)?,
        fri_acc: res.load_function("fri_acc").map_err(e)?,
        air_quotient: res.load_function("air_quotient").map_err(e)?,
        life_gen0: res.load_function("life_gen0").map_err(e)?,
        life_gen_step: res.load_function("life_gen_step").map_err(e)?,
        life_fill: res.load_function("life_fill").map_err(e)?,
        twiddles: HashMap::new(),
        pinned: HashMap::new(),
        ctx: ctx.clone(),
    })
}

pub(crate) fn device() -> Option<&'static Mutex<Device>> {
    DEVICE
        .get_or_init(|| match std::panic::catch_unwind(init) {
            Ok(Ok(d)) => Some(Mutex::new(d)),
            Ok(Err(err)) => {
                eprintln!("umis-gpu: no GPU ({err}); using the CPU");
                None
            }
            Err(_) => {
                eprintln!("umis-gpu: CUDA unavailable (driver library failed to load); using the CPU");
                None
            }
        })
        .as_ref()
}

/// Whether a CUDA device and the kernels are usable.
pub fn gpu_available() -> bool {
    device().is_some()
}

fn min_elems() -> usize {
    static MIN: OnceLock<usize> = OnceLock::new();
    *MIN.get_or_init(|| std::env::var("UMIS_GPU_MIN").ok().and_then(|s| s.parse().ok()).unwrap_or(1 << 18))
}

pub(crate) fn launch_cfg(work: u64) -> LaunchConfig {
    let threads = 256u32;
    let blocks = work.div_ceil(threads as u64).clamp(1, 65_535) as u32;
    LaunchConfig { grid_dim: (blocks, 1, 1), block_dim: (threads, 1, 1), shared_mem_bytes: 0 }
}

impl Device {
    fn twiddles(&mut self, log_n: usize) -> Result<&(CudaSlice<u32>, CudaSlice<u32>), String> {
        if !self.twiddles.contains_key(&log_n) {
            let half = (1usize << log_n) / 2;
            let w = F::two_adic_generator(log_n);
            let wi = w.inverse();
            let pows = |g: F| -> Vec<u32> { g.powers().take(half.max(1)).map(raw).collect() };
            let fwd = self.stream.clone_htod(&pows(w)).map_err(|e| e.to_string())?;
            let inv = self.stream.clone_htod(&pows(wi)).map_err(|e| e.to_string())?;
            self.twiddles.insert(log_n, (fwd, inv));
        }
        Ok(&self.twiddles[&log_n])
    }

    /// In-place DIF NTT of every column (natural in, bit-reversed out).
    fn dif(&mut self, buf: &mut CudaSlice<u32>, log_n: usize, cols: usize, inverse: bool) -> Result<(), String> {
        let n = 1u32 << log_n;
        let (fwd, inv) = self.twiddles(log_n)?;
        let tw = if inverse { inv.clone() } else { fwd.clone() };
        let cols = cols as u32;
        let mut m = n / 2;
        while m >= 1 {
            let cfg = launch_cfg((n as u64 / 2) * cols as u64);
            // SAFETY: argument types match the kernel signature; indices stay inside `buf`.
            unsafe {
                self.stream
                    .launch_builder(&self.dif)
                    .arg(&mut *buf)
                    .arg(&tw)
                    .arg(&n)
                    .arg(&cols)
                    .arg(&m)
                    .launch(cfg)
                    .map_err(|e| e.to_string())?;
            }
            m /= 2;
        }
        Ok(())
    }

    /// Coset LDE of a host matrix (`h x w` raw values, natural rows), left on the
    /// device: `h << added_bits` rows in bit-reversed order.
    pub(crate) fn coset_lde_device(
        &mut self,
        src: &[u32],
        h: usize,
        w: usize,
        added_bits: usize,
        shift: F,
    ) -> Result<CudaSlice<u32>, String> {
        let prof = std::env::var_os("UMIS_GPU_PROFILE").is_some();
        let t0 = std::time::Instant::now();
        let a = self.stream.clone_htod(src).map_err(|e| e.to_string())?;
        if prof {
            self.stream.synchronize().map_err(|e| e.to_string())?;
            eprintln!("umis-gpu lde {h}x{w}: upload {:?}", t0.elapsed());
        }
        self.coset_lde_on_device(a, h, w, added_bits, shift)
    }

    /// Coset LDE of a device matrix (`h x w`, natural rows), consumed.
    pub(crate) fn coset_lde_on_device(
        &mut self,
        mut a: CudaSlice<u32>,
        h: usize,
        w: usize,
        added_bits: usize,
        shift: F,
    ) -> Result<CudaSlice<u32>, String> {
        let log_h = h.trailing_zeros() as usize;
        let out_h = h << added_bits;
        // evaluations on H -> h * coefficients (bit-reversed rows)
        self.dif(&mut a, log_h, w, true)?;
        // natural coefficients * shift^j / h, zero-padded to out_h rows
        let h_inv = F::from_u64(h as u64).inverse();
        let f: Vec<u32> = shift.powers().take(h).map(|s| raw(s * h_inv)).collect();
        let f = self.stream.clone_htod(&f).map_err(|e| e.to_string())?;
        let mut b = self.stream.alloc_zeros::<u32>(out_h * w).map_err(|e| e.to_string())?;
        let (hh, lh, ww) = (h as u32, log_h as u32, w as u32);
        // SAFETY: argument types match the kernel signature; indices stay inside the buffers.
        unsafe {
            self.stream
                .launch_builder(&self.unpermute)
                .arg(&a)
                .arg(&mut b)
                .arg(&f)
                .arg(&hh)
                .arg(&lh)
                .arg(&ww)
                .launch(launch_cfg(h as u64 * w as u64))
                .map_err(|e| e.to_string())?;
        }
        // coefficients -> evaluations on shift * K (bit-reversed rows)
        self.dif(&mut b, log_h + added_bits, w, false)?;
        Ok(b)
    }

    /// Coset LDE: rows of the result are in bit-reversed order.
    fn coset_lde(&mut self, mat: &RowMajorMatrix<F>, added_bits: usize, shift: F) -> Result<Vec<F>, String> {
        // SAFETY: repr(transparent) u32 values.
        let src: &[u32] = unsafe { std::slice::from_raw_parts(mat.values.as_ptr() as *const u32, mat.values.len()) };
        let b = self.coset_lde_device(src, mat.height(), mat.width(), added_bits, shift)?;
        let out: Vec<u32> = self.stream.clone_dtoh(&b).map_err(|e| e.to_string())?;
        Ok(from_raw_vec(out))
    }
}

/// `Vec<u32>` of canonical Montgomery values as field elements, without copying.
pub(crate) fn from_raw_vec(v: Vec<u32>) -> Vec<F> {
    let mut v = std::mem::ManuallyDrop::new(v);
    // SAFETY: same size and alignment (repr(transparent)); every value is canonical (< p).
    unsafe { Vec::from_raw_parts(v.as_mut_ptr() as *mut F, v.len(), v.capacity()) }
}

/// `TwoAdicSubgroupDft` whose coset LDE runs on the GPU (other transforms, and
/// small or failing LDEs, use `Radix2DitParallel`).
#[derive(Clone, Default)]
pub struct GpuDft {
    cpu: Radix2DitParallel<F>,
}

impl TwoAdicSubgroupDft<F> for GpuDft {
    type Evaluations = BitReversedMatrixView<RowMajorMatrix<F>>;

    fn dft_batch(&self, mat: RowMajorMatrix<F>) -> Self::Evaluations {
        self.cpu.dft_batch(mat)
    }

    fn coset_lde_batch(&self, mat: RowMajorMatrix<F>, added_bits: usize, shift: F) -> Self::Evaluations {
        if mat.height() >= 2
            && mat.values.len() >= min_elems()
            && let Some(dev) = device()
        {
            let w = mat.width();
            let r = dev.lock().expect("gpu").coset_lde(&mat, added_bits, shift);
            match r {
                Ok(values) => return BitReversalPerm::new_view(RowMajorMatrix::new(values, w)),
                Err(e) => eprintln!("umis-gpu: coset LDE failed ({e}); using the CPU"),
            }
        }
        self.cpu.coset_lde_batch(mat, added_bits, shift)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use p3_matrix::bitrev::BitReversibleMatrix;

    #[test]
    fn gpu_lde_equals_cpu() {
        if !gpu_available() {
            eprintln!("no GPU; skipped");
            return;
        }
        let cpu = Radix2DitParallel::<F>::default();
        let mut dev = device().unwrap().lock().unwrap();
        for (log_h, w, bits) in [(1, 3, 1), (4, 5, 1), (8, 17, 1), (10, 64, 2), (11, 300, 1)] {
            let h = 1usize << log_h;
            let vals: Vec<F> =
                (0..h * w).map(|i| F::from_u64((i as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15) >> 33)).collect();
            let mat = RowMajorMatrix::new(vals, w);
            let shift = F::GENERATOR;
            let want = cpu.coset_lde_batch(mat.clone(), bits, shift).bit_reverse_rows().to_row_major_matrix();
            let got = dev.coset_lde(&mat, bits, shift).unwrap();
            assert_eq!(got, want.values, "log_h={log_h} w={w} bits={bits}");
            eprintln!("gpu == cpu: log_h={log_h} w={w} bits={bits}");
        }
    }
}
