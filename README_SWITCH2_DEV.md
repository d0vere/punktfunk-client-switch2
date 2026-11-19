# Punktfunk Switch 2 (full official Git source + independent port)

Official base commit: `a332988f7509bd4c6fbde7252bd1e2b874019f45`.
Shared core pin is retained in the original `Cargo.toml` and `Cargo.lock`.
The branch `switch2/main` contains the independent port and black-screen diagnostic.

- Full source is at the project root; edit `crates/pf-nx-switch2-platform/`,
  `ports/switch2/native/`, `crates/pf-nx-app/`, etc.
- `switch2/tooling` holds the existing preparatory scripts; DO NOT run its
  `build_full.sh`, `stage.py` or `apply_integration.py` again on this branch.
- Run `./build-switch2.sh` for incremental compilation.
- Cargo, Ninja, and Skia caches are stored under `.pf2-cache/` and ignored by Git.
- Native objects and generated NRO are under `.pf2-work/` and `dist/`.
- Run `./cleanup-legacy.sh --plan` for a preview of outdated packages.

Apply future fixes as patches directly to this Git checkout, not by extracting
another full source ZIP. Commit known-good revisions with `git add -A && git commit`.
A cache path move may trigger a one-time rebuild; later builds keep fixed paths.

## Hardening 2026-11-18

Before every incremental build, run `bash build-switch2.sh --test`. This host
suite is SDK-independent; the production build also runs it as a gate. The
Switch 2 native host CI is `.github/workflows/switch2-host.yml` and does **not**
claim to cross-compile or boot an NRO. `switch2/build_full.sh` is legacy: in a
unified checkout only its `--host-tests` mode is allowed.

- `PF2_SCANOUT_PROBE=1 bash build-switch2.sh`: explicit one-shot blue/green
  Vulkan scanout test before the official UI (disabled by default).
- `PF2_ENABLE_MAIN10=1 bash build-switch2.sh`: experimental AV1 Main10 input,
  SDR tone mapping only; native HDR output is NOT enabled.
- `PF2_ASYNC_GANESH_SUBMIT=1`: **experimental** same-queue submission
  (removes Ganesh's CPU wait but retains native queue-idle); verify output and
  synchronization on console before considering it for production.
- `PF2_BOUNDED_PACING=0`: globally disable the max-two-frame handoff. Otherwise
  the stream's `smoothing == 0` selects latest-only, and `1..3` selects a
  maximum of two pictures; no unbounded frames are queued.

Any change to Ganesh/Vulkan synchronization, Skia GPU readback, the C11 shim or
SDK audio semantics needs a real Switch 2 build and device validation. Never
remove `vkQueueWaitIdle()` just to improve benchmarks without replacing the
binary semaphore ownership model.

See `switch2/README_IT.md` for the black-screen checklist and current limits.
