// build.rs — Platform detection for lz4-programs Rust port.
// Migrated from platform.h (lz4-1.10.0/programs).
//
// Emits `cargo:rustc-cfg=has_sparse_files` on Unix targets, corresponding to
// platform.h's SET_SPARSE_FILE_MODE (which is a no-op on POSIX but an
// IOCTL on Windows). On Unix, sparse files are supported natively by the OS;
// on Windows, explicit DeviceIoControl(FSCTL_SET_SPARSE) is needed instead.
fn main() {
    // Sparse file support: available on Unix-like targets (Linux, macOS, BSDs, etc.)
    // SET_SPARSE_FILE_MODE in platform.h is a no-op on POSIX — the OS handles it.
    // On Windows, explicit IOCTL calls are required (handled separately).
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let unix_targets = [
        "linux",
        "macos",
        "freebsd",
        "netbsd",
        "openbsd",
        "dragonfly",
        "solaris",
        "illumos",
        "haiku",
        "android",
        "ios",
        "watchos",
        "tvos",
        "visionos",
    ];
    if unix_targets.contains(&target_os.as_str()) || std::env::var("CARGO_CFG_UNIX").is_ok() {
        println!("cargo:rustc-cfg=has_sparse_files");
    }
}
