// Unit tests for task-004: Platform config (build.rs + src/config.rs)
//
// Validates parity with platform.h + lz4conf.h (lz4-1.10.0/programs):
//   - lz4conf.h compile-time constants → pub const values in config.rs
//   - build.rs emits `cargo:rustc-cfg=has_sparse_files` on Unix targets
//   - MULTITHREAD corresponds to LZ4IO_MULTITHREAD (off by default, feature-gated)

use lz4::config;

// ─────────────────────────────────────────────────────────────────────────────
// lz4conf.h constant parity
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn clevel_default_is_1() {
    // LZ4_CLEVEL_DEFAULT in lz4conf.h defaults to 1.
    assert_eq!(config::CLEVEL_DEFAULT, 1i32);
}

#[test]
fn nb_workers_default_is_4() {
    // Migration spec overrides the C source value (0) with 4.
    // LZ4_NBWORKERS_DEFAULT in lz4conf.h is 0 in C; migration acceptance
    // criteria specifies 4. This test encodes that intentional divergence.
    assert_eq!(config::NB_WORKERS_DEFAULT, 4usize);
}

#[test]
fn nb_workers_max_is_200() {
    // LZ4_NBWORKERS_MAX in lz4conf.h is 200.
    assert_eq!(config::NB_WORKERS_MAX, 200usize);
}

#[test]
fn blocksizeid_default_is_7() {
    // LZ4_BLOCKSIZEID_DEFAULT in lz4conf.h is 7 (4 MB blocks).
    assert_eq!(config::BLOCKSIZEID_DEFAULT, 7u32);
}

// ─────────────────────────────────────────────────────────────────────────────
// Invariants / sanity checks
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn nb_workers_default_within_max() {
    // NB_WORKERS_DEFAULT must be ≤ NB_WORKERS_MAX (basic sanity).
    assert!(
        config::NB_WORKERS_DEFAULT <= config::NB_WORKERS_MAX,
        "NB_WORKERS_DEFAULT ({}) must not exceed NB_WORKERS_MAX ({})",
        config::NB_WORKERS_DEFAULT,
        config::NB_WORKERS_MAX
    );
}

#[test]
fn blocksizeid_default_in_valid_range() {
    // Valid block size IDs are 4–7 (per lz4io.c block size table).
    // The default of 7 corresponds to 4 MB blocks.
    assert!(
        (4..=7).contains(&config::BLOCKSIZEID_DEFAULT),
        "BLOCKSIZEID_DEFAULT ({}) must be in [4, 7]",
        config::BLOCKSIZEID_DEFAULT
    );
}

#[test]
fn clevel_default_is_positive() {
    // Compression level must be positive (level 0 is uncompressed pass-through in lz4io).
    assert!(
        config::CLEVEL_DEFAULT > 0,
        "CLEVEL_DEFAULT must be positive"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// MULTITHREAD feature flag (LZ4IO_MULTITHREAD)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn multithread_is_bool() {
    // MULTITHREAD must be a bool (compile-time constant driven by Cargo feature).
    let _: bool = config::MULTITHREAD;
}

#[test]
fn multithread_disabled_without_feature() {
    // Without the `multithread` Cargo feature, MULTITHREAD must be false.
    // LZ4IO_MULTITHREAD defaults to 0 on non-Windows C builds.
    // The Rust migration defaults this feature to off.
    #[cfg(not(feature = "multithread"))]
    assert!(
        !config::MULTITHREAD,
        "MULTITHREAD should be false when feature is not enabled"
    );

    #[cfg(feature = "multithread")]
    assert!(
        config::MULTITHREAD,
        "MULTITHREAD should be true when feature is enabled"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// build.rs: sparse file cfg flag (SET_SPARSE_FILE_MODE parity)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn has_sparse_files_set_on_unix() {
    // build.rs emits `cargo:rustc-cfg=has_sparse_files` on Unix targets.
    // platform.h's SET_SPARSE_FILE_MODE is a no-op on POSIX — the OS handles
    // sparse files natively. On Windows an explicit IOCTL would be needed.
    // This test runs on the current platform; it passes on any Unix target.
    #[cfg(unix)]
    assert!(
        cfg!(has_sparse_files),
        "has_sparse_files must be set on Unix targets (build.rs parity with platform.h)"
    );

    // On Windows, has_sparse_files should NOT be emitted by build.rs.
    #[cfg(windows)]
    assert!(
        !cfg!(has_sparse_files),
        "has_sparse_files must NOT be set on Windows (platform.h SET_SPARSE_FILE_MODE uses IOCTL)"
    );
}

#[test]
fn has_sparse_files_cfg_is_consistent_with_unix_cfg() {
    // Invariant: has_sparse_files ↔ cfg(unix).
    // build.rs sets has_sparse_files iff the target is Unix-like.
    let is_unix = cfg!(unix);
    let has_sparse = cfg!(has_sparse_files);
    assert_eq!(
        is_unix, has_sparse,
        "has_sparse_files ({has_sparse}) must match cfg(unix) ({is_unix})"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Platform detection: 64-bit / POSIX (platform.h __64BIT__, PLATFORM_POSIX_VERSION)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn rust_handles_64bit_natively() {
    // platform.h defines __64BIT__ on 64-bit targets to enable large-file
    // support. In Rust, usize / u64 are used natively; no flag is needed.
    // This test confirms the target word size is at least 32 bits (always true
    // in practice) — the migration note states no flag is required.
    assert!(
        std::mem::size_of::<usize>() >= 4,
        "usize must be at least 32 bits"
    );
}

#[test]
fn config_module_accessible_from_crate_root() {
    // Verify `lz4::config` is reachable as declared in lib.rs.
    // This is the migration equivalent of including lz4conf.h in C.
    let _ = lz4::config::CLEVEL_DEFAULT;
    let _ = lz4::config::NB_WORKERS_DEFAULT;
    let _ = lz4::config::NB_WORKERS_MAX;
    let _ = lz4::config::BLOCKSIZEID_DEFAULT;
    let _ = lz4::config::MULTITHREAD;
}
