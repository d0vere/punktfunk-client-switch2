// Host-side minimal test SDK for one native decoder-path contract.
// This is NOT the Switch 2 SDK and cannot validate hardware behavior.
#pragma once
#include <stddef.h>
#include <stdint.h>

typedef int Result;
#define R_FAILED(rc) ((rc) != 0)
#define R_SUCCEEDED(rc) ((rc) == 0)
#define R_VALUE(rc) (rc)
#define KERNELRESULT(x) (-234)
#define VIDEO_DECODER_MAX_FRAME_RATE 120u
#define VIDEO_DECODER_DEFAULT_IMAGES 4u
#define VIDDEC_MAX_IMAGES 8u

typedef enum ViddecCodec {
    ViddecCodec_H264 = 0,
    ViddecCodec_Av1 = 3,
} ViddecCodec;
typedef enum ViddecTilingMode {
    ViddecTilingMode_BlockLinear = 1,
    ViddecTilingMode_PitchLinear = 2,
} ViddecTilingMode;
typedef struct VideoDecoderImage {
    void *backing;
    size_t backing_size;
} VideoDecoderImage;
typedef struct VideoDecoder {
    uint32_t image_count;
    VideoDecoderImage images[8];
} VideoDecoder;
typedef struct VideoFrame {
    uint32_t width, height, slot;
    uint8_t *planes[2];
    uint32_t pitches[2];
    int64_t timestamp_ns;
} VideoFrame;
typedef struct VideoDecoderConfig {
    ViddecCodec codec;
    uint32_t width, height, image_count, frame_rate;
    ViddecTilingMode tiling;
    uint32_t color_policy;
    uint64_t max_sample_size;
    const uint8_t *codec_config;
    size_t codec_config_size;
} VideoDecoderConfig;
Result videoDecoderCreate(VideoDecoder*, const VideoDecoderConfig*);
Result videoDecoderSubmit(VideoDecoder*, const uint8_t*, size_t, int64_t, int, uint64_t);
Result videoDecoderGetFrame(VideoDecoder*, VideoFrame*, uint64_t);
Result videoDecoderCopyFrame(VideoDecoder*, VideoFrame*, uint8_t*, uint32_t, uint8_t*, uint32_t);
Result videoDecoderReleaseFrame(VideoDecoder*, VideoFrame*);
Result videoDecoderFlush(VideoDecoder*);
void videoDecoderClose(VideoDecoder*);
uint64_t armGetSystemTick(void);
uint64_t armTicksToNs(uint64_t);
