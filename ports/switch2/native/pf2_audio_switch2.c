// SPDX-License-Identifier: MIT OR Apache-2.0
#include "pf2_native.h"
#if !defined(__SWITCH2__)
#error "Native Switch 2 audio only"
#endif
#include <switch.h>
#include <malloc.h>
#include <math.h>
#include <stdlib.h>
#include <stdint.h>
#include <string.h>

struct Pf2Audio {
    uint32_t rate, channels;
    int started;
    int16_t *pcm_buffer;
    size_t pcm_capacity;
};

Pf2Error pf2_audio_open(Pf2Audio **out, uint32_t rate, uint32_t channels) {
    if (!out) return PF2_INVALID;
    *out = NULL;
    if (rate != 48000 || channels != 2) return PF2_UNSUPPORTED;
    Pf2Audio *a = calloc(1, sizeof(*a));
    if (!a) return PF2_NO_MEMORY;
    Result rc = audoutInitialize();
    if (R_FAILED(rc)) { free(a); return PF2_SDK_ERROR; }
    if (audoutGetSampleRate() != 48000 || audoutGetChannelCount() != 2) {
        audoutExit(); free(a); return PF2_UNSUPPORTED;
    }
    rc = audoutStartAudioOut();
    if (R_FAILED(rc)) { audoutExit(); free(a); return PF2_SDK_ERROR; }
    a->rate = rate;
    a->channels = channels;
    a->started = 1;
    *out = a;
    return PF2_OK;
}

Pf2Error pf2_audio_write(Pf2Audio *a, const float *pcm, size_t frames) {
    if (!a || !a->started || !pcm || !frames || frames > 48000) return PF2_INVALID;
    if (frames > SIZE_MAX / (2 * sizeof(int16_t))) return PF2_INVALID;
    size_t bytes = frames * 2 * sizeof(int16_t);
    const size_t aligned = (bytes + 4095) & ~(size_t)4095;
    // audoutPlayBuffer blocks until this very packet is released. Keep the
    // allocation for subsequent grains; grow only if the stream grain size
    // changes. No allocator churn in the steady-state audio path.
    if (aligned > a->pcm_capacity) {
        int16_t *next = memalign(4096, aligned);
        if (!next) return PF2_NO_MEMORY;
        free(a->pcm_buffer);
        a->pcm_buffer = next;
        a->pcm_capacity = aligned;
    }
    int16_t *buf = a->pcm_buffer;
    for (size_t i=0; i<frames*2; ++i) {
        float f = pcm[i];
        if (!isfinite(f)) f = 0;
        f = fmaxf(-1.f, fminf(1.f, f));
        buf[i] = (int16_t)lrintf(f * 32767.f);
    }
    AudioOutBuffer packet = {0};
    packet.buffer = buf;
    packet.buffer_size = aligned;
    packet.data_size = bytes;
    AudioOutBuffer *released = NULL;
    Result rc = audoutPlayBuffer(&packet, &released);
    // audoutPlayBuffer waits for release. No buffer is freed while queued.
    if (R_SUCCEEDED(rc) && released != &packet) rc = MAKERESULT(Module_Libnx, LibnxError_ShouldNotHappen);
    return R_SUCCEEDED(rc) ? PF2_OK : PF2_SDK_ERROR;
}

void pf2_audio_close(Pf2Audio *a) {
    if (!a) return;
    if (a->started) {
        bool flushed = false;
        audoutFlushAudioOutBuffers(&flushed);
        audoutStopAudioOut();
        audoutExit();
    }
    free(a->pcm_buffer);
    free(a);
}
