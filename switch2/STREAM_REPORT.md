# Switch 2 client stream log regression report

Use the **client-only**, host-independent analyzer after every hardware test.
It reads a Punktfunk log; it does not modify the executable or the server.

```sh
python3 switch2/tooling/stream_log_report.py "/path/to/latest.log"
python3 switch2/tooling/stream_log_report.py "/path/to/latest.log" --compare "/path/to/previous.log"
python3 switch2/tooling/stream_log_report.py "/path/to/latest.log" --json
```

Interpretation:

- `active_115fps_windows` counts reporting windows at 115 fps or more; it is
  **not** guaranteed continuous 120 fps for the entire session.
- Bitrate (`Mb/s`) is traffic generated and received, not the manual target.
  Compare the host's accepted `wire_budget_kbps` separately.
- `distinct_frames` measures newly displayed images; `ticks` includes scanout
  of the last image when the desktop is unchanged. High `cached_gpu_copies`
  can be normal in static scenes.
- `rejected` is a packet count which includes reassembly or anti-replay
  rejection. It must **not** be reported as dropped video frames. Use the
  separate `frames_dropped` and recovery events for actual loss.
- `data receive gap` measures datagram silence, not a proven visible freeze.
  The capture host may be idle. `unpolled_ms` can separate local stalls from
  network/host silence but cannot tell which of host or network is responsible.
- The AV sync offset is diagnostic. Audio device output latency is unknown;
  do not automatically adjust playout from this log alone.
- NV12 `reuses` and `allocations` are cumulative. A plateau in allocations
  shows that the decoder's heap buffer reuse is working.

For meaningful A/B comparisons, use the **same host, game/scene, display mode,
quality settings, duration and input pattern**. Different scenes may have
many static frames and produce lower effective bitrate without any fault.
This report intentionally changes no runtime code paths, no Vulkan fences,
no audio jitter policy, no codec, no bitrate and no frame-rate setting.

## Precise presenter cadence (new builds only)

If the client log contains `present_gap_*`, the report includes return
intervals between successful native present calls. `>2x` counts intervals
at least twice the refresh period, with a separate count for the one-second
windows that also show at least 115 new pictures. In a 120 Hz run, these
intervals deserve investigation, but they are **not** by themselves proof of
an observed freeze or a transport loss. The report still reads old logs.
See `switch2/FRAME_PACING_AB_20261119.md` for the reversible build flags.
