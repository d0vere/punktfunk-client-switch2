# AV1 120 Hz: fixed bitrate, host capture, and receive gaps

This document describes the source-only client fix after the 2026-11-18
15 Mb/s test. That test proved the AV1 8-bit path can display a 120-fps
stream, but it did **not** test the user's chosen 100 Mb/s: the old
`PF2_SAFE_STREAM=1` profile overwrote `settings.bitrate_kbps` with `15_000`.

## Client-only correction

The optional build flag now limits **codec negotiation to AV1 8-bit SDR**;
it does **not** change bitrate, resolution, or frame rate. `0` kbps stays
Automatic; any nonzero user setting (for example `100_000` = 100 Mb/s)
is sent to the host unchanged. Resolution and rate still pass through the
console's existing decoder capability check.

```sh
PF2_SAFE_STREAM=1 bash ./build-switch2.sh
```

Verify in the client log:

```text
PF2_SAFE_STREAM AV1-only enabled: requesting 1920x1080@120, 8-bit AV1, user bitrate=100000 kb/s
requesting 1920x1080@120 bitrate_kbps=100000
connected: codec=4 (offered=0x04 preferred=0x04) ... wire_budget_kbps=100000
```

The **wire budget is a target/limit**, not a promise that every window sends
100 Mb/s. Static or easy-to-compress content can use fewer bits. The host
subtracts audio, FEC, and framing overhead from this budget before setting
the encoder rate. Any override from a host/game/launch preset is logged by
`stream profile bitrate override` before the connection starts.

## What the client cannot change by itself

- The Windows host in both logs advertised a stream mode of 1920x1080@120,
  but the AV1 sequence header reported **2560x1440**. The host can encode
  the physical captured desktop at that size; a client request does not
  force a desktop resolution change.
- High fixed rates can bring back packet gaps. The first 100 Mb/s test
  showed up to 2229 ms without received datagrams; the later 15 Mb/s test
  still included idle gaps. Those gaps are not proof of visual freezes.
- The client cannot force the host to emit constant 100 Mb/s or fix a host
  encoder/network fault by changing its local Vulkan presenter.

For comparisons, play the same moving scene at 120 fps with manual settings
15 / 40 / 60 / 100 Mb/s and inspect the server and client logs. Compare
`wire_budget_kbps`, `video_kbps`, `incoming_au_s`, `decoded_s`, `distinct_frames`,
`silence_ms`, `unpolled_ms` and whether any pauses were actually visible.
