# AV1 120 Hz follow-up: conservative client-only performance patch

## Why this patch exists

The Switch 2 client log from 2026-11-18 (the manual 100 Mb/s run) confirms:

- Hello asked for `1920x1080@120` and `100000` kb/s; Welcome contained
  `codec=4`, `wire_budget_kbps=100000`.
- During sustained active rendering, the decoder reported 120 decoded frames/s,
  the display reported approximately 120 distinct frames/s, and no vblank
  misses. Effective video bitrate was around 87–90 Mb/s in these windows.
- The UI and transport use deliberately low-latency "latest useful picture"
  replacement. A `replaced` picture is **not** proof of a visible stutter.
- The audio route measured approximately +30 ms A/V offset during some
  windows, **but** the native Audout device output latency is unknown. A
  measurement by itself does not authorize automatically delaying the video
  or changing the audio clock.

These are **observations**, not a guarantee of permanent 120 fps or a claim
that this implementation is identical to Moonlight-NX2.

## Implemented, client only

1. A session-local pool recycles CPU NV12/P010 packed allocations **after**
   `VideoPicture` is dropped. This avoids repeated 3 MiB (1080p) / 5.5 MiB
   (1440p) heap allocations during steady-state 120-fps playback. Frames
   remain independently owned and fully copied from the decoder staging
   planes. We do **not** borrow SDK memory across a release, nor try unsafe
   zero-copy into Vulkan.
2. The pool holds at most 2 retired buffers and 32 MiB total; buffers are
   returned on frame replacement, normal display replacement, flush, or
   disconnect. A pool's lifetime is bound to its decoder session, and old
   sessions cannot pollute new ones.
3. Once-per-second decoder logs include cumulative `nv12_pool_reuses` and
   `nv12_pool_allocations`, making it possible to check actual reuse.
4. Audio logs include estimated `audio_ahead_ms` and `video_e2e_ms` ranges;
   they are **diagnostic only** and do not alter jitter policy or sync.
5. Extra Rust unit tests and host source guards check memory limits, reuse,
   thread handoff, compile-time AV1/120 negotiation, and unchanged GPU
   upload ownership.

## Intentionally unchanged

- **AV1 8-bit** and **120 fps**: no forced H.264, no 60 Hz cap.
- Manual bitrate: `100000` kb/s remains `100000` in the request.
- The two-frame bounded/latest-wins policy. It is already test-covered and
  intentionally trades a small number of overwritten pictures for lower lag.
- Ganesh `Data::new_copy`, submit synchronization and native `CopyFrame`:
  these are ownership boundaries, not safely removable memcpy calls without
  replacing them with a real fence-backed GPU/decoder lifetime protocol.
- Audio output correction: no blind +/-30 ms calibration. An on-device
  loopback or synchronized photodiode/microphone capture is needed to
  establish actual audible A/V delay before applying a policy change.

## Apply after previous patches

This unified diff is relative to the previously delivered **AV1 120 + user
bitrate** client patch, as verified in the user log. It is not a replacement
for the hardening or bitrate patch. In the unified Git checkout root:

```sh
PATCH="$HOME/Downloads/punktfunk_switch2_av1_120_perf_followup.patch"
git apply --check "$PATCH" && git apply "$PATCH"
bash ./build-switch2.sh --test
PF2_SAFE_STREAM=1 bash ./build-switch2.sh
```

Run the test and build commands as *separate commands*, so no NRO is shipped
when the test gate fails. The first applies the patch only when it matches
this exact source revision. If `git apply --check` fails, stop and inspect
`git status --short`; do not force an unrelated Git tree.

## Acceptance checks on the Switch 2

- `codec=4 (offered=0x04 preferred=0x04)`, `wire_budget_kbps=100000`.
- Stream set to 1920x1080@120; log reports `incoming=120 fps`.
- During a moving scene, `decoded_s` stays near 120, and
  `distinct_frames` stays near `ticks` at the expected 120 Hz scanout.
- `nv12_pool_reuses` increases over time and `nv12_pool_allocations`
  flattens after warm-up; if it keeps increasing, investigate retained
  video frames and the number of in-flight buffers.
- Compare p95 decode-to-present, input cadence, audio underruns and any
  `data receive gap` events against the preceding build. The patch must not
  degrade subjective responsiveness or cause more drops.
- Verify HOME/suspend/resume, decoder resolution changes, stream close,
  and 2+ repeated connects; the buffer pool must not retain large frames
  indefinitely.

## Validation limits

Host source-contract suite and native C tests can be run outside Horizon.
This environment cannot compile the NRO (no Rust nightly or ounce-devkit)
and has no physical Switch 2. Rust unit tests for the pool are included but
must be compiled by the user in their configured toolchain/CI. This patch
improves one known allocation path and observability; it is **not** proof
of Moonlight-NX2 feature parity, nor a full asynchronous Vulkan rewrite.
