//! Build script for mesh-cue
//!
//! Forces DT_RPATH (instead of the modern default DT_RUNPATH) plus direct
//! NEEDED entries on libopenmpt + libmpg123. This is required for procspawn
//! subprocess library resolution on NixOS.
//!
//! ## The DT_RPATH vs DT_RUNPATH tradeoff (don't undo this without reading)
//!
//! mesh-cue uses `procspawn` to fork a subprocess for Essentia analysis
//! (Essentia is not thread-safe; isolating it in a subprocess prevents
//! corruption from concurrent imports). The subprocess re-execs the binary,
//! ld.so re-runs, and transitive library resolution happens fresh.
//!
//! **DT_RPATH** (set here via `--disable-new-dtags`) is "promiscuous":
//! the binary's RPATH is searched for ALL transitive deps. When libopenmpt
//! internally calls into libmpg123 (`mpg123_open_handle64`), ld.so finds
//! the correct libmpg123 via the binary's RPATH.
//!
//! **DT_RUNPATH** (the modern default, what we'd get without this flag)
//! only applies to the binary's DIRECT NEEDED entries. Transitive lookups
//! (libopenmpt → libmpg123) fall back to libopenmpt's own RPATH which on
//! NixOS points to a different libmpg123 build that lacks `_64` symbols.
//! Result: subprocess crashes with `undefined symbol: mpg123_open_handle64`
//! during track import.
//!
//! ## The tradeoff (accepted)
//!
//! `pw-jack` (PipeWire-JACK shim) injects PipeWire's libjack via
//! LD_LIBRARY_PATH at runtime. With DT_RPATH, the binary's RPATH wins
//! over LD_LIBRARY_PATH, so `pw-jack cargo run -p mesh-cue` cannot
//! redirect libjack to PipeWire-JACK. mesh-cue's JACK backend therefore
//! fails to connect (no JACK server) and falls back to CPAL/ALSA.
//!
//! This is acceptable because:
//! - mesh-cue is the editor app — single-track preview playback only,
//!   doesn't need pro-audio JACK routing
//! - CPAL/ALSA at 1024-frame buffer (~21ms) handles preview reliably
//! - mesh-player (the performance app) does NOT use procspawn, so its
//!   build.rs doesn't set this flag → RUNPATH applies → `pw-jack` works
//!   for mesh-player as expected
//!
//! If you ever remove `--disable-new-dtags` here, track imports will
//! freeze with `mpg123_open_handle64` symbol errors. Don't do that.

fn main() {
    // ELF-only linker flags. These are Linux-specific (DT_RPATH, NEEDED entries)
    // and must not be emitted when cross-compiling to Windows.
    let target = std::env::var("TARGET").unwrap_or_default();
    if !target.contains("windows") {
        // Force DT_RPATH instead of DT_RUNPATH so the binary's RPATH applies
        // to transitive library resolution in the procspawn subprocess.
        // Without this, libopenmpt fails to find mpg123_open_handle64 because
        // its own RPATH points to a different libmpg123 build than ours.
        // See the long-form explanation in the file-level docs above.
        println!("cargo:rustc-link-arg=-Wl,--disable-new-dtags");

        // Force direct linkage to FFmpeg transitive deps — this adds them to
        // the binary's NEEDED list, ensuring they're loaded at startup and
        // their symbols are in the global resolution scope.
        //
        // --no-as-needed prevents the linker from dropping these even though
        // our code doesn't directly reference their symbols.
        for pkg in &["libopenmpt", "libmpg123"] {
            add_forced_link(pkg);
        }
    }
}

/// Add a library as a forced NEEDED entry plus RPATH entry.
/// Uses pkg-config to find the library. Silently skips if not found
/// (e.g., on systems where FFmpeg doesn't link against libopenmpt).
fn add_forced_link(pkg: &str) {
    if let Ok(output) = std::process::Command::new("pkg-config")
        .args(["--libs", pkg])
        .output()
    {
        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            for token in stdout.split_whitespace() {
                if let Some(path) = token.strip_prefix("-L") {
                    // Add search path for link time
                    println!("cargo:rustc-link-search=native={}", path);
                    // Add rpath for runtime
                    println!("cargo:rustc-link-arg=-Wl,-rpath,{}", path);
                } else if let Some(lib) = token.strip_prefix("-l") {
                    // Force as NEEDED even though our code doesn't directly
                    // reference symbols from this library
                    println!("cargo:rustc-link-arg=-Wl,--no-as-needed");
                    println!("cargo:rustc-link-arg=-l{}", lib);
                    println!("cargo:rustc-link-arg=-Wl,--as-needed");
                }
            }
        }
    }
}
