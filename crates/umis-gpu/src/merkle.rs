//! Poseidon2 Merkle trees on the GPU: the [`TreeBackend`] of `umis_stark::fast_mmcs`, with the
//! stock KoalaBear tree (`ValMmcs`: `PaddingFreeSponge<16, 8, 8>` leaves, `TruncatedPermutation`
//! nodes, arity 2). The rows go up once, every layer is built on the device, the layers come
//! back for the openings. Trees below `UMIS_GPU_TREE_MIN` cells (default 2^16) stay on the CPU.

use crate::lde::{device, from_raw_vec, launch_cfg};
use cudarc::driver::{CudaSlice, CudaStream, PushKernelArg};
use std::sync::Arc;
use umis_stark::config::koala_bear::{ROW_CHUNK, Val};
use umis_stark::fast_mmcs::{DIGEST, Digest, Group, TreeBackend};

/// The GPU tree builder (falls back to the CPU reference when no device is usable).
#[derive(Clone, Debug)]
pub struct GpuTree {
    min_cells: usize,
    cpu: umis_stark::fast_mmcs::CpuTree,
}

impl Default for GpuTree {
    fn default() -> Self {
        let min_cells = std::env::var("UMIS_GPU_TREE_MIN").ok().and_then(|v| v.parse().ok()).unwrap_or(1 << 16);
        Self { min_cells, cpu: umis_stark::fast_mmcs::CpuTree::new(0) }
    }
}

impl GpuTree {
    /// Builds every tree of at least `min_cells` cells on the GPU (tests: 0).
    pub fn with_min_cells(min_cells: usize) -> Self {
        Self { min_cells, cpu: umis_stark::fast_mmcs::CpuTree::new(0) }
    }
}

fn digests(words: Vec<u32>) -> Vec<Digest> {
    from_raw_vec(words).chunks_exact(DIGEST).map(|c| c.try_into().expect("digest")).collect()
}

struct Dev<'a> {
    s: Arc<CudaStream>,
    d: &'a crate::lde::Device,
    rc: CudaSlice<u32>,
    /// Pinned staging buffers taken from the device's pool for this tree (returned after the
    /// last, synchronous download, so no DMA reads one that is being refilled).
    staging: std::cell::RefCell<Vec<cudarc::driver::PinnedHostSlice<u32>>>,
    pool: std::cell::RefCell<std::collections::HashMap<usize, Vec<cudarc::driver::PinnedHostSlice<u32>>>>,
    /// Time spent (`UMIS_GPU_TRACE`): host copy into staging, DMA enqueue, downloads.
    t_copy: std::cell::Cell<std::time::Duration>,
    t_down: std::cell::Cell<std::time::Duration>,
    t_up: std::cell::Cell<std::time::Duration>,
    t_hash: std::cell::Cell<std::time::Duration>,
    trace: bool,
}

type E = cudarc::driver::DriverError;

impl Dev<'_> {
    fn upload(&self, v: &[Val]) -> Result<CudaSlice<u32>, E> {
        use rayon::prelude::*;
        // SAFETY: `Val` is `repr(transparent)` over its Montgomery u32 (as `lde::raw`).
        let words: &[u32] = unsafe { std::slice::from_raw_parts(v.as_ptr().cast::<u32>(), v.len()) };
        if words.len() < 1 << 16 {
            return self.s.clone_htod(if words.is_empty() { &[0u32][..] } else { words });
        }
        let n = words.len();
        let mut pin = match self.pool.borrow_mut().get_mut(&n).and_then(Vec::pop) {
            Some(p) => p,
            // SAFETY: the buffer is written in full below before any read.
            None => unsafe { self.d.ctx.alloc_pinned::<u32>(n)? },
        };
        let t = std::time::Instant::now();
        pin.as_mut_slice()?
            .par_chunks_mut(1 << 16)
            .zip(words.par_chunks(1 << 16))
            .for_each(|(d, s)| d.copy_from_slice(s));
        self.t_copy.set(self.t_copy.get() + t.elapsed());
        let mut dst = self.s.alloc_zeros::<u32>(n)?;
        self.s.memcpy_htod(&pin, &mut dst)?;
        self.staging.borrow_mut().push(pin);
        Ok(dst)
    }

    /// A group's concatenated rows on the device (a single matrix goes up as it is).
    fn group_rows(&self, g: &Group<'_>) -> Result<CudaSlice<u32>, E> {
        if g.mats.len() == 1 {
            return self.upload(g.mats[0].1);
        }
        let (w, h) = (g.width(), g.height);
        let mut dst = self.s.alloc_zeros::<u32>((h * w).max(1))?;
        let mut off = 0u32;
        for (mw, v) in &g.mats {
            let src = self.upload(v)?;
            let (sw, rows, dw) = (*mw as u32, h as u32, w as u32);
            // SAFETY: `src` holds `h × mw` words, `dst` `h × w` with `off + mw ≤ w`.
            unsafe {
                self.s
                    .launch_builder(&self.d.concat_cols)
                    .arg(&src)
                    .arg(&sw)
                    .arg(&rows)
                    .arg(&mut dst)
                    .arg(&dw)
                    .arg(&off)
                    .launch(launch_cfg((h * mw) as u64))?;
            }
            off += *mw as u32;
        }
        Ok(dst)
    }

    /// The leaf hash (`WideRowHash`) of every row: the sponge, or for rows longer than
    /// `ROW_CHUNK` the sponge over the rows' chunk digests (chunks hashed in parallel).
    fn hash_rows(&self, g: &Group<'_>) -> Result<CudaSlice<u32>, E> {
        let t = std::time::Instant::now();
        let r = self.hash_rows_inner(g);
        if self.trace {
            self.s.synchronize()?;
            self.t_hash.set(self.t_hash.get() + t.elapsed());
        }
        r
    }

    fn hash_rows_inner(&self, g: &Group<'_>) -> Result<CudaSlice<u32>, E> {
        let (h, width) = (g.height, g.width());
        let t = std::time::Instant::now();
        let m = self.group_rows(g)?;
        if self.trace {
            self.s.synchronize()?;
            self.t_up.set(self.t_up.get() + t.elapsed());
        }
        if width > ROW_CHUNK {
            let nchunks = width.div_ceil(ROW_CHUNK);
            let mut d = self.s.alloc_zeros::<u32>(h * nchunks * DIGEST)?;
            let (w, n, c, k) = (width as u64, h as u32, ROW_CHUNK as u32, nchunks as u32);
            // SAFETY: `m` holds `h` rows of `width` words, `d` `h·nchunks` digests.
            unsafe {
                self.s
                    .launch_builder(&self.d.p2_hash_chunks)
                    .arg(&m)
                    .arg(&w)
                    .arg(&n)
                    .arg(&c)
                    .arg(&k)
                    .arg(&self.rc)
                    .arg(&mut d)
                    .launch(launch_cfg((h * nchunks) as u64))?;
            }
            return self.sponge_rows(&d, nchunks * DIGEST, h);
        }
        self.sponge_rows(&m, width, h)
    }

    /// The plain sponge of each of `h` rows of `width` words.
    fn sponge_rows(&self, m: &CudaSlice<u32>, width: usize, h: usize) -> Result<CudaSlice<u32>, E> {
        let mut out = self.s.alloc_zeros::<u32>(h * DIGEST)?;
        let (w, n) = (width as u64, h as u32);
        // SAFETY: `m` holds `h` rows of `width` words, `out` `h` digests, `rc` the constants.
        unsafe {
            self.s
                .launch_builder(&self.d.p2_hash_rows)
                .arg(m)
                .arg(&w)
                .arg(&n)
                .arg(&self.rc)
                .arg(&mut out)
                .launch(launch_cfg(n as u64))?;
        }
        Ok(out)
    }

    /// out[i] = C(a[i·sa], b[i·sb]) for `n` outputs (strides in words).
    fn compress(
        &self,
        a: &CudaSlice<u32>,
        sa: u32,
        a_off: usize,
        b: &CudaSlice<u32>,
        sb: u32,
        b_off: usize,
        n: usize,
    ) -> Result<CudaSlice<u32>, E> {
        let mut out = self.s.alloc_zeros::<u32>(n * DIGEST)?;
        let (av, bv) = (a.slice(a_off..), b.slice(b_off..));
        let n32 = n as u32;
        // SAFETY: `a` and `b` hold `n` digests at their strides, `out` `n` digests.
        unsafe {
            self.s
                .launch_builder(&self.d.p2_compress)
                .arg(&av)
                .arg(&sa)
                .arg(&bv)
                .arg(&sb)
                .arg(&self.rc)
                .arg(&mut out)
                .arg(&n32)
                .launch(launch_cfg(n as u64))?;
        }
        Ok(out)
    }

    fn layers(&self, groups: &[Group<'_>]) -> Result<Vec<Vec<Digest>>, E> {
        let trace = std::env::var("UMIS_GPU_TRACE").is_ok();
        let t0 = std::time::Instant::now();
        let r = self.layers_inner(groups);
        if trace {
            self.s.synchronize()?;
            let shapes: Vec<(usize, usize)> = groups.iter().map(|g| (g.height, g.width())).collect();
            eprintln!(
                "umis-gpu tree {shapes:?}: {:.1} ms (staging copy {:.1}, upload incl. copy {:.1}, hash incl. upload {:.1}, layer downloads {:.1} ms)",
                t0.elapsed().as_secs_f64() * 1e3,
                self.t_copy.get().as_secs_f64() * 1e3,
                self.t_up.get().as_secs_f64() * 1e3,
                self.t_hash.get().as_secs_f64() * 1e3,
                self.t_down.get().as_secs_f64() * 1e3
            );
        }
        r
    }

    fn layers_inner(&self, groups: &[Group<'_>]) -> Result<Vec<Vec<Digest>>, E> {
        let mut cur = self.hash_rows(&groups[0])?;
        let mut len = cur.len() / DIGEST;
        let mut layers = vec![digests(self.s.clone_dtoh(&cur)?)];
        let mut next_group = 1;
        while len > 1 {
            let half = len / 2;
            let mut folded = self.compress(&cur, 16, 0, &cur, 16, 8, half)?;
            if let Some(g) = groups.get(next_group)
                && g.height == half
            {
                let inj = self.hash_rows(g)?;
                folded = self.compress(&folded, 8, 0, &inj, 8, 0, half)?;
                next_group += 1;
            }
            let t = std::time::Instant::now();
            layers.push(digests(self.s.clone_dtoh(&folded)?));
            self.t_down.set(self.t_down.get() + t.elapsed());
            cur = folded;
            len = half;
        }
        Ok(layers)
    }
}

impl TreeBackend for GpuTree {
    fn wants(&self, cells: usize) -> bool {
        cells >= self.min_cells
    }

    fn layers(&self, groups: &[Group<'_>]) -> Vec<Vec<Digest>> {
        // One thread per row or chunk, and the hash kernels are not grid-strided (`launch_cfg`).
        let max_threads = 65_535 * 256;
        let fits = groups.iter().all(|g| g.height * g.width().div_ceil(ROW_CHUNK).max(1) <= max_threads);
        let gpu = fits.then(device).flatten().and_then(|dev| {
            let d = dev.lock().ok()?;
            let s = d.stream.clone();
            let mut d = d;
            let rc = s.clone_htod(&crate::pow::constants()).ok()?;
            let pool = std::mem::take(&mut d.pinned);
            let dev = Dev {
                s,
                d: &d,
                rc,
                staging: Default::default(),
                pool: std::cell::RefCell::new(pool),
                t_copy: Default::default(),
                t_down: Default::default(),
                t_up: Default::default(),
                t_hash: Default::default(),
                trace: std::env::var("UMIS_GPU_TRACE").is_ok(),
            };
            let r = dev.layers(groups);
            // Every DMA is done: the last layer's download is synchronous.
            let _ = dev.s.synchronize();
            let (staging, mut pool) = (dev.staging.into_inner(), dev.pool.into_inner());
            for p in staging {
                pool.entry(p.len()).or_default().push(p);
            }
            d.pinned = pool;
            r.map_err(|e| eprintln!("umis-gpu: tree on the CPU ({e})")).ok()
        });
        gpu.unwrap_or_else(|| self.cpu.layers(groups))
    }
}
