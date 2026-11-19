# Linker collision between original Switch 1 window shim and libounce

Evidence: CachyOS log received 17 November 2026, after 19/19 Python tests,
3/3 C tests, Skia Ninja, bindgen C++ header preflight and Rust crates built.
The final `rust-lld` reports ONLY the following duplicate definitions:

- `nwindowGetDefault`: `pf-nx-sys/src/vi.rs:107` vs `libounce.a(default_window.o)`
- `__nx_win_init`: `pf-nx-sys/src/vi.rs:118` vs `libounce.a(default_window.o)`
- `__nx_win_exit`: `pf-nx-sys/src/vi.rs:148` vs `libounce.a(default_window.o)`

Official sys crate has `pub mod vi;` and `pub mod fb;` with no Switch 2 guard.
`fb.rs` calls `crate::vi::default_window()`. The new Rust adapter on Switch 2
uses libounce's own default window and OunceVK; it never needs the legacy
framebuffer or its private static VI display.

Overlay changes only two upstream sys-crate files:

1. `pf-nx-sys/build.rs` declares and emits `cfg(pf2_switch2)` when
   `target_os == horizon` and `PF2_SWITCH2` is set, and asks Cargo to rerun
   the build script if that environment variable changes.
2. `pf-nx-sys/src/lib.rs` guards `pub mod vi;` and `pub mod fb;` using
   `#[cfg(not(pf2_switch2))]`. This removes their definitions from the
   Switch 2 link, not merely masking errors with weak symbols or linker flags.

Rollback recovers official source blobs; Switch 1 remains unchanged.
No Rust cross-toolchain exists in this container to test the real linker.
Host tests check anchor matching and correct cfg gating, and integration
idempotence/user-edit protection. Any later linking errors must be examined
independently before considering any NRO valid.
