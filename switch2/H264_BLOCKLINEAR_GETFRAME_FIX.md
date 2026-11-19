# H.264 hardware decoder: 8-bit block-linear output (experimental fix)

## Device evidence

On 2026-11-19 the H.264 test negotiated `codec=1`, but the decoder's
`videoDecoderGetFrame` failed with `sdk=0xfa66f, stage=2` on pitch-linear
output. This happened *before* Direct NV12 or Skia could receive any frame.
The same console successfully decoded AV1 8-bit using block-linear surfaces
and the SDK `videoDecoderCopyFrame` untiling path. The precise SDK error
meaning has not been independently verified.

## Implementation

- For **both H.264 and AV1 at 8-bit**, request SDK block-linear surfaces with
  `color_policy=0` and use `videoDecoderCopyFrame` to obtain packed NV12.
- `PF2_DIRECT_NV12_OUTPUT=1` passes the Rust pool allocation directly to
  that SDK copy; with it disabled the existing native staging + Rust packing
  path remains available. Neither path is GPU zero-copy.
- The separate experimental AV1 Main10 pitch-linear code remains unchanged.
- Eliminate two unused Rust declarations in release builds: one unused FFI
  function prototype (keep its C implementation/tests), and restrict the
  `h264_dimensions` helper to its unit tests. The active SPS parser remains.
- **No change** to host protocol, H.264 SPS/PPS and IDR bootstrap, AV1,
  120 fps selection, manually selected bitrate, HEVC blocking, or frame pacing.

## Build / testing

Apply after the H.264/HEVC UI bundle and Direct-NV12-for-H.264 patches.
Then, on the Switch 2 toolchain:

```sh
bash ./build-switch2.sh --test
PF2_SAFE_STREAM=0 PF2_BOUNDED_PACING=1 PF2_DIRECT_NV12_OUTPUT=1 bash ./build-switch2.sh
```

Select **H.264** in Settings. The console test must verify **actual pictures**
(`decoded_s > 0` and `distinct_frames > 0`), not only successful negotiation.
Look for `8-bit block-linear CopyFrame`, `codec=1`, `direct_nv12=true` and
`nv12_pack_us=0` in the log. Also validate AV1 at 1080p120, and repeat H.264
with `PF2_DIRECT_NV12_OUTPUT=0` if it still fails, to separate the decoder
problem from the buffer optimization.

If `sdk=0xfa66f, stage=2` persists despite block-linear output, the remaining
candidate is codec framing/configuration or an SDK H.264 limitation. **Do not
claim this is proven fixed without the device test.** The host-side mock
validates the SDK call sequence, not real hardware decode.
