//! Native code is compiled with ounce-devkit by tooling/build_native.sh.
//! This step LINKS its actual library into the official Rust title. No native
//! dependencies are downloaded or injected into global Cargo caches.
use std::env;
use std::path::PathBuf;
fn main() {
    println!("cargo:rerun-if-env-changed=PF2_NATIVE_LIB_DIR");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("horizon") { return; }
    let dir = PathBuf::from(env::var("PF2_NATIVE_LIB_DIR")
        .expect("PF2_NATIVE_LIB_DIR must name the freshly built Switch 2 native library"));
    assert!(dir.join("libpf2native.a").is_file(), "missing freshly compiled libpf2native.a");
    let sdk = PathBuf::from(env::var("DEVKITPRO").unwrap_or_else(|_| "/opt/devkitpro".into()));
    assert!(sdk.join("libounce/include/switch2/video/decoder.h").is_file(),
            "libounce Switch2 VideoDecoder SDK header missing");
    println!("cargo:rerun-if-changed={}", dir.join("libpf2native.a").display());
    println!("cargo:rustc-link-search=native={}", dir.display());
    for d in ["libounce/lib", "portlibs/switch2/lib", "portlibs/switch/lib",
              "devkitA64/aarch64-none-elf/lib/pic"] {
        println!("cargo:rustc-link-search=native={}", sdk.join(d).display());
    }
    if let Ok(entries) = std::fs::read_dir(sdk.join("devkitA64/lib/gcc/aarch64-none-elf")) {
        for entry in entries.flatten() {
            let pic = entry.path().join("pic");
            if pic.is_dir() { println!("cargo:rustc-link-search=native={}", pic.display()); }
        }
    }
    println!("cargo:rustc-link-lib=static=pf2native");
    // OunceVK provides libvulkan; libounce is a libnx-compatible libnx.a.
    println!("cargo:rustc-link-lib=vulkan");
    println!("cargo:rustc-link-lib=nx");
    for lib in ["stdc++", "sysbase", "c", "m", "gcc"] {
        println!("cargo:rustc-link-lib={lib}");
    }
}
