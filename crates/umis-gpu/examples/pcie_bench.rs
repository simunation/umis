//! Host<->device bandwidth: `pcie_bench [device] [MB] [reps]`, pageable and pinned.
use cudarc::driver::CudaContext;
use std::time::Instant;
fn main() {
    let a: Vec<usize> = std::env::args().skip(1).filter_map(|x| x.parse().ok()).collect();
    let (dev, mb, reps) =
        (a.first().copied().unwrap_or(0), a.get(1).copied().unwrap_or(256), a.get(2).copied().unwrap_or(10));
    let n = mb << 18; // u32 words
    let ctx = CudaContext::new(dev).unwrap();
    println!("device {dev}: {}", ctx.name().unwrap());
    let s = ctx.default_stream();
    let host = vec![7u32; n];
    let mut back = vec![0u32; n];
    let mut d = s.alloc_zeros::<u32>(n).unwrap();
    let mut pin = unsafe { ctx.alloc_pinned_with_flags::<u32>(n, 0).unwrap() };
    pin.as_mut_slice().unwrap().copy_from_slice(&host);
    let gb = (n * 4) as f64 / 1e9;
    let rate = |f: &mut dyn FnMut()| {
        let t = Instant::now();
        for _ in 0..reps {
            f();
        }
        s.synchronize().unwrap();
        gb * reps as f64 / t.elapsed().as_secs_f64()
    };
    let up = rate(&mut || s.memcpy_htod(&host, &mut d).unwrap());
    let down = rate(&mut || s.memcpy_dtoh(&d, &mut back).unwrap());
    println!("pageable {mb} MB x{reps}: up {up:.1} GB/s down {down:.1} GB/s");
    let up = rate(&mut || s.memcpy_htod(&pin, &mut d).unwrap());
    let down = rate(&mut || s.memcpy_dtoh(&d, &mut pin).unwrap());
    println!("pinned   {mb} MB x{reps}: up {up:.1} GB/s down {down:.1} GB/s");
}
