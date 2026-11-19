// SPDX-License-Identifier: MIT OR Apache-2.0
// Pure input parser; no video decoder/session is started here.
#include "pf2_native.h"
#include <string.h>

static size_t start_code(const uint8_t *buf, size_t size, size_t at) {
    if (at + 3 > size || buf[at] != 0 || buf[at+1] != 0) return 0;
    if (buf[at+2] == 1) return 3;
    if (at + 4 <= size && buf[at+2] == 0 && buf[at+3] == 1) return 4;
    return 0;
}

// Search only for framing bytes (three/four-byte start code), never parse RBSP.
// Emulation prevention bytes are irrelevant here as 00 00 03 cannot be a start code.
static int find_start(const uint8_t *buf, size_t size, size_t from,
                      size_t *pos, size_t *length) {
    for (size_t i = from; i + 3 <= size; ++i) {
        size_t len = start_code(buf, size, i);
        if (len) { *pos = i; *length = len; return 1; }
    }
    return 0;
}

Pf2Error pf2_h264_config_from_annexb(const uint8_t *buf, size_t len,
                                    uint8_t *out, size_t capacity, size_t *written) {
    if (!buf || !out || !written || !len) return PF2_INVALID;
    *written = 0;
    size_t offsets[2] = {0}, sizes[2] = {0};
    size_t from = 0, begin, prefix;
    while (find_start(buf, len, from, &begin, &prefix)) {
        const size_t nal_begin = begin + prefix;
        size_t next, next_prefix;
        const int has_next = find_start(buf, len, nal_begin, &next, &next_prefix);
        const size_t end = has_next ? next : len;
        if (nal_begin < end) {
            const uint8_t type = buf[nal_begin] & 0x1fu;
            if ((type == 7 || type == 8) && offsets[type == 8] == 0) {
                offsets[type == 8] = nal_begin;
                sizes[type == 8] = end - nal_begin;
            }
        }
        if (!has_next || (offsets[0] && offsets[1])) break;
        from = next;
    }
    if (!sizes[0] || !sizes[1]) return PF2_NOT_READY;
    if (sizes[0] > SIZE_MAX - sizes[1] - 8) return PF2_INVALID;
    const size_t total = sizes[0] + sizes[1] + 8;
    if (capacity < total) return PF2_NO_MEMORY;
    const uint8_t header[4] = {0,0,0,1};
    size_t w = 0;
    for (size_t i = 0; i < 2; ++i) {
        memcpy(out + w, header, 4); w += 4;
        memcpy(out + w, buf + offsets[i], sizes[i]); w += sizes[i];
    }
    *written = w;
    return PF2_OK;
}
