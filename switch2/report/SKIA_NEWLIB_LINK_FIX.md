# Link correction: newlib mmap, munmap, pread (17 November 2026)

The user cross-built the exact pinned official `pf-nx` with ounce-devkit, after
23/23 Python tests, the C host tests, Skia GN/Ninja and bindgen passed. The
user's latest log shows `rust-lld` now fails with **only** these undefined
symbols:

- `mmap`: referenced by ICU `umapfile.cpp` and Skia `SkOSFile_posix.cpp`
- `munmap`: referenced by ICU `umapfile.cpp` and Skia `SkOSFile_posix.cpp`
- `pread`: referenced by Skia `SkOSFile_posix.cpp`

The duplicate legacy `nwindowGetDefault`, `__nx_win_init`, `__nx_win_exit`
errors were already resolved by the preceding overlay.

The original pinned official client-switch already ships a targeted newlib
compatibility shim at `toolchain/skia/compat.c` and a custom header at
`toolchain/skia/include/sys/mman.h`. Its script `scripts/build-skia.sh`
adds that shim's object to the Skia static archive when producing official
Skia bundles. Our standalone source build had **not linked** this object.

This fix adapts only that official shim in `native/pf2_skia_compat.c`, compiles
it under `__SWITCH2__` with the official header into `libpf2native.a`, and
adds a strong Rust FFI call to `pf2_skia_compat_link_guard`. Since that
export lives in the same C object as the weak `mmap`, `munmap`, `pread` and
`posix_madvise` definitions, the archive member is extracted regardless of
how a future linker resolves the weak POSIX symbol references. No permissive
link flags, fake ELF, or copied Moonlight/Azkali code are used.

**Important limitation:** this is *not* a kernel-backed mmap. It uses a
`malloc` copy only for read-only, private file mappings. `pread` temporarily
seeks the descriptor and restores the offset; it is not atomic with other
users sharing the file descriptor. Mapping a size past EOF fails instead of
providing POSIX's zero-padded page. `munmap` frees the allocation in full.
These limitations match the intended Skia/ICU file-loading use case, but
need device tests for concurrency, fonts and large files. Unsupported mapping
modes return `ENOSYS` instead of silently providing invalid semantics.

Host validation: Python source/build contracts, Clang host executable with
ASan/UBSan testing the **internal implementations** (not the Linux ASan
interceptor of weak POSIX symbols), and static archive link extraction.
A real AArch64 `rust-lld` run with the user's SDK remains required; no NRO
can be claimed from these tests alone.
