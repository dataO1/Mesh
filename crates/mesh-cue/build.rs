//! Build script for mesh-cue
//!
//! Two critical linker fixes for the procspawn subprocess:
//!
//! 1. Adds FFmpeg's transitive dependency paths (libopenmpt, mpg123) via -rpath
//! 2. Uses --disable-new-dtags to generate DT_RPATH instead of DT_RUNPATH
//!
//! DT_RPATH is inherited by transitive dependencies, while DT_RUNPATH is not.
//! Without this, the procspawn subprocess (which re-executes the binary) fails
//! with "undefined symbol: mpg123_open_handle64" because the deep transitive
//! chain Essentia → FFmpeg → libopenmpt → mpg123 can't resolve across
//! non-inherited DT_RUNPATH entries on NixOS.

fn main() {
    // Force DT_RPATH instead of DT_RUNPATH so paths are inherited by
    // transitive dependencies in the procspawn subprocess
    println!("cargo:rustc-link-arg=-Wl,--disable-new-dtags");

    // Add libopenmpt's lib dir (FFmpeg transitive dep)
    add_rpath_from_pkg_config("libopenmpt");

    // Add mpg123's lib dir (libopenmpt transitive dep)
    add_rpath_from_pkg_config("libmpg123");
}

fn add_rpath_from_pkg_config(pkg: &str) {
    if let Ok(output) = std::process::Command::new("pkg-config")
        .args(["--libs-only-L", pkg])
        .output()
    {
        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            for token in stdout.split_whitespace() {
                if let Some(path) = token.strip_prefix("-L") {
                    println!("cargo:rustc-link-arg=-Wl,-rpath,{}", path);
                }
            }
        }
    }
}
