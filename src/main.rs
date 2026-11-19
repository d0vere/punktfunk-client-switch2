//! The title's `main`. On the console it builds the console's platform, NVK first and the
//! framebuffer when NVK does not come up, and runs the app, which never returns.
//!
//! On a developer box it runs the same app on `pf-nx-host`: `--data-dir DIR` or `PF_NX_DATA`
//! names the data directory (identity, settings, logs, bundles), `PF_NX_SIZE` the output
//! (`1280x720` for handheld), `PF_NX_PADS` a scripted pad file. A `test-run.json` in the data
//! directory runs a scenario and leaves its bundle in `verdict/`.

#[cfg(target_os = "horizon")]
fn main() {
    // Official Horizon runtime linker guards retained; old NVK/AV backend is not linked.
    pf_nx_sys::rt::link_guard();
    let platform = pf_nx_switch2_platform::platform().unwrap_or_else(|e| {
        eprintln!("Punktfunk Switch 2 native initialization failed: {e:#}");
        std::process::exit(1)
    });
    // This is the ORIGINAL full frontend, discovery, pairing and QUIC session.
    pf_nx_app::run(platform)
}

#[cfg(not(target_os = "horizon"))]
fn main() -> anyhow::Result<()> {
    let opts = pf_nx_host::Options::from_env();
    let platform = pf_nx_host::platform(&opts)?;
    pf_nx_app::run(platform)
}
