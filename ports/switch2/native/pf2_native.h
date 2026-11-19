// SPDX-License-Identifier: MIT OR Apache-2.0
// Independent Switch 2 native subsystem interface; no Moonlight sources are used.
#pragma once
#include <stddef.h>
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif

typedef struct Pf2Decoder Pf2Decoder;
typedef struct Pf2Presenter Pf2Presenter;
typedef struct Pf2Audio Pf2Audio;

typedef enum Pf2Error {
    PF2_OK = 0,
    PF2_INVALID = -1,
    PF2_NO_MEMORY = -2,
    PF2_SDK_ERROR = -3,
    PF2_NOT_READY = -4,
    PF2_UNSUPPORTED = -5,
    PF2_OUT_OF_DATE = -6
} Pf2Error;

typedef struct Pf2DecodedFrame {
    uint32_t width;
    uint32_t height;
    uint32_t pitch_y;
    uint32_t pitch_uv;
    uint64_t pts_ns;
    const uint8_t *plane_y;
    const uint8_t *plane_uv;
} Pf2DecodedFrame;

typedef struct Pf2DecoderSettings {
    uint32_t width;
    uint32_t height;
    uint32_t fps;
    uint32_t codec;       // 0=H264, 3=AV1 (verified libounce ViddecCodec values)
    uint32_t image_count;
    uint32_t bit_depth; // 8=NV12; 10=P010; occupies existing ABI padding at offset 20
    const uint8_t *codec_data;   // H264: Annex B SPS/PPS configuration
    size_t codec_data_length;
    uint64_t max_sample_bytes;
} Pf2DecoderSettings;

// Decoder calls are synchronous and must be serialized by the caller.
// acquire() copies the SDK's block-linear frame into decoder-owned linear
// staging buffers before releasing the SDK frame. The returned plane pointers
// belong to that staging area: they remain valid until the next successful
// acquire(), flush(), or close(), NOT merely until pf2_decoder_release().
// The Rust caller must copy them while holding the decoder mutex; it must not
// retain raw pointers across any subsequent decoder operation. Exactly one
// result at a time is supported. release() only marks that result consumed.
Pf2Error pf2_decoder_open(Pf2Decoder **out, const Pf2DecoderSettings *settings);
Pf2Error pf2_decoder_submit(Pf2Decoder *decoder, const uint8_t *data,
                            size_t length, int64_t pts_ns, uint64_t timeout_ns);
Pf2Error pf2_decoder_acquire(Pf2Decoder *decoder, Pf2DecodedFrame *out,
                             uint64_t timeout_ns);
// EXPERIMENTAL: 8-bit AV1 and H.264 block-linear SDK CopyFrame both write
// linear NV12 directly into a caller-owned buffer. This removes the second
// CPU-side copy, NOT SDK untiling or the Skia GPU upload. The caller must
// supply at least width*height*3/2 bytes and release the SDK image normally.
// No HEVC decoder is exposed by the current libounce ViddecCodec.
// The returned plane pointers alias `packed` and become invalid if it moves.
Pf2Error pf2_decoder_acquire_into(Pf2Decoder *decoder, Pf2DecodedFrame *out,
                                   uint8_t *packed, size_t capacity,
                                   uint64_t timeout_ns);
Pf2Error pf2_decoder_release(Pf2Decoder *decoder);
Pf2Error pf2_decoder_flush(Pf2Decoder *decoder);
void pf2_decoder_close(Pf2Decoder *decoder);
uint32_t pf2_decoder_last_sdk_result(const Pf2Decoder *decoder);
// Last SDK failure stage: 1=submit, 2=get-frame, 3=copy-frame, 4=flush,
// 5=output-layout validation. Only diagnostic; not a protocol field.
uint32_t pf2_decoder_last_sdk_stage(const Pf2Decoder *decoder);
// Duration of the NV12 surface copy during the last successful frame acquire.
uint32_t pf2_decoder_last_copy_us(const Pf2Decoder *decoder);

typedef enum Pf2ColorMatrix { PF2_BT601 = 0, PF2_BT709 = 1 } Pf2ColorMatrix;
// Source planes may have larger strides. Destination RGBA is packed with its own stride.
// Width and height must be even for 4:2:0. Pointer capacities are validated.
Pf2Error pf2_nv12_rgba(const uint8_t *y, size_t y_bytes, size_t y_stride,
                       const uint8_t *uv, size_t uv_bytes, size_t uv_stride,
                       uint32_t width, uint32_t height,
                       uint8_t *rgba, size_t rgba_bytes, size_t rgba_stride,
                       Pf2ColorMatrix matrix, int full_range);

Pf2Error pf2_presenter_open(Pf2Presenter **out, uint32_t width, uint32_t height);
// Emergency 1080p layer if TV-native Vulkan setup cannot be initialized.
Pf2Error pf2_presenter_open_fixed(Pf2Presenter **out, uint32_t width, uint32_t height);
// Borrowed Vulkan handles for Skia Ganesh. The presenter retains ownership of
// instance/device/queue; the caller MUST destroy every Ganesh resource before
// pf2_presenter_close(), including when rebuilding after a display-mode change.
// These are dispatchable Vulkan handles (pointers) on the AArch64 target.
typedef struct Pf2VkBorrowedContext {
    void *instance;
    void *physical_device;
    void *device;
    void *queue;
    uint32_t queue_family;
    uint32_t physical_api_version;
} Pf2VkBorrowedContext;
Pf2Error pf2_presenter_vk_borrow(const Pf2Presenter *presenter, Pf2VkBorrowedContext *out);
// Read-only capability probe: whether the active VI/HDMI surface advertises
// an HDR10 PQ + 10-bit format. Does not change swapchain or scan-out mode.
Pf2Error pf2_presenter_hdr10_surface(const Pf2Presenter *presenter, uint32_t *supported);
// The two OunceVK procedure resolvers use the presenter's existing Vulkan loader.
// Returned pointers are only valid while the Vulkan instance/device remain alive.
const void *pf2_vk_instance_proc(void *instance, const char *name);
const void *pf2_vk_device_proc(void *device, const char *name);
// rgba pointer can be freed as soon as this function returns.
Pf2Error pf2_presenter_frame(Pf2Presenter *presenter,
                             const uint8_t *rgba, size_t bytes, size_t stride);
// The presenter owns the VkImage and its device memory. Skia may BORROW the
// image only while the presenter/device live; drop SkSurface and DirectContext
// before pf2_presenter_close. The image starts in VK_IMAGE_LAYOUT_UNDEFINED.
typedef struct Pf2VkOffscreenImage {
    uint64_t image;
    uint32_t format;
    uint32_t width, height;
    uint32_t queue_family;
} Pf2VkOffscreenImage;
// One offscreen image per presenter, in exactly the swapchain's UNORM format.
// Usage: COLOR_ATTACHMENT | TRANSFER_SRC | TRANSFER_DST (Skia requirement);
// sample count = 1, optimal tiling. Native copy reads this image only.
Pf2Error pf2_presenter_gpu_image(Pf2Presenter *presenter, Pf2VkOffscreenImage *out);
// The Skia caller MUST flush to TRANSFER_SRC_OPTIMAL and wait for completion
// before calling. Same VkQueue, no queue-family ownership transfer required.
Pf2Error pf2_presenter_frame_gpu(Pf2Presenter *presenter);
void pf2_presenter_close(Pf2Presenter *presenter);

Pf2Error pf2_audio_open(Pf2Audio **out, uint32_t rate, uint32_t channels);
// Blocks until this grain is played. Interleaved float PCM [-1,+1].
Pf2Error pf2_audio_write(Pf2Audio *audio, const float *pcm, size_t frames);
void pf2_audio_close(Pf2Audio *audio);

// Raw HID samples from libounce. Deliberately not wire-format gamepad messages:
// the official application's pad-map must perform protocol mapping later.
typedef struct Pf2Input Pf2Input;
typedef struct Pf2PadRaw {
    uint32_t slot;
    uint32_t connected;
    // 0=other, 1=handheld Joy-Cons, 2=JoyDual, 3=JoyLeft,
    // 4=JoyRight, 5=FullKey (Switch Pro). Values are private to this FFI.
    uint32_t style;
    uint64_t buttons;
    int32_t left_x, left_y, right_x, right_y;
} Pf2PadRaw;
Pf2Error pf2_input_open(Pf2Input **out);
// Slots match the console's No1..No4 numbering. Handheld Joy-Cons join No1.
Pf2Error pf2_input_poll(Pf2Input *input, Pf2PadRaw out_pads[4]);
// Whether physical Joy-Cons are currently attached to the console.
uint32_t pf2_input_handheld_attached(const Pf2Input *input);
// Latest six-axis sample of the controller in this player slot, starts lazily.
// The native SDK reports angular velocity in rotations/s and acceleration in g.
// A Joy-Con pair uses the right controller's sensor, as in Moonlight NX2.
typedef struct Pf2MotionRaw {
    float accel[3];
    float gyro[3];
} Pf2MotionRaw;
Pf2Error pf2_input_motion(Pf2Input *input, uint32_t slot, Pf2MotionRaw *out);
// Touch coordinates are the Switch panel's 1280x720 pixels, not the TV output.
// `attributes & 2` marks a released finger; don't synthesize a new Down for it.
typedef struct Pf2TouchRaw {
    uint32_t finger_id, attributes, x, y;
} Pf2TouchRaw;
Pf2Error pf2_input_touches(Pf2Input *input, Pf2TouchRaw out_touches[16], uint32_t *count);
Pf2Error pf2_input_rumble(Pf2Input *input, uint32_t slot, uint16_t low, uint16_t high);
void pf2_input_release_all(Pf2Input *input);
void pf2_input_close(Pf2Input *input);

// Extract H.264 parameter sets from Annex B access units for videoDecoderCreate.
// Result is two start-coded NAL units (SPS then PPS); insufficient capacity is an error.
Pf2Error pf2_h264_config_from_annexb(const uint8_t *access_unit, size_t au_size,
                                    uint8_t *config, size_t capacity, size_t *written);

/* Runtime is allocated once per title. Networking is opened before Quinn starts;
 * closes only on actual app exit AFTER the official session shutdown. */
typedef struct Pf2Runtime Pf2Runtime;
// Crash breadcrumbs live at sdmc:/switch/punktfunk-boot.log, outside the data dir.
void pf2_runtime_boot_trace(const char *stage);
// Prepare the data/log directories; an existing directory is not an error.
Pf2Error pf2_runtime_prepare_storage(void);
Pf2Error pf2_runtime_open(Pf2Runtime **out);
// Diagnostic for the active libounce BSD transfer buffers. This does not
// change an established connection or modify the official QUIC protocol.
typedef struct Pf2SocketTuning {
    uint32_t boosted;          // 1: enlarged BSD UDP transfer buffers; 0: SDK defaults
    uint32_t boost_result;     // Result of initial boosted init, 0 on success
    uint32_t requested_tx;     // Effective SocketInitConfig UDP TX buffer, bytes
    uint32_t requested_rx;     // Effective SocketInitConfig UDP RX buffer, bytes
    int32_t socket_sndbuf;     // SO_SNDBUF from a fresh UDP socket, -1 on failure
    int32_t socket_rcvbuf;     // SO_RCVBUF from a fresh UDP socket, -1 on failure
} Pf2SocketTuning;
Pf2Error pf2_runtime_socket_tuning(const Pf2Runtime *r, Pf2SocketTuning *out);
// One-shot diagnostic of the native libounce BSD socket path. Never contacts a host.
// -2 means the stage was not attempted; errno is captured immediately on failure.
typedef struct Pf2NetProbe {
    int32_t socket_rc, socket_errno;
    uint32_t socket_bsd_result;
    int32_t bind_rc, bind_errno;
    uint32_t bind_bsd_result;
    int32_t nonblock_rc, nonblock_errno;
    uint32_t nonblock_bsd_result;
} Pf2NetProbe;
Pf2Error pf2_runtime_net_probe(const Pf2Runtime *r, Pf2NetProbe *out);
// Diagnoses the same Rust-created UDP descriptor through native ioctl(FIONBIO).
// Borrowed fd: this function must never close it.
Pf2Error pf2_runtime_nonblock_fd(int32_t fd, int32_t *ioctl_rc,
    int32_t *ioctl_errno, uint32_t *bsd_result);
/* Flags: 1=focus lost, 2=focus gained, 4=suspend, 8=resume, 16=mode change, 32=exit request.
 * On exit-request return SUSPEND and leave the app's official shutdown to run. */
Pf2Error pf2_runtime_poll(Pf2Runtime *r, uint32_t *flags, uint32_t *docked);
uint32_t pf2_runtime_docked(const Pf2Runtime *r);
uint32_t pf2_runtime_applet(const Pf2Runtime *r);
void pf2_runtime_close(Pf2Runtime *r);
/* Actual OunceVK swapchain extent; may differ from the requested extent. */
Pf2Error pf2_presenter_size(const Pf2Presenter *p, uint32_t *width, uint32_t *height);
// Stage an explicit docked native-window size and 60/120 Hz request.
// width=height=0 restores the system's own TV output preference.
// The request takes effect on pf2_presenter_resize(), after Ganesh is dropped.
Pf2Error pf2_presenter_request_tv_mode(Pf2Presenter *p, uint32_t width,
                                        uint32_t height, uint32_t fps);
// Recreate only the swapchain on the same device, after Rust drops Ganesh.
Pf2Error pf2_presenter_resize(Pf2Presenter *p);
/* Current native-window dimensions (1920x1080 handheld; docked follows TV mode).
 * policy_rc is the initial AM resolution-subscription result, 0 on success. */
Pf2Error pf2_presenter_output_size(const Pf2Presenter *p, uint32_t *width,
                                    uint32_t *height, uint32_t *policy_rc);
// Actual VI2 nominal refresh (60 or 120 Hz), not a requested/assumed rate.
Pf2Error pf2_presenter_refresh(const Pf2Presenter *p, uint32_t *hz, uint32_t *policy_rc);

#ifdef __cplusplus
}
#endif
