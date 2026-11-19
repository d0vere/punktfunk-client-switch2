// Mock-SDK host test. Hardware / GPU import is NOT tested.
#include "pf2_native.h"
#include <switch.h>
#include <assert.h>
#include <stdlib.h>
#include <string.h>
#include <stdio.h>
static unsigned get_calls, copy_calls, release_calls;
static uint32_t video_w, video_h;
static int h264_mode;
static unsigned h264_copy_calls;
Result videoDecoderCreate(VideoDecoder *d, const VideoDecoderConfig *c) {
    d->image_count = c->image_count;
    video_w = c->width; video_h = c->height;
    h264_mode = c->codec == ViddecCodec_H264;
    // Reproduce the surface mode from the console failure: both 8-bit codecs
    // must use BlockLinear; H.264 pitch-linear did not return usable frames.
    assert(c->tiling == ViddecTilingMode_BlockLinear);
    assert(c->color_policy == 0);
    if (h264_mode) {
        assert(video_w == 16 && video_h == 8);
        assert(c->codec_config && c->codec_config_size);
    }
    return 0;
}
Result videoDecoderSubmit(VideoDecoder *d, const uint8_t *p, size_t n, int64_t pts, int eos, uint64_t wait) {
    (void)d;(void)p;(void)n;(void)pts;(void)eos;(void)wait;
    return 0;
}
Result videoDecoderGetFrame(VideoDecoder *d, VideoFrame *f, uint64_t wait) {
    (void)d;(void)wait;
    ++get_calls;
    f->width = video_w; f->height = video_h; f->timestamp_ns = 123456;
    return 0;
}
Result videoDecoderCopyFrame(VideoDecoder *d, VideoFrame *f, uint8_t *y, uint32_t pitch_y, uint8_t *uv, uint32_t pitch_uv) {
    (void)d; (void)f;
    assert(pitch_y == video_w && pitch_uv == video_w);
    memset(y, h264_mode ? 0x39 : 0x2A, (size_t)video_w * video_h);
    memset(uv, h264_mode ? 0xB0 : 0x80, (size_t)video_w * video_h / 2u);
    ++copy_calls;
    if (h264_mode) ++h264_copy_calls;
    return 0;
}
Result videoDecoderReleaseFrame(VideoDecoder *d, VideoFrame *f) {
    (void)d; (void)f; ++release_calls; return 0;
}
Result videoDecoderFlush(VideoDecoder *d) { (void)d;return 0; }
void videoDecoderClose(VideoDecoder *d) { (void)d; }
uint64_t armGetSystemTick(void) { static uint64_t ticks;return ++ticks; }
uint64_t armTicksToNs(uint64_t v) {return v * 1000;}
int main(void) {
    Pf2Decoder *d = NULL;
    Pf2DecoderSettings cfg = {
        .width=16,.height=8,.fps=60,.codec=ViddecCodec_Av1,.image_count=4,
        .bit_depth=8,.max_sample_bytes=1024,
    };
    assert(pf2_decoder_open(&d, &cfg) == PF2_OK && d);
    const size_t size = cfg.width * cfg.height * 3 / 2;
    uint8_t *buf = malloc(size);
    assert(buf);
    memset(buf,0,size);
    Pf2DecodedFrame f = {0};
    assert(pf2_decoder_acquire_into(d, &f, buf, size - 1, 0) == PF2_INVALID);
    assert(get_calls == 0 && copy_calls == 0);
    assert(pf2_decoder_acquire_into(d, &f, buf, size, 0) == PF2_OK);
    assert(get_calls == 1 && copy_calls == 1);
    assert(f.plane_y == buf && f.plane_uv == buf + cfg.width * cfg.height);
    assert(f.width == cfg.width && f.height == cfg.height);
    assert(f.pitch_y == cfg.width && f.pitch_uv == cfg.width);
    for (size_t i = 0; i < cfg.width * cfg.height; ++i) assert(buf[i] == 0x2A);
    for (size_t i = cfg.width * cfg.height; i < size; ++i) assert(buf[i] == 0x80);
    assert(pf2_decoder_release(d) == PF2_OK);
    assert(release_calls == 1);
    // After temporary lending, the original native staging remains owned and
    // is still usable on the untouched fallback path.
    assert(pf2_decoder_acquire(d, &f, 0) == PF2_OK);
    assert(f.plane_y != buf && f.plane_uv != buf + cfg.width * cfg.height);
    assert(f.plane_y[0] == 0x2A && f.plane_uv[0] == 0x80);
    assert(pf2_decoder_release(d) == PF2_OK);
    pf2_decoder_close(d);
    // Verify H.264 block-linear SDK CopyFrame into both a borrowed Rust-style
    // buffer and the original native staging allocation.
    const uint8_t h264_config[] = {0, 0, 0, 1, 0x67, 0x42};
    cfg.codec = ViddecCodec_H264;
    cfg.codec_data = h264_config;
    cfg.codec_data_length = sizeof(h264_config);
    assert(pf2_decoder_open(&d, &cfg) == PF2_OK && d);
    const unsigned copies_before_h264 = copy_calls;
    const unsigned gets_before_h264 = get_calls;
    assert(pf2_decoder_acquire_into(d, &f, buf, size - 1, 0) == PF2_INVALID);
    assert(get_calls == gets_before_h264); // No image acquired on bad capacity.
    memset(buf, 0, size);
    assert(pf2_decoder_acquire_into(d, &f, buf, size, 0) == PF2_OK);
    assert(f.plane_y == buf && f.plane_uv == buf + cfg.width * cfg.height);
    assert(f.pitch_y == cfg.width && f.pitch_uv == cfg.width);
    for (size_t i = 0; i < cfg.width * cfg.height; ++i) assert(buf[i] == 0x39);
    for (size_t i = cfg.width * cfg.height; i < size; ++i) assert(buf[i] == 0xB0);
    assert(copy_calls == copies_before_h264 + 1);
    assert(h264_copy_calls == 1); // H.264 was copied by the SDK, not row memcpy.
    assert(pf2_decoder_release(d) == PF2_OK);
    assert(pf2_decoder_acquire(d, &f, 0) == PF2_OK);
    assert(f.plane_y != buf && f.plane_uv != buf + cfg.width * cfg.height);
    assert(f.plane_y[0] == 0x39 && f.plane_uv[0] == 0xB0);
    assert(h264_copy_calls == 2);
    assert(pf2_decoder_release(d) == PF2_OK);
    pf2_decoder_close(d);
    free(buf);
    puts("PASS native AV1/H264 block-linear SDK CopyFrame, direct NV12 and fallback");
    return 0;
}
