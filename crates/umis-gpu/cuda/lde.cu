// KoalaBear (p = 2^31 - 2^24 + 1) coset LDE kernels on row-major matrices
// (rows x cols, every column an independent polynomial).
//
// Values are in Plonky3's canonical Montgomery form (x * 2^32 mod p, in [0, p)):
// `mred` is Plonky3's `monty_reduce`, so every result equals the CPU's bit for bit.

#include <stdint.h>

#define P 0x7f000001u
#define MU 0x81000001u  // p^-1 mod 2^32

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

// One radix-2 decimation-in-frequency stage of size-n NTTs on every column:
// pairs (i0, i0 + m) inside blocks of 2m rows, twiddle w_{2m}^j = tw[j * (n / 2m)],
// tw[i] = w_n^i. Natural-order input after all stages gives bit-reversed output.
extern "C" __global__ void dif_stage(uint32_t* a, const uint32_t* tw, uint32_t n, uint32_t cols, uint32_t m) {
    uint64_t total = (uint64_t)(n / 2) * cols;
    uint32_t stride = n / (2 * m);
    for (uint64_t idx = blockIdx.x * (uint64_t)blockDim.x + threadIdx.x; idx < total;
         idx += (uint64_t)gridDim.x * blockDim.x) {
        uint32_t c = (uint32_t)(idx % cols);
        uint32_t k = (uint32_t)(idx / cols);
        uint32_t j = k % m;
        uint32_t i0 = (k / m) * 2 * m + j;
        uint64_t p0 = (uint64_t)i0 * cols + c;
        uint64_t p1 = p0 + (uint64_t)m * cols;
        uint32_t u = a[p0], v = a[p1];
        a[p0] = fadd(u, v);
        a[p1] = fmul(fsub(u, v), tw[j * stride]);
    }
}

// dst (out_rows x cols, zeroed) row j = src row bitrev_{log_h}(j) * f[j], j < h:
// bit-reversed iDFT output -> natural coefficients scaled by shift^j / h, zero-padded.
extern "C" __global__ void scale_unpermute(const uint32_t* src, uint32_t* dst, const uint32_t* f, uint32_t h,
                                           uint32_t log_h, uint32_t cols) {
    uint64_t total = (uint64_t)h * cols;
    for (uint64_t idx = blockIdx.x * (uint64_t)blockDim.x + threadIdx.x; idx < total;
         idx += (uint64_t)gridDim.x * blockDim.x) {
        uint32_t c = (uint32_t)(idx % cols);
        uint32_t j = (uint32_t)(idx / cols);
        uint32_t q = log_h == 0 ? 0 : (__brev(j) >> (32 - log_h));
        dst[(uint64_t)j * cols + c] = fmul(src[(uint64_t)q * cols + c], f[j]);
    }
}
