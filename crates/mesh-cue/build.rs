//! Build script for mesh-cue
//!
//! Fixes for the procspawn subprocess library resolution:
//!
//! 1. Force-links libopenmpt and libmpg123 so they appear in the binary's
//!    NEEDED list (not just transitively via Essentia → FFmpeg)
//! 2. Adds their library dirs to RPATH via -rpath
//! 3. Uses --disable-new-dtags to generate DT_RPATH instead of DT_RUNPATH
//!
//! The procspawn subprocess re-executes the binary. Without direct NEEDED
//! entries, the transitive chain (Essentia → FFmpeg → libopenmpt → mpg123)
//! can fail lazy PLT resolution in the subprocess on NixOS even though
//! all libraries are loaded — the symbols aren't in the right resolution
//! scope unless the binary directly depends on them.

fn main() {
    // Force DT_RPATH instead of DT_RUNPATH so paths are inherited by
    // transitive dependencies in the procspawn subprocess
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
