#include "../native/pf2_native.h"
#include <assert.h>
#include <stdio.h>
#include <stdint.h>
#include <string.h>

int main(void) {
    uint8_t y[12] = {16, 235, 16, 235, 16, 235, 16, 235, 0, 0, 0, 0};
    uint8_t uv[6] = {128, 128, 128, 128, 0, 0};
    uint8_t rgba[48];
    memset(rgba, 0xAF, sizeof(rgba));
    assert(pf2_nv12_rgba(y, sizeof(y), 4, uv, sizeof(uv), 4, 4, 2,
                         rgba, sizeof(rgba), 24, PF2_BT709, 0) == PF2_OK);
    for (int r = 0; r < 2; ++r) {
        for (int col = 0; col < 4; ++col) {
            const uint8_t *p = rgba + r * 24 + col * 4;
            int v = col % 2 ? 255 : 0;
            assert(p[0] == v && p[1] == v && p[2] == v && p[3] == 255);
        }
        for (int j = 16; j < 24; ++j) assert(rgba[r*24+j] == 0xAF);
    }
    assert(pf2_nv12_rgba(y, sizeof(y), 1, uv, sizeof(uv), 4, 4, 2,
                         rgba, sizeof(rgba), 24, PF2_BT601, 0) == PF2_INVALID);
    assert(pf2_nv12_rgba(y, sizeof(y), 4, uv, 2, 4, 4, 2,
                         rgba, sizeof(rgba), 24, PF2_BT601, 0) == PF2_INVALID);
    assert(pf2_nv12_rgba(y, sizeof(y), 4, uv, sizeof(uv), 4, 3, 2,
                         rgba, sizeof(rgba), 24, PF2_BT601, 0) == PF2_INVALID);
    // Full-range black and white at the endpoint; no overflow.
    uint8_t full_y[4] = {0, 255, 0, 255}, full_uv[2] = {128,128}, out[16];
    assert(pf2_nv12_rgba(full_y,4,2,full_uv,2,2,2,2,out,16,8,PF2_BT601,1)==PF2_OK);
    assert(out[0] == 0 && out[4] == 255);
    puts("PASS NV12 conversion/capacity/stride/color");
    return 0;
}
