//! Run-time compiled constraint kernels: a [`Program`]'s templates become straight-line
//! CUDA (one SSA value per operation, registers instead of the interpreter's stack),
//! compiled once per program with NVRTC to a CUBIN for the device's architecture.
//! Same parameters and result as the `air_quotient` interpreter, which stays the fallback.

use crate::resident::Program;
use cudarc::driver::{CudaContext, CudaFunction};
use cudarc::nvrtc::{Ptx, result, sys};
use std::fmt::Write;
use std::sync::Arc;

/// Field arithmetic and helpers, as in `cuda/resident.cu`.
const PRELUDE: &str = r#"
typedef unsigned int uint32_t;
typedef unsigned long long uint64_t;
#define P 0x7f000001u
#define MU 0x81000001u
__device__ __forceinline__ uint32_t mred(uint64_t x) {
    uint32_t t = (uint32_t)x * MU;
    uint64_t u = (uint64_t)t * P;
    uint32_t hi = (uint32_t)((x - u) >> 32);
    return x < u ? hi + P : hi;
}
__device__ __forceinline__ uint32_t fmul(uint32_t a, uint32_t b) { return mred((uint64_t)a * b); }
__device__ __forceinline__ uint32_t fadd(uint32_t a, uint32_t b) {
    uint32_t s = a + b;
    return s >= P ? s - P : s;
}
__device__ __forceinline__ uint32_t fsub(uint32_t a, uint32_t b) { return a >= b ? a - b : a + P - b; }
__device__ __forceinline__ uint32_t brev(uint32_t i, uint32_t log_n) { return log_n == 0 ? 0 : (__brev(i) >> (32 - log_n)); }
"#;

/// Above this many template words the interpreter is used (compile time).
const MAX_WORDS: usize = 200_000;

/// How an instance's argument is obtained: `base + stride * j`, or loaded.
enum Arg {
    Affine(u32, u32),
    Load,
}

fn classify(vals: &[u32]) -> Arg {
    let base = vals[0];
    let stride = if vals.len() > 1 { vals[1].wrapping_sub(base) } else { 0 };
    if vals.iter().enumerate().all(|(j, &v)| v == base.wrapping_add(stride.wrapping_mul(j as u32))) {
        Arg::Affine(base, stride)
    } else {
        Arg::Load
    }
}

/// CUDA source of the program's quotient kernel `air_quotient_jit`.
pub fn source(prog: &Program) -> Result<String, String> {
    let mut s = String::from(PRELUDE);
    s.push_str(
        r#"
extern "C" __global__ void air_quotient_jit(const uint32_t* __restrict__ lde, uint32_t width, uint32_t next_step,
        const uint32_t* __restrict__ per, uint32_t n_per, uint32_t per_brev, const uint32_t* __restrict__ pub,
        const uint32_t* sel_first, const uint32_t* sel_last, const uint32_t* sel_trans, const uint32_t* inv_van,
        const uint32_t* __restrict__ apw, const uint32_t* __restrict__ args, uint32_t log_q, uint32_t stride,
        uint32_t n_pub, uint32_t log_chunks, uint32_t* out) {
    __shared__ uint32_t sh[4][256];
    uint32_t q = 1u << log_q;
    uint32_t i = blockIdx.x, k = blockIdx.y, m = gridDim.y;
    if (i >= q) return;
    const uint32_t* __restrict__ cur = lde + (uint64_t)brev(i, log_q) * stride + (uint64_t)k * width;
    const uint32_t* __restrict__ nxt = lde + (uint64_t)brev((i + next_step) & (q - 1), log_q) * stride + (uint64_t)k * width;
    const uint32_t* __restrict__ pv =
        n_per ? per + (uint64_t)(per_brev ? brev(i, log_q) : i) * n_per * m + (uint64_t)k * n_per : per;
    pub += (uint64_t)k * n_pub;
    uint32_t f = sel_first[i], l = sel_last[i], t = sel_trans[i];
    (void)f; (void)l; (void)t; (void)pv; (void)nxt; (void)pub;
    uint32_t acc0 = 0, acc1 = 0, acc2 = 0, acc3 = 0;
"#,
    );
    let e = |x: std::fmt::Error| x.to_string();
    for g in prog.groups.chunks_exact(5) {
        let (c0, c1, na, ni, off) = (g[0] as usize, g[1] as usize, g[2] as usize, g[3] as usize, g[4] as usize);
        let row = |a: usize| &prog.args[off + a * ni..off + (a + 1) * ni];
        let arg = |a: usize| match classify(row(a)) {
            Arg::Affine(b, 0) => format!("{b}u"),
            Arg::Affine(b, st) => format!("({b}u + {st}u * j)"),
            Arg::Load => format!("ga[{}u + j]", a * ni),
        };
        writeln!(s, "    {{ const uint32_t* __restrict__ ga = args + {off}u;").map_err(e)?;
        writeln!(s, "    for (uint32_t j = threadIdx.x; j < {ni}u; j += blockDim.x) {{").map_err(e)?;
        let mut stack: Vec<String> = Vec::new();
        let mut slots: Vec<Option<String>> = Vec::new();
        let mut n = 0usize;
        let mut fresh = |s: &mut String, expr: String| -> Result<String, String> {
            let v = format!("v{n}");
            n += 1;
            writeln!(s, "        uint32_t {v} = {expr};").map_err(e)?;
            Ok(v)
        };
        for &w in &prog.code[c0..c1] {
            let (op, x) = (w & 15, (w >> 4) as usize);
            let pop = |st: &mut Vec<String>| st.pop().ok_or_else(|| "stack underflow".to_string());
            match op {
                0 => stack.push(fresh(&mut s, format!("cur[{}]", arg(x)))?),
                1 => stack.push(fresh(&mut s, format!("nxt[{}]", arg(x)))?),
                2 => stack.push(fresh(&mut s, format!("pv[{}]", arg(x)))?),
                3 => stack.push(fresh(&mut s, format!("pub[{}]", arg(x)))?),
                4 => stack.push(fresh(&mut s, arg(x))?),
                5 => stack.push("f".into()),
                6 => stack.push("l".into()),
                7 => stack.push("t".into()),
                8 | 9 | 11 => {
                    let y = pop(&mut stack)?;
                    let a = pop(&mut stack)?;
                    let f = ["fadd", "fsub", "", "fmul"][op as usize - 8];
                    stack.push(fresh(&mut s, format!("{f}({a}, {y})"))?);
                }
                10 => {
                    let a = pop(&mut stack)?;
                    stack.push(fresh(&mut s, format!("fsub(0u, {a})"))?);
                }
                12 => {
                    if slots.len() <= x {
                        slots.resize(x + 1, None);
                    }
                    slots[x] = Some(stack.last().ok_or("store on empty stack")?.clone());
                }
                13 => stack.push(slots.get(x).cloned().flatten().ok_or("load of an unset slot")?),
                _ => return Err(format!("unknown opcode {op}")),
            }
        }
        let v = stack.pop().ok_or("empty constraint")?;
        writeln!(s, "        uint32_t c = {};", arg(na)).map_err(e)?;
        writeln!(
            s,
            "        acc0 = fadd(acc0, fmul({v}, apw[4 * c])); acc1 = fadd(acc1, fmul({v}, apw[4 * c + 1]));\n        \
             acc2 = fadd(acc2, fmul({v}, apw[4 * c + 2])); acc3 = fadd(acc3, fmul({v}, apw[4 * c + 3]));"
        )
        .map_err(e)?;
        s.push_str("    } }\n");
    }
    s.push_str(
        r#"    sh[0][threadIdx.x] = acc0; sh[1][threadIdx.x] = acc1; sh[2][threadIdx.x] = acc2; sh[3][threadIdx.x] = acc3;
    __syncthreads();
    for (uint32_t s2 = blockDim.x / 2; s2 > 0; s2 >>= 1) {
        if (threadIdx.x < s2)
            for (int d = 0; d < 4; d++) sh[d][threadIdx.x] = fadd(sh[d][threadIdx.x], sh[d][threadIdx.x + s2]);
        __syncthreads();
    }
    if (threadIdx.x == 0) {
        uint32_t iv = inv_van[i], j = i & ((1u << log_chunks) - 1), row = i >> log_chunks;
        uint64_t o = (((uint64_t)j * (q >> log_chunks) + row) * m + k) * 4;
        for (int d = 0; d < 4; d++) out[o + d] = fmul(sh[d][0], iv);
    }
}
"#,
    );
    Ok(s)
}

/// Compiles `src` to a CUBIN for the context's device.
fn cubin(ctx: &CudaContext, src: &str) -> Result<Vec<u8>, String> {
    let (major, minor) = ctx.compute_capability().map_err(|e| e.to_string())?;
    let csrc = std::ffi::CString::new(src).map_err(|e| e.to_string())?;
    let prog = result::create_program(&csrc, None).map_err(|e| format!("nvrtc: {e:?}"))?;
    let opts = [format!("--gpu-architecture=sm_{major}{minor}"), "-default-device".into()];
    // SAFETY: `prog` was created above and is destroyed once below.
    unsafe {
        let r = result::compile_program(prog, &opts);
        let out = match r {
            Ok(()) => {
                let mut n = 0usize;
                sys::nvrtcGetCUBINSize(prog, &mut n).result().map_err(|e| format!("nvrtc: {e:?}"))?;
                let mut buf = vec![0u8; n];
                sys::nvrtcGetCUBIN(prog, buf.as_mut_ptr().cast()).result().map_err(|e| format!("nvrtc: {e:?}"))?;
                Ok(buf)
            }
            Err(e) => {
                let log = result::get_program_log(prog).ok().map(|l| {
                    std::ffi::CStr::from_ptr(l.as_ptr()).to_string_lossy().chars().take(2000).collect::<String>()
                });
                Err(format!("nvrtc: {e:?}: {}", log.unwrap_or_default()))
            }
        };
        let _ = result::destroy_program(prog);
        out
    }
}

/// The compiled kernel of `prog`, or why there is none.
pub(crate) fn kernel(ctx: &Arc<CudaContext>, prog: &Program) -> Result<CudaFunction, String> {
    if !prog.allow_jit {
        return Err("disabled".into());
    }
    if prog.code.len() > MAX_WORDS {
        return Err(format!("{} template words", prog.code.len()));
    }
    let t = std::time::Instant::now();
    let src = source(prog)?;
    let bin = cubin(ctx, &src)?;
    let m = ctx.load_module(Ptx::from_binary(bin)).map_err(|e| e.to_string())?;
    let f = m.load_function("air_quotient_jit").map_err(|e| e.to_string())?;
    if std::env::var_os("UMIS_GPU_PROFILE").is_some() {
        eprintln!("  jit: {} lines compiled in {:?}", src.lines().count(), t.elapsed());
    }
    Ok(f)
}
