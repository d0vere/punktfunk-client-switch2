// SPDX-License-Identifier: MIT OR Apache-2.0
// Native libounce decoder using the actual Switch 2 VideoDecoder API.
// No libnx Switch 1 NVDEC calls and no third-party streaming protocol.
#include "pf2_native.h"
#if !defined(__SWITCH2__)
#error "This translation unit must only be built by the Switch 2 toolchain"
#endif
#include <switch.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

struct Pf2Decoder {
    VideoDecoder object;
    VideoFrame frame;
    uint8_t *y;
    uint8_t *uv;
    size_t luma_bytes;
    size_t chroma_bytes;
    uint32_t width;
    uint32_t height;
    uint32_t bit_depth;
    uint32_t row_bytes;
    uint32_t last_sdk_result;
    uint32_t last_sdk_stage;
    uint32_t last_copy_us;
    bool opened;
    bool held;
    bool block_linear_8bit;
};

static Pf2Error sdk_error(Pf2Decoder *d, Result rc, uint32_t stage) {
    d->last_sdk_result = (uint32_t)rc;
    d->last_sdk_stage = stage;
    if (R_VALUE(rc) == KERNELRESULT(TimedOut)) return PF2_NOT_READY;
    return PF2_SDK_ERROR;
}

Pf2Error pf2_decoder_open(Pf2Decoder **out, const Pf2DecoderSettings *settings) {
    if (!out || !settings) return PF2_INVALID;
    *out = NULL;
    if (settings->codec != ViddecCodec_Av1 && settings->codec != ViddecCodec_H264)
        return PF2_UNSUPPORTED;
    if ((settings->bit_depth != 8 && settings->bit_depth != 10) ||
        (settings->bit_depth == 10 && settings->codec != ViddecCodec_Av1))
        return PF2_UNSUPPORTED;
    if (!settings->width || !settings->height || (settings->width & 1) || (settings->height & 1) ||
        settings->width > 8192 || settings->height > 8192 ||
        settings->fps > VIDEO_DECODER_MAX_FRAME_RATE ||
        (settings->image_count && (settings->image_count < 2 || settings->image_count > VIDDEC_MAX_IMAGES)) ||
        (settings->codec == ViddecCodec_H264 && (!settings->codec_data || !settings->codec_data_length)) ||
        (settings->codec_data_length && !settings->codec_data)) return PF2_INVALID;
    const size_t pixels = (size_t)settings->width * settings->height;
    if (pixels > SIZE_MAX / 4 || settings->width > UINT32_MAX / 2) return PF2_INVALID;
    Pf2Decoder *d = calloc(1, sizeof(*d));
    if (!d) return PF2_NO_MEMORY;
    d->width = settings->width;
    d->height = settings->height;
    d->bit_depth = settings->bit_depth;
    // At 8-bit, both codecs use libounce's block-linear output surfaces.
    // H.264 pitch-linear consistently failed GetFrame with SDK 0xfa66f on
    // the Switch 2; AV1 block-linear + CopyFrame is already device-tested.
    // Keep experimental AV1 Main10 pitch-linear unchanged.
    d->block_linear_8bit = settings->bit_depth == 8;
    d->row_bytes = settings->width * (settings->bit_depth == 10 ? 2u : 1u);
    d->luma_bytes = pixels * (settings->bit_depth == 10 ? 2u : 1u);
    d->chroma_bytes = d->luma_bytes / 2;
    d->y = malloc(d->luma_bytes);
    d->uv = malloc(d->chroma_bytes);
    if (!d->y || !d->uv) {
        pf2_decoder_close(d);
        return PF2_NO_MEMORY;
    }
    VideoDecoderConfig config = {
        .codec = (ViddecCodec)settings->codec,
        .width = settings->width,
        .height = settings->height,
        .image_count = settings->image_count ? settings->image_count : VIDEO_DECODER_DEFAULT_IMAGES,
        .frame_rate = settings->fps,
        // Do not attempt H.264 NV12 pitch-linear: GetFrame returned
        // 0xfa66f before any NV12 conversion. SDK CopyFrame untiles the
        // block-linear picture for both AV1 and H.264 8-bit.
        .tiling = d->block_linear_8bit
            ? ViddecTilingMode_BlockLinear : ViddecTilingMode_PitchLinear,
        .color_policy = d->block_linear_8bit ? 0
            : (settings->width >= 1920 || settings->height >= 1080 ? 3 : 1),
        .max_sample_size = settings->max_sample_bytes,
        .codec_config = settings->codec_data,
        .codec_config_size = settings->codec_data_length,
    };
    Result rc = videoDecoderCreate(&d->object, &config);
    if (R_FAILED(rc)) {
        // A failed Create is not necessarily a fully initialized decoder; the SDK owns rollback.
        d->last_sdk_result = (uint32_t)rc;
        pf2_decoder_close(d);
        return PF2_SDK_ERROR;
    }
    d->opened = true;
    *out = d;
    return PF2_OK;
}

Pf2Error pf2_decoder_submit(Pf2Decoder *d, const uint8_t *data,
                            size_t length, int64_t pts_ns, uint64_t timeout_ns) {
    if (!d || !d->opened || !data || !length) return PF2_INVALID;
    if (d->held) return PF2_NOT_READY;
    Result rc = videoDecoderSubmit(&d->object, data, length, pts_ns, false, timeout_ns);
    if (R_FAILED(rc)) return sdk_error(d, rc, 1);
    return PF2_OK;
}

Pf2Error pf2_decoder_acquire(Pf2Decoder *d, Pf2DecodedFrame *out,
                             uint64_t timeout_ns) {
    if (!d || !out || !d->opened) return PF2_INVALID;
    if (d->held) return PF2_NOT_READY;
    Result rc = videoDecoderGetFrame(&d->object, &d->frame, timeout_ns);
    if (R_FAILED(rc)) return sdk_error(d, rc, 2);
    // Real producer dependency has been waited for by videoDecoderGetFrame.
    const uint32_t w = d->frame.width, h = d->frame.height;
    if (!w || !h || (w & 1) || (h & 1) || w > d->width || h > d->height) {
        videoDecoderReleaseFrame(&d->object, &d->frame);
        return PF2_UNSUPPORTED;
    }
    d->held = true;
    const uint64_t copy_started = armGetSystemTick();
    if (d->block_linear_8bit) {
        // Block-linear hardware output must never be dereferenced as CPU
        // pitch-linear planes. SDK CopyFrame untiles into our NV12 buffers.
        rc = videoDecoderCopyFrame(&d->object, &d->frame,
                                   d->y, d->row_bytes, d->uv, d->row_bytes);
        if (R_FAILED(rc)) {
            videoDecoderReleaseFrame(&d->object, &d->frame);
            d->held = false;
            return sdk_error(d, rc, 3);
        }
    } else {
        // Experimental AV1 Main10 retains pitch-linear direct readback.
        // Validate both planes against their backing allocation before copying.
        const VideoDecoderImage *image = d->frame.slot < d->object.image_count
            ? &d->object.images[d->frame.slot] : NULL;
        const uint32_t row_bytes = w * (d->bit_depth == 10 ? 2u : 1u);
        bool valid = image && image->backing && d->frame.planes[0] && d->frame.planes[1]
            && d->frame.pitches[0] >= row_bytes && d->frame.pitches[1] >= row_bytes;
        if (valid) {
            const uintptr_t base = (uintptr_t)image->backing;
            const size_t total = (size_t)image->backing_size;
            for (unsigned plane = 0; plane < 2; ++plane) {
                const uintptr_t address = (uintptr_t)d->frame.planes[plane];
                const uint32_t rows = plane == 0 ? h : h / 2u;
                const size_t stride = d->frame.pitches[plane];
                if (address < base || (size_t)(address - base) > total ||
                    (size_t)(rows - 1u) > (SIZE_MAX - row_bytes) / stride ||
                    ((size_t)(rows - 1u) * stride + row_bytes) > total - (size_t)(address - base)) {
                    valid = false;
                    break;
                }
            }
        }
        if (!valid) {
            d->last_sdk_result = PF2_UNSUPPORTED;
            d->last_sdk_stage = 5;
            videoDecoderReleaseFrame(&d->object, &d->frame);
            d->held = false;
            return PF2_UNSUPPORTED;
        }
        for (uint32_t row = 0; row < h; ++row)
            memcpy(d->y + (size_t)row * d->row_bytes,
                   d->frame.planes[0] + (size_t)row * d->frame.pitches[0], row_bytes);
        for (uint32_t row = 0; row < h / 2u; ++row)
            memcpy(d->uv + (size_t)row * d->row_bytes,
                   d->frame.planes[1] + (size_t)row * d->frame.pitches[1], row_bytes);
    }
    const uint64_t copy_ns = armTicksToNs(armGetSystemTick() - copy_started);
    d->last_copy_us = (uint32_t)((copy_ns / 1000) > UINT32_MAX ? UINT32_MAX : (copy_ns / 1000));
    *out = (Pf2DecodedFrame) {
        .width = w,
        .height = h,
        .pitch_y = d->row_bytes,
        .pitch_uv = d->row_bytes,
        .pts_ns = d->frame.timestamp_ns >= 0 ? (uint64_t)d->frame.timestamp_ns : 0,
        .plane_y = d->y,
        .plane_uv = d->uv,
    };
    return PF2_OK;
}

// Avoid d->y/d->uv -> Rust packing by lending a caller-owned packed NV12
// buffer for one acquire. Both 8-bit AV1 and H.264 use SDK block-linear
// CopyFrame into this buffer. Neither path is hardware zero-copy.
Pf2Error pf2_decoder_acquire_into(Pf2Decoder *d, Pf2DecodedFrame *out,
                                   uint8_t *packed, size_t capacity,
                                   uint64_t timeout_ns) {
    if (!d || !out || !packed || !d->opened || d->held ||
        d->bit_depth != 8) return PF2_UNSUPPORTED;
    // d->luma_bytes/chroma_bytes and row_bytes were checked at decoder open.
    if (capacity < d->luma_bytes ||
        capacity - d->luma_bytes < d->chroma_bytes) return PF2_INVALID;
    uint8_t *const original_y = d->y;
    uint8_t *const original_uv = d->uv;
    d->y = packed;
    d->uv = packed + d->luma_bytes;
    Pf2Error rc = pf2_decoder_acquire(d, out, timeout_ns);
    d->y = original_y;
    d->uv = original_uv;
    // The packed stride is d->row_bytes; never interpret a smaller decoded
    // size as tightly packed. Release the image and reject such a frame.
    if (rc == PF2_OK && (out->width != d->width || out->height != d->height)) {
        pf2_decoder_release(d);
        d->last_sdk_stage = 5;
        return PF2_UNSUPPORTED;
    }
    return rc;
}

Pf2Error pf2_decoder_release(Pf2Decoder *d) {
    if (!d || !d->opened || !d->held) return PF2_INVALID;
    videoDecoderReleaseFrame(&d->object, &d->frame);
    d->held = false;
    return PF2_OK;
}

Pf2Error pf2_decoder_flush(Pf2Decoder *d) {
    if (!d || !d->opened) return PF2_INVALID;
    if (d->held) pf2_decoder_release(d);
    Result rc = videoDecoderFlush(&d->object);
    return R_SUCCEEDED(rc) ? PF2_OK : sdk_error(d, rc, 4);
}

void pf2_decoder_close(Pf2Decoder *d) {
    if (!d) return;
    if (d->held) pf2_decoder_release(d);
    if (d->opened) videoDecoderClose(&d->object);
    free(d->uv);
    free(d->y);
    free(d);
}

uint32_t pf2_decoder_last_sdk_result(const Pf2Decoder *d) {
    return d ? d->last_sdk_result : 0;
}

uint32_t pf2_decoder_last_sdk_stage(const Pf2Decoder *d) {
    return d ? d->last_sdk_stage : 0;
}

// Measured only during the most recent pf2_decoder_acquire().
uint32_t pf2_decoder_last_copy_us(const Pf2Decoder *d) {
    return d ? d->last_copy_us : 0;
}
