// Device-resident proving kernels for KoalaBear STARKs (see src/resident.rs):
// Keccak-f[1600] Merkle trees byte-compatible with Plonky3's
// MerkleTreeMmcs<_, [u64; 4], SerializingHasher<PaddingFreeSponge<KeccakF, 25, 17, 4>>,
// CompressionFunctionFromHasher<_, 2, 4>>, row gathers, out-of-domain dot
// products and alpha-compressed rows over the degree-4 extension, and the
// life2d quotient.
//
// Field values are canonical Montgomery u32 (as Plonky3 stores them).

#include <stdint.h>

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

// ---------------------------------------------------------------- Keccak-f[1600]

__constant__ uint64_t KRC[24] = {
    0x0000000000000001ULL, 0x0000000000008082ULL, 0x800000000000808aULL, 0x8000000080008000ULL,
    0x000000000000808bULL, 0x0000000080000001ULL, 0x8000000080008081ULL, 0x8000000000008009ULL,
    0x000000000000008aULL, 0x0000000000000088ULL, 0x0000000080008009ULL, 0x000000008000000aULL,
    0x000000008000808bULL, 0x800000000000008bULL, 0x8000000000008089ULL, 0x8000000000008003ULL,
    0x8000000000008002ULL, 0x8000000000000080ULL, 0x000000000000800aULL, 0x800000008000000aULL,
    0x8000000080008081ULL, 0x8000000000008080ULL, 0x0000000080000001ULL, 0x8000000080008008ULL};

__device__ __forceinline__ uint64_t rotl(uint64_t x, int n) { return n == 0 ? x : (x << n) | (x >> (64 - n)); }

// Standard Keccak-f[1600], lane (x, y) at s[x + 5y].
__device__ __forceinline__ void keccakf(uint64_t s[25]) {
    constexpr int rho[25] = {0, 1, 62, 28, 27, 36, 44, 6, 55, 20, 3, 10, 43, 25, 39, 41, 45, 15, 21, 8, 18, 2, 61, 56, 14};
    for (int round = 0; round < 24; round++) {
        uint64_t c[5], b[25];
#pragma unroll
        for (int x = 0; x < 5; x++) c[x] = s[x] ^ s[x + 5] ^ s[x + 10] ^ s[x + 15] ^ s[x + 20];
#pragma unroll
        for (int x = 0; x < 5; x++) {
            uint64_t d = c[(x + 4) % 5] ^ rotl(c[(x + 1) % 5], 1);
#pragma unroll
            for (int y = 0; y < 5; y++) s[x + 5 * y] ^= d;
        }
#pragma unroll
        for (int x = 0; x < 5; x++)
#pragma unroll
            for (int y = 0; y < 5; y++) b[y + 5 * ((2 * x + 3 * y) % 5)] = rotl(s[x + 5 * y], rho[x + 5 * y]);
#pragma unroll
        for (int x = 0; x < 5; x++)
#pragma unroll
            for (int y = 0; y < 5; y++) s[x + 5 * y] = b[x + 5 * y] ^ (~b[(x + 1) % 5 + 5 * y] & b[(x + 2) % 5 + 5 * y]);
        s[0] ^= KRC[round];
    }
}

// PaddingFreeSponge<KeccakF, 25, 17, 4>: overwrite the 17 rate words of each block
// (a short last block overwrites only its words), permute after every block,
// squeeze state[0..4]. Blocks are unrolled so the state stays in registers.

// Leaf digests of `rows` rows of the concatenation of two row-major matrices
// (w1 = 0 for one): elements paired into u64 words low-first, an odd last element alone.
extern "C" __global__ void leaf_hash(const uint32_t* m0, uint32_t w0, const uint32_t* m1, uint32_t w1, uint32_t rows,
                                     uint64_t* out) {
    uint32_t r = blockIdx.x * blockDim.x + threadIdx.x;
    if (r >= rows) return;
    const uint32_t* a0 = m0 + (uint64_t)r * w0;
    const uint32_t* a1 = m1 + (uint64_t)r * w1;
    uint32_t total = w0 + w1;
    uint32_t nwords = (total + 1) / 2;
    uint64_t s[25];
#pragma unroll
    for (int i = 0; i < 25; i++) s[i] = 0;
    for (uint32_t base = 0; base < nwords; base += 17) {
#pragma unroll
        for (int k = 0; k < 17; k++) {
            uint32_t wi = base + k;
            if (wi < nwords) {
                uint32_t e = 2 * wi;
                uint32_t x = e < w0 ? a0[e] : a1[e - w0];
                uint64_t word = x;
                if (e + 1 < total) {
                    uint32_t y = e + 1 < w0 ? a0[e + 1] : a1[e + 1 - w0];
                    word |= (uint64_t)y << 32;
                }
                s[k] = word;
            }
        }
        keccakf(s);
    }
    uint64_t* o = out + (uint64_t)r * 4;
    o[0] = s[0];
    o[1] = s[1];
    o[2] = s[2];
    o[3] = s[3];
}

// One tree layer: out[j] = H(in[2j] || in[2j+1]) (8 words, one partial block).
extern "C" __global__ void compress_layer(const uint64_t* in, uint64_t* out, uint32_t n_out) {
    uint32_t j = blockIdx.x * blockDim.x + threadIdx.x;
    if (j >= n_out) return;
    uint64_t s[25];
#pragma unroll
    for (int i = 0; i < 25; i++) s[i] = i < 8 ? in[(uint64_t)j * 8 + i] : 0;
    keccakf(s);
    uint64_t* o = out + (uint64_t)j * 4;
    o[0] = s[0];
    o[1] = s[1];
    o[2] = s[2];
    o[3] = s[3];
}

// out[q][c] = m[idx[q]][c].
extern "C" __global__ void gather_rows(const uint32_t* m, uint32_t width, const uint32_t* idx, uint32_t n, uint32_t* out) {
    uint64_t total = (uint64_t)n * width;
    for (uint64_t t = blockIdx.x * (uint64_t)blockDim.x + threadIdx.x; t < total; t += (uint64_t)gridDim.x * blockDim.x) {
        uint32_t q = (uint32_t)(t / width), c = (uint32_t)(t % width);
        out[t] = m[(uint64_t)idx[q] * width + c];
    }
}

// dst[r][dst_off + c] = src[r][src_off + c] for c < cols, r < rows (row strides given).
extern "C" __global__ void copy_block(const uint32_t* src, uint32_t src_stride, uint32_t src_off, uint32_t* dst,
                                      uint32_t dst_stride, uint32_t dst_off, uint32_t cols, uint32_t rows) {
    uint64_t total = (uint64_t)rows * cols;
    for (uint64_t t = blockIdx.x * (uint64_t)blockDim.x + threadIdx.x; t < total; t += (uint64_t)gridDim.x * blockDim.x) {
        uint32_t r = (uint32_t)(t / cols), c = (uint32_t)(t % cols);
        dst[(uint64_t)r * dst_stride + dst_off + c] = src[(uint64_t)r * src_stride + src_off + c];
    }
}

// ---------------------------------------------------------------- extension dot products

// out[r] (4 coords) = sum_c pw[c] * m[r][c]: one block per row, threads over columns.
extern "C" __global__ void ext_row_dot(const uint32_t* m, uint32_t width, uint32_t rows, const uint32_t* pw, uint32_t* out) {
    __shared__ uint32_t sh[4][256];
    uint32_t r = blockIdx.x;
    if (r >= rows) return;
    uint32_t acc[4] = {0, 0, 0, 0};
    for (uint32_t c = threadIdx.x; c < width; c += blockDim.x) {
        uint32_t v = m[(uint64_t)r * width + c];
#pragma unroll
        for (int d = 0; d < 4; d++) acc[d] = fadd(acc[d], fmul(v, pw[4 * c + d]));
    }
#pragma unroll
    for (int d = 0; d < 4; d++) sh[d][threadIdx.x] = acc[d];
    __syncthreads();
    for (uint32_t s = blockDim.x / 2; s > 0; s >>= 1) {
        if (threadIdx.x < s)
#pragma unroll
            for (int d = 0; d < 4; d++) sh[d][threadIdx.x] = fadd(sh[d][threadIdx.x], sh[d][threadIdx.x + s]);
        __syncthreads();
    }
    if (threadIdx.x == 0)
#pragma unroll
        for (int d = 0; d < 4; d++) out[4 * r + d] = sh[d][0];
}

__device__ __forceinline__ uint32_t brev(uint32_t i, uint32_t log_n) { return log_n == 0 ? 0 : (__brev(i) >> (32 - log_n)); }

// ---------------------------------------------------------------- life2d trace generation

struct LifeTraceShape {
    uint32_t n, k, strips, rpr, w, ws, y, height, rows, ext_w;
    uint8_t band_absent[9];  // absent flag of extended-board band (row band * 3 + column band)
};

__device__ __forceinline__ bool tlocate(const LifeTraceShape& s, uint32_t y, uint32_t* st, uint32_t* yy) {
    y %= s.rows;
    if (y >= s.strips * s.y) return false;
    *st = y / s.y;
    *yy = y % s.y;
    return true;
}

// Generation 0 on the stacked torus: g0[y][x] = extended board cell (dead in padding).
extern "C" __global__ void life_gen0(const uint8_t* ext, const LifeTraceShape s, uint8_t* g0) {
    uint64_t total = (uint64_t)s.rows * s.ws;
    for (uint64_t t = blockIdx.x * (uint64_t)blockDim.x + threadIdx.x; t < total; t += (uint64_t)gridDim.x * blockDim.x) {
        uint32_t y = (uint32_t)(t / s.ws), x = (uint32_t)(t % s.ws), st, yy;
        g0[t] = tlocate(s, y, &st, &yy) ? ext[(uint64_t)yy * s.ext_w + st * s.w + x] : 0;
    }
}

// Generation t+1 from t on columns [t+1, ws-t-1): cell and f = (c-3)(c-3+a) + mask.
extern "C" __global__ void life_gen_step(const uint8_t* g, uint8_t* ng, uint8_t* nf, const LifeTraceShape s, uint32_t t) {
    uint64_t total = (uint64_t)s.rows * s.ws;
    for (uint64_t id = blockIdx.x * (uint64_t)blockDim.x + threadIdx.x; id < total; id += (uint64_t)gridDim.x * blockDim.x) {
        uint32_t y = (uint32_t)(id / s.ws), x = (uint32_t)(id % s.ws);
        if (x < t + 1 || x >= s.ws - t - 1) continue;
        uint32_t up = (y + s.rows - 1) % s.rows, dn = (y + 1) % s.rows;
        const uint8_t* U = g + (uint64_t)up * s.ws;
        const uint8_t* M = g + (uint64_t)y * s.ws;
        const uint8_t* D = g + (uint64_t)dn * s.ws;
        int c = U[x - 1] + U[x] + U[x + 1] + M[x - 1] + M[x + 1] + D[x - 1] + D[x] + D[x + 1];
        int a = M[x];
        uint32_t st, yy, m = 0;
        if (tlocate(s, y, &st, &yy)) {
            uint32_t by = yy < s.k ? 0 : (yy < s.k + s.n ? 1 : 2);
            uint32_t cls = x < s.k ? 0 : (x < s.w + s.k ? 1 : 2);
            uint32_t bx = (cls == 0 && st == 0) ? 0 : ((cls == 2 && st + 1 == s.strips) ? 2 : 1);
            m = s.band_absent[by * 3 + bx];
        }
        int f = (c - 3) * (c - 3 + a) + (int)m;
        ng[id] = f == 0;
        nf[id] = (uint8_t)f;
    }
}

// Trace row r, every column: colmap[c] = kind (0 gen-0 cell, 1 cell, 2 inverse) | t << 2 | i << 10 | x << 18.
// gens/fs: (k+1) generations of rows x ws bytes; inv: inverses of 0..31 (inv[0] = 0).
extern "C" __global__ void life_fill(const uint8_t* gens, const uint8_t* fs, const uint32_t* colmap, const uint32_t* inv,
                                     const LifeTraceShape s, uint32_t width, uint32_t* trace) {
    const uint32_t ONE = 0x01fffffeu;
    uint64_t total = (uint64_t)s.height * width;
    uint64_t gsz = (uint64_t)s.rows * s.ws;
    for (uint64_t id = blockIdx.x * (uint64_t)blockDim.x + threadIdx.x; id < total; id += (uint64_t)gridDim.x * blockDim.x) {
        uint32_t r = (uint32_t)(id / width), c = (uint32_t)(id % width);
        uint32_t cm = colmap[c];
        uint32_t kind = cm & 3, t = (cm >> 2) & 0xff, i = (cm >> 10) & 0xff, x = cm >> 18;
        uint32_t y = (s.rpr * r + t + i) % s.rows;
        uint64_t at = (uint64_t)t * gsz + (uint64_t)y * s.ws + x;
        uint32_t v;
        if (kind == 2) v = inv[fs[at]];
        else v = gens[at] ? ONE : 0;
        trace[id] = v;
    }
}

// ---------------------------------------------------------------- degree-4 extension (x^4 = 3)

struct Ext4 {
    uint32_t v[4];
};

__device__ __forceinline__ void emul(const uint32_t* a, const uint32_t* b, uint32_t* c) {
    const uint32_t W = 0x05fffffau;  // 3 in Montgomery form
    uint32_t t[7] = {0, 0, 0, 0, 0, 0, 0};
#pragma unroll
    for (int i = 0; i < 4; i++)
#pragma unroll
        for (int j = 0; j < 4; j++) t[i + j] = fadd(t[i + j], fmul(a[i], b[j]));
#pragma unroll
    for (int k = 0; k < 3; k++) c[k] = fadd(t[k], fmul(W, t[k + 4]));
    c[3] = t[3];
}

// FRI input accumulation (TwoAdicFriPcs::open): ro[x] += (weighted - rescale * c[x]) * d[x],
// all in the extension (4 coordinates per entry).
extern "C" __global__ void fri_acc(uint32_t* ro, const uint32_t* c, const uint32_t* d, const Ext4 weighted, const Ext4 rescale,
                                   uint32_t n) {
    uint32_t x = blockIdx.x * blockDim.x + threadIdx.x;
    if (x >= n) return;
    uint32_t t[4], u[4], v[4];
    emul(rescale.v, c + 4 * (uint64_t)x, t);
#pragma unroll
    for (int k = 0; k < 4; k++) u[k] = fsub(weighted.v[k], t[k]);
    emul(u, d + 4 * (uint64_t)x, v);
#pragma unroll
    for (int k = 0; k < 4; k++) ro[4 * (uint64_t)x + k] = fadd(ro[4 * (uint64_t)x + k], v[k]);
}

// Column dot products in two passes: part[ch][c] = sum over rows of chunk ch
// (grid.y = chunks), then out[c] = sum_ch part[ch][c].
extern "C" __global__ void ext_col_dot_part(const uint32_t* m, uint32_t width, uint32_t rows, const uint32_t* wgt,
                                            uint32_t* part) {
    uint32_t c = blockIdx.x * blockDim.x + threadIdx.x;
    if (c >= width) return;
    uint32_t chunks = gridDim.y, ch = blockIdx.y;
    uint32_t per = (rows + chunks - 1) / chunks;
    uint32_t r0 = ch * per, r1 = min(rows, r0 + per);
    uint32_t acc[4] = {0, 0, 0, 0};
    for (uint32_t r = r0; r < r1; r++) {
        uint32_t v = m[(uint64_t)r * width + c];
#pragma unroll
        for (int d = 0; d < 4; d++) acc[d] = fadd(acc[d], fmul(v, wgt[4 * r + d]));
    }
#pragma unroll
    for (int d = 0; d < 4; d++) part[((uint64_t)ch * width + c) * 4 + d] = acc[d];
}

extern "C" __global__ void ext_col_dot_sum(const uint32_t* part, uint32_t width, uint32_t chunks, uint32_t* out) {
    uint32_t c = blockIdx.x * blockDim.x + threadIdx.x;
    if (c >= width) return;
    uint32_t acc[4] = {0, 0, 0, 0};
    for (uint32_t ch = 0; ch < chunks; ch++)
#pragma unroll
        for (int d = 0; d < 4; d++) acc[d] = fadd(acc[d], part[((uint64_t)ch * width + c) * 4 + d]);
#pragma unroll
    for (int d = 0; d < 4; d++) out[4 * c + d] = acc[d];
}

// ---------------------------------------------------------------- generic AIR quotient

// Postfix bytecode of base-field constraints (umis-gpu/src/prover.rs, `Program`):
// word = op | arg << 4; op 4 (constant) is followed by the Montgomery value.
#define OP_MAIN 0
#define OP_NEXT 1
#define OP_PER 2
#define OP_PUB 3
#define OP_CONST 4
#define OP_FIRST 5
#define OP_LAST 6
#define OP_TRANS 7
#define OP_ADD 8
#define OP_SUB 9
#define OP_NEG 10
#define OP_MUL 11
#define OP_STORE 12
#define OP_LOAD 13
#define STACK 48
#define SLOTS 64

// One block per quotient point (natural index i), threads over constraints:
// Q(x_i) = inv_van[i] * sum_c apw[c] * C_c(x_i). `lde`: committed trace LDE
// (bit-reversed rows); next row = natural (i + next_step). `per`: periodic table,
// q rows, bit-reversed if per_brev else natural. Selectors natural, q entries.
// Constraints are grouped by template (the same opcode sequence with different
// arguments), so a warp's lanes run the same instructions on different instances:
// no divergence, and neighbouring instances read neighbouring trace columns.
// groups: per group [code_start, code_end, n_args, n_inst, args_offset]; an
// instance's argument a is args[args_offset + a * n_inst + j], and row n_args is
// its constraint index (the weight apw[c]). The template operand of MAIN/NEXT/PER/
// PUB/CONST is the argument index; that of STORE/LOAD is the slot.
extern "C" __global__ void air_quotient(const uint32_t* lde, uint32_t width, uint32_t next_step, const uint32_t* per,
                                        uint32_t n_per, uint32_t per_brev, const uint32_t* pub, const uint32_t* sel_first,
                                        const uint32_t* sel_last, const uint32_t* sel_trans, const uint32_t* inv_van,
                                        const uint32_t* apw, const uint32_t* code, const uint32_t* groups,
                                        uint32_t n_groups, const uint32_t* args, uint32_t log_q, uint32_t stride,
                                        uint32_t n_pub, uint32_t log_chunks, uint32_t* out) {
    __shared__ uint32_t sh[4][256];
    uint32_t q = 1u << log_q;
    uint32_t i = blockIdx.x, k = blockIdx.y, m = gridDim.y;
    if (i >= q) return;
    const uint32_t* cur = lde + (uint64_t)brev(i, log_q) * stride + (uint64_t)k * width;
    const uint32_t* nxt = lde + (uint64_t)brev((i + next_step) & (q - 1), log_q) * stride + (uint64_t)k * width;
    const uint32_t* pv = n_per ? per + (uint64_t)(per_brev ? brev(i, log_q) : i) * n_per * m + (uint64_t)k * n_per : per;
    pub += (uint64_t)k * n_pub;
    uint32_t f = sel_first[i], l = sel_last[i], t = sel_trans[i];
    uint32_t acc[4] = {0, 0, 0, 0};
    for (uint32_t g = 0; g < n_groups; g++) {
        const uint32_t* gm = groups + 5 * g;
        uint32_t c0 = gm[0], c1 = gm[1], na = gm[2], ni = gm[3];
        const uint32_t* ga = args + gm[4];
        for (uint32_t j = threadIdx.x; j < ni; j += blockDim.x) {
            const uint32_t* ar = ga + j;
            uint32_t st[STACK], sl[SLOTS];
            int sp = 0;
            for (uint32_t pc = c0; pc < c1; pc++) {
                uint32_t w = code[pc], op = w & 15, arg = w >> 4;
                switch (op) {
                    case OP_MAIN: st[sp++] = cur[ar[arg * ni]]; break;
                    case OP_NEXT: st[sp++] = nxt[ar[arg * ni]]; break;
                    case OP_PER: st[sp++] = pv[ar[arg * ni]]; break;
                    case OP_PUB: st[sp++] = pub[ar[arg * ni]]; break;
                    case OP_CONST: st[sp++] = ar[arg * ni]; break;
                    case OP_FIRST: st[sp++] = f; break;
                    case OP_LAST: st[sp++] = l; break;
                    case OP_TRANS: st[sp++] = t; break;
                    case OP_ADD: sp--; st[sp - 1] = fadd(st[sp - 1], st[sp]); break;
                    case OP_SUB: sp--; st[sp - 1] = fsub(st[sp - 1], st[sp]); break;
                    case OP_NEG: st[sp - 1] = fsub(0, st[sp - 1]); break;
                    case OP_STORE: sl[arg] = st[sp - 1]; break;
                    case OP_LOAD: st[sp++] = sl[arg]; break;
                    default: sp--; st[sp - 1] = fmul(st[sp - 1], st[sp]); break;
                }
            }
            uint32_t v = st[0], c = ar[na * ni];
#pragma unroll
            for (int d = 0; d < 4; d++) acc[d] = fadd(acc[d], fmul(v, apw[4 * c + d]));
        }
    }
#pragma unroll
    for (int d = 0; d < 4; d++) sh[d][threadIdx.x] = acc[d];
    __syncthreads();
    for (uint32_t s2 = blockDim.x / 2; s2 > 0; s2 >>= 1) {
        if (threadIdx.x < s2)
#pragma unroll
            for (int d = 0; d < 4; d++) sh[d][threadIdx.x] = fadd(sh[d][threadIdx.x], sh[d][threadIdx.x + s2]);
        __syncthreads();
    }
    if (threadIdx.x == 0) {
        // chunk-major: chunk j = points i = row * n_chunks + j (`split_evals`), instances side by side
        uint32_t iv = inv_van[i], j = i & ((1u << log_chunks) - 1), row = i >> log_chunks;
        uint64_t o = (((uint64_t)j * (q >> log_chunks) + row) * m + k) * 4;
#pragma unroll
        for (int d = 0; d < 4; d++) out[o + d] = fmul(sh[d][0], iv);
    }
}

// ---------------------------------------------------------------- Poseidon2 (KoalaBear, width 16)

// Constants (Montgomery form): rc[0..64] initial external rounds, rc[64..84] internal,
// rc[84..148] terminal external rounds, rc[148..164] internal diagonal V (V[0] unused).
__device__ __forceinline__ void p2_mat4(uint32_t* x) {
    uint32_t t01 = fadd(x[0], x[1]), t23 = fadd(x[2], x[3]);
    uint32_t t0123 = fadd(t01, t23);
    uint32_t t01123 = fadd(t0123, x[1]), t01233 = fadd(t0123, x[3]);
    x[3] = fadd(t01233, fadd(x[0], x[0]));
    x[1] = fadd(t01123, fadd(x[2], x[2]));
    x[0] = fadd(t01123, t01);
    x[2] = fadd(t01233, t23);
}

__device__ __forceinline__ void p2_mds_light(uint32_t* s) {
#pragma unroll
    for (int c = 0; c < 4; c++) p2_mat4(s + 4 * c);
    uint32_t sums[4];
#pragma unroll
    for (int k = 0; k < 4; k++) sums[k] = fadd(fadd(s[k], s[4 + k]), fadd(s[8 + k], s[12 + k]));
#pragma unroll
    for (int i = 0; i < 16; i++) s[i] = fadd(s[i], sums[i & 3]);
}

__device__ __forceinline__ uint32_t p2_cube(uint32_t x) { return fmul(fmul(x, x), x); }

__device__ __forceinline__ void p2_permute(uint32_t* s, const uint32_t* rc) {
    p2_mds_light(s);
#pragma unroll
    for (int r = 0; r < 4; r++) {
#pragma unroll
        for (int i = 0; i < 16; i++) s[i] = p2_cube(fadd(s[i], rc[16 * r + i]));
        p2_mds_light(s);
    }
#pragma unroll
    for (int r = 0; r < 20; r++) {
        s[0] = p2_cube(fadd(s[0], rc[64 + r]));
        uint32_t part = 0;
#pragma unroll
        for (int i = 1; i < 16; i++) part = fadd(part, s[i]);
        uint32_t full = fadd(part, s[0]);
        s[0] = fsub(part, s[0]);
#pragma unroll
        for (int i = 1; i < 16; i++) s[i] = fadd(fmul(s[i], rc[148 + i]), full);
    }
#pragma unroll
    for (int r = 0; r < 4; r++) {
#pragma unroll
        for (int i = 0; i < 16; i++) s[i] = p2_cube(fadd(s[i], rc[84 + 16 * r + i]));
        p2_mds_light(s);
    }
}

// Permutes `n` states of 16 elements in place (tests).
extern "C" __global__ void p2_permute_states(uint32_t* states, const uint32_t* rc, uint32_t n) {
    uint32_t t = blockIdx.x * blockDim.x + threadIdx.x;
    if (t >= n) return;
    uint32_t s[16];
#pragma unroll
    for (int i = 0; i < 16; i++) s[i] = states[16 * t + i];
    p2_permute(s, rc);
#pragma unroll
    for (int i = 0; i < 16; i++) states[16 * t + i] = s[i];
}

// Proof-of-work search (`DuplexChallenger::grind`): candidate w = start + thread
// (canonical, below p) goes into rate slot `widx` of `base`; it passes when the low bits
// of canonical(state[7]) after the permutation are zero (`mask`). The smallest passing
// candidate is kept in `found` (atomicMin; initialise to 0xffffffff). r2 = R^2 mod p.
extern "C" __global__ void p2_grind(const uint32_t* base, uint32_t widx, const uint32_t* rc, uint32_t mask,
                                    uint32_t start, uint32_t r2, uint32_t* found) {
    __shared__ uint32_t src[164];
    for (uint32_t i = threadIdx.x; i < 164; i += blockDim.x) src[i] = rc[i];
    __syncthreads();
    uint32_t w = start + blockIdx.x * blockDim.x + threadIdx.x;
    if (w >= P) return;
    uint32_t s[16];
#pragma unroll
    for (int i = 0; i < 16; i++) s[i] = base[i];
    s[widx & 7] = fmul(w, r2);
    p2_permute(s, src);
    if ((mred(s[7]) & mask) == 0) atomicMin(found, w);
}
