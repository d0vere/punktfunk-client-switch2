# Switch 2: frame pacing A/B at AV1 120 Hz

## Why

The November 18 gameplay logs demonstrate AV1 1080p120 at 100 Mb/s manual
budget, 120 decoded frames/s during motion and zero reported missed vblanks.
Brief reductions in distinct pictures and UDP receive gaps are compatible with
perceived micro-stuttering, but do **not** prove a broken Vulkan presenter.
The decoder NV12 pool is working (three allocations, thousands of reuses).

The existing `PF2_BOUNDED_PACING` variable was misleading: `=1` still respected
`spec.smoothing` and therefore left the client in newest-wins mode when the
Latency priority selected smoothing zero. The log continued to show
`pacing_bounded=false`. This patch corrects the override **without changing
anything in the default/unset build**.

## Behavior

- **Unset `PF2_BOUNDED_PACING`**: preserve the stream's smoothing policy, as
  before this patch. Low-latency sessions keep newest-wins (single slot).
- **`PF2_BOUNDED_PACING=0`**: force newest-wins (single slot) even if smoothing
  was requested by the stream. This is the baseline A/B control.
- **`PF2_BOUNDED_PACING=1`**: force a two-picture handoff (one extra pending
  picture). If the older picture waits a full display refresh while a newer
  picture is queued, discard that stale one. This may improve perceived
  regularity under short bursts, but may also add a refresh of latency.
  Neither outcome is guaranteed; check on the device.

No timer, worker thread, artificial sleep, host changes or new capture frame
rate is introduced. Both branches use the same NV12 buffer pool and GPU path.
AV1 8-bit, 120 fps, 100 Mb/s manual target and audio are unchanged.

## Diagnostics

One existing once-per-second `switch2 video displayed` log line now also
reports **successful native present return intervals**:

- `present_gap_samples`: intervals measured in that window
- `present_gap_over_1p5x`: intervals >=1.5 nominal refresh periods
- `present_gap_over_2x`: intervals >=2 nominal refresh periods
- `present_gap_max_us`: maximum measured interval in microseconds

They are *not* frame loss or network drop counts. Idle scenes may reuse the
same image while the present loop continues normally. No per-frame logging
is added and reset/rebuild/reconnect clears prior cadence state.

The regression report accepts both older logs (without cadence counters) and
new ones. For new logs it reports full-stream cadence and also counts the
`>2x` intervals in ~one-second windows with at least 115 distinct frames.

## Reproducible A/B builds (Linux + unified repository + ounce-devkit)

Apply this patch after the four previously applied changes (hardening,
AV1-120 bundle, manual bitrate, NV12 pool); the log analyzer patch is also
expected. From the repository root:

```sh
PATCH="$HOME/Downloads/punktfunk_switch2_frame_pacing_ab_20261119.patch"
git apply --check "$PATCH" && git apply "$PATCH"
bash ./build-switch2.sh --test
```

Test **the same moving scene** at 1920x1080, AV1, 120 fps, manual 100 Mb/s.
Build each mode separately, copy the NRO to the console, play the same scene,
then save its client log before installing the next mode:

```sh
PF2_SAFE_STREAM=1 PF2_BOUNDED_PACING=0 bash ./build-switch2.sh
# Save this NRO/log as the reference before rebuilding the next variant.
PF2_SAFE_STREAM=1 PF2_BOUNDED_PACING=1 bash ./build-switch2.sh
# Run the same test again.
```

The new session line **must** say `bounded=false override=newest-wins` for
mode 0, and `bounded=true override=two-frame` for mode 1. If it does not,
verify which NRO was copied to the console. Use the report:

```sh
python3 switch2/tooling/stream_log_report.py "/path/to/two-frame.log" \
  --compare "/path/to/newest-wins.log"
```

Compare the count of active `present_gap_over_2x`, active decoded/displayed
frames, p95 decode-to-present latency, host/network gaps and subjective input
latency. Because scene content varies, the numbers are only comparable with
matched scene and route. **Do not select the two-picture mode as default**
solely because it reports fewer duplicated frames; it might add latency.

## Validation limits

Source guards and host tests check the override wiring, parser and existing
contracts. Runtime Rust tests cover the mode selector and cadence samples,
but a Rust toolchain and ounce-devkit are not available in this environment;
no NRO was compiled or tested on physical hardware. This is a conservative,
opt-in A/B improvement, not proof of a micro-stutter fix.
