// SPDX-License-Identifier: MIT OR Apache-2.0
#include "pf2_native.h"
#include <stdint.h>
#include <stddef.h>

static uint8_t clamp8(int32_t x) {
    if (x < 0) return 0;
    if (x > 255) return 255;
    return (uint8_t)x;
}
static int fits(size_t height, size_t stride, size_t row, size_t cap) {
    return height != 0 && stride >= row &&
           (height - 1) <= (SIZE_MAX - row) / stride &&
           (height - 1) * stride + row <= cap;
}

Pf2Error pf2_nv12_rgba(const uint8_t *y, size_t y_bytes, size_t y_stride,
                       const uint8_t *uv, size_t uv_bytes, size_t uv_stride,
                       uint32_t width, uint32_t height,
                       uint8_t *rgba, size_t rgba_bytes, size_t rgba_stride,
                       Pf2ColorMatrix matrix, int full_range) {
    if (!y || !uv || !rgba || !width || !height || (width & 1) || (height & 1) ||
        (matrix != PF2_BT601 && matrix != PF2_BT709)) return PF2_INVALID;
    const size_t w = width, h = height;
    if (!fits(h, y_stride, w, y_bytes) ||
        !fits(h / 2, uv_stride, w, uv_bytes) ||
        !fits(h, rgba_stride, w * 4, rgba_bytes)) return PF2_INVALID;
    // Fixed-point coefficients are scaled by 256. Conversion is 8-bit SDR.
    // Build small lookup tables per frame to avoid millions of repeated
    // multiply operations. The two luma pixels in a 4:2:0 pair share UV.
    const int cr = full_range ? (matrix == PF2_BT709 ? 403 : 359)
                              : (matrix == PF2_BT709 ? 459 : 409);
    const int cb = full_range ? (matrix == PF2_BT709 ? 475 : 454)
                              : (matrix == PF2_BT709 ? 541 : 516);
    const int cg_u = full_range ? (matrix == PF2_BT709 ? 48 : 88)
                                : (matrix == PF2_BT709 ? 55 : 100);
    const int cg_v = full_range ? (matrix == PF2_BT709 ? 120 : 183)
                                : (matrix == PF2_BT709 ? 136 : 208);
    int32_t base_y[256], r_v[256], g_u[256], g_v[256], b_u[256];
    for (int i = 0; i < 256; ++i) {
        const int c = full_range ? i : (i > 16 ? i - 16 : 0);
        const int chroma = i - 128;
        base_y[i] = full_range ? c * 256 : c * 298;
        r_v[i] = cr * chroma;
        g_u[i] = cg_u * chroma;
        g_v[i] = cg_v * chroma;
        b_u[i] = cb * chroma;
    }
    for (size_t r = 0; r < h; ++r) {
        const uint8_t *yy = y + r * y_stride;
        const uint8_t *cc = uv + (r / 2) * uv_stride;
        uint8_t *dest = rgba + r * rgba_stride;
        for (size_t col = 0; col < w; col += 2) {
            const int u = cc[col], v = cc[col + 1];
            const int rr = r_v[v] + 128;
            const int gg = 128 - g_u[u] - g_v[v];
            const int bb = b_u[u] + 128;
            const int y0 = base_y[yy[col]], y1 = base_y[yy[col + 1]];
            uint8_t *p0 = dest + col * 4;
            uint8_t *p1 = p0 + 4;
            p0[0] = clamp8((y0 + rr) >> 8);
            p0[1] = clamp8((y0 + gg) >> 8);
            p0[2] = clamp8((y0 + bb) >> 8);
            p0[3] = 255;
            p1[0] = clamp8((y1 + rr) >> 8);
            p1[1] = clamp8((y1 + gg) >> 8);
            p1[2] = clamp8((y1 + bb) >> 8);
            p1[3] = 255;
        }
    }
    return PF2_OK;
}
