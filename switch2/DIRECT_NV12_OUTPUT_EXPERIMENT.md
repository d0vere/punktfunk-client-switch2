# AV1 120 fps: direct NV12 output (experimental, NOT zero-copy)

## Scope

The known working hardware AV1 path uses `ViddecTilingMode_BlockLinear` and
`videoDecoderCopyFrame()`. The output is *not* a native Vulkan image. The
optional `PF2_DIRECT_NV12_OUTPUT=1` build passes the Rust-managed recycled
NV12 allocation into the native copy operation, removing the previous
`d->y` / `d->uv` -> Rust `Vec` packing memcpy. This avoids one full-frame
CPU copy (3,110,400 bytes at 1080p, up to ~373 MB/s at 120 fps).

**This is not zero-copy decoder-to-display.** The mandatory block-linear
untile and Skia's `Data::new_copy`/texture upload remain. Vulkan also copies
Ganesh output to the swapchain. A validated API to share decoder images
with Vulkan plus GPU fence/ownership integration is a prerequisite for real
end-to-end zero-copy. Do not claim zero-copy without hardware evidence.

## Build

Apply this patch to the repository after the earlier AV1-120, bitrate,
NV12-pool and frame-pacing patches. Test on the host:

    bash ./build-switch2.sh --test

Compile the **current working build** (unchanged):

    PF2_SAFE_STREAM=1 PF2_BOUNDED_PACING=1 bash ./build-switch2.sh

Compile the experimental **direct NV12 output** variant:

    PF2_SAFE_STREAM=1 PF2_BOUNDED_PACING=1 PF2_DIRECT_NV12_OUTPUT=1 bash ./build-switch2.sh

Check log: `direct_nv12=true`, `nv12_pack_us=0` during active streaming.
Compare `native_nv12_copy_us`, `decode to present p95`, presentation gap
and perceived smoothness. Keep AV1 8-bit 1080p120, 100 Mb/s unchanged.
If you see visual corruption, SDK errors, crashes or worse latency, restore
normal mode (unset PF2_DIRECT_NV12_OUTPUT) and rebuild.

**Limitations:** The supplied source archive omits the official Switch 2 SDK
and the compiled toolchain. This path is tested by static/host contracts, not
NRO compilation or hardware. No additional host changes are made.

## H.264 native direct NV12 (Switch 2)

When `PF2_DIRECT_NV12_OUTPUT=1` is set, both **8-bit AV1** and **8-bit
H.264** use the already existing native `pf2_decoder_acquire_into()` API.
Both AV1 and H.264 8-bit now use block-linear hardware output with
`videoDecoderCopyFrame()` into the recycled Rust NV12 buffer. H.264
pitch-linear had failed at SDK `videoDecoderGetFrame()` before any readback.
This removes the redundant native staging buffer -> Rust packing step,
**not** the SDK untile or GPU upload. Hardware H.264 decode still requires
an on-console test to establish the fix.
P010/Main10 and unsupported codecs do not use this path.

To test H.264, do not force the AV1-only diagnostic profile:

    PF2_SAFE_STREAM=0 PF2_BOUNDED_PACING=1 PF2_DIRECT_NV12_OUTPUT=1 bash ./build-switch2.sh

Choose H.264 in the Switch 2 codec menu. During active decode the log should
show `codec=1` (Punktfunk H.264), `direct_nv12=true` and `nv12_pack_us=0`.
Use `PF2_DIRECT_NV12_OUTPUT=0` to rebuild with the legacy staging path if the
hardware test exposes unexpected decoder behavior. AV1-only builds remain
available with `PF2_SAFE_STREAM=1` as before.
