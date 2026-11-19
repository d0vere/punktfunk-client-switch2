// Host-only fixed-width Rust <-> C ABI check. No libounce headers required.
#include "pf2_native.h"
#include <stddef.h>
#include <stdio.h>

_Static_assert(sizeof(Pf2DecodedFrame) == 40, "Rust Frame size mismatch");
_Static_assert(offsetof(Pf2DecodedFrame, pts_ns) == 16, "Frame PTS offset mismatch");
_Static_assert(offsetof(Pf2DecodedFrame, plane_y) == 24, "Frame Y pointer offset mismatch");
_Static_assert(offsetof(Pf2DecodedFrame, plane_uv) == 32, "Frame UV pointer offset mismatch");
_Static_assert(sizeof(Pf2DecoderSettings) == 48, "Rust DecoderSettings size mismatch");
_Static_assert(offsetof(Pf2DecoderSettings, codec_data) == 24, "Codec pointer offset mismatch");
_Static_assert(offsetof(Pf2DecoderSettings, codec_data_length) == 32, "Codec data len offset mismatch");
_Static_assert(offsetof(Pf2DecoderSettings, max_sample_bytes) == 40, "Sample size offset mismatch");
_Static_assert(sizeof(Pf2PadRaw) == 40, "Rust PadRaw size mismatch");
_Static_assert(offsetof(Pf2PadRaw, style) == 8, "Pad style offset mismatch");
_Static_assert(offsetof(Pf2PadRaw, buttons) == 16, "Pad buttons offset mismatch");
_Static_assert(sizeof(Pf2MotionRaw) == 24, "Rust MotionRaw size mismatch");
_Static_assert(offsetof(Pf2MotionRaw, gyro) == 12, "Motion gyro offset mismatch");
_Static_assert(offsetof(Pf2PadRaw, right_y) == 36, "Pad axis offset mismatch");

// Dispatchable Vulkan handles are pointers on the Switch 2 64-bit target.
#if UINTPTR_MAX == UINT64_MAX
_Static_assert(sizeof(Pf2VkBorrowedContext) == 40, "Vulkan borrow ABI size mismatch");
_Static_assert(offsetof(Pf2VkBorrowedContext, queue_family) == 32, "Vulkan queue family offset mismatch");
_Static_assert(offsetof(Pf2VkBorrowedContext, physical_api_version) == 36, "Vulkan API version offset mismatch");
#endif

int main(void) {
    puts("PASS Fixed-width FFI C ABI struct sizes and offsets");
    return 0;
}
