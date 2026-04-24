//! Build script for mesh-cue
//!
//! Force-links libopenmpt and libmpg123 so they appear in the binary's
//! NEEDED list (not just transitively via Essentia → FFmpeg). This puts
//! their symbols in the global resolution scope so the procspawn subprocess
//! can resolve them via lazy PLT lookup.
//!
//! Uses DT_RUNPATH (the modern default) so pw-jack can override libjack
//! via LD_LIBRARY_PATH at runtime. Transitive deps still resolve correctly
//! because libessentia.so has its own RPATH set by Nix.

fn main() {
    // ELF-only linker flags. These are Linux-specific (DT_RPATH, NEEDED entries)
    // and must not be emitted when cross-compiling to Windows.
    let target = std::env::var("TARGET").unwrap_or_default();
    if !target.contains("windows") {
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
