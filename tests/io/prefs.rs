// Integration tests for task-011: io/prefs.rs — LZ4IO Preferences
//
// Verifies behavioural parity with lz4io.c v1.10.0, lines 1–345 and lz4io.h:
//   - Numeric/magic constants
//   - `g_displayLevel`            → `DISPLAY_LEVEL` (AtomicI32)
//   - `LZ4IO_setNotificationLevel` → `set_notification_level`
//   - `LZ4IO_defaultNbWorkers`     → `default_nb_workers`
//   - `LZ4IO_prefs_s` / `LZ4IO_defaultPreferences` → `Prefs` / `Prefs::default`
//   - All 14 preference setters
//   - `LZ4IO_blockMode_t`          → `BlockMode` enum
//   - `cpuLoad_sec`                → `cpu_load_sec`

use lz4::io::prefs::{
    cpu_load_sec, default_nb_workers, display_level, set_notification_level, BlockMode, Prefs,
    CACHELINE, DISPLAY_LEVEL, GB, KB, LEGACY_BLOCKSIZE, LEGACY_MAGICNUMBER,
    LZ4IO_BLOCKSIZEID_DEFAULT, LZ4IO_MAGICNUMBER, LZ4IO_SKIPPABLE0, LZ4IO_SKIPPABLEMASK,
    LZ4_MAX_DICT_SIZE, MAGICNUMBER_SIZE, MB, MIN_STREAM_BUFSIZE, REFRESH_RATE_NS,
};
use std::sync::atomic::Ordering;

// ─────────────────────────────────────────────────────────────────────────────
// Numeric / size constants  (lz4io.c lines 69–71)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn constant_kb_is_1024() {
    assert_eq!(KB, 1024);
}

#[test]
fn constant_mb_is_1048576() {
    assert_eq!(MB, 1 << 20);
}

#[test]
fn constant_gb_is_correct() {
    assert_eq!(GB, 1 << 30);
}

#[test]
fn constant_cacheline_is_64() {
    assert_eq!(CACHELINE, 64);
}

#[test]
fn constant_legacy_blocksize_is_8mb() {
    assert_eq!(LEGACY_BLOCKSIZE, 8 * MB);
}

#[test]
fn constant_min_stream_bufsize_is_192kb() {
    assert_eq!(MIN_STREAM_BUFSIZE, 192 * KB);
}

#[test]
fn constant_lz4_max_dict_size_is_64kb() {
    assert_eq!(LZ4_MAX_DICT_SIZE, 64 * KB);
}

// ─────────────────────────────────────────────────────────────────────────────
// Magic numbers  (lz4io.c lines 79–83)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn magic_number_size_is_4() {
    assert_eq!(MAGICNUMBER_SIZE, 4);
}

#[test]
fn lz4io_magicnumber_value() {
    assert_eq!(LZ4IO_MAGICNUMBER, 0x184D_2204);
}

#[test]
fn lz4io_skippable0_value() {
    assert_eq!(LZ4IO_SKIPPABLE0, 0x184D_2A50);
}

#[test]
fn lz4io_skippablemask_value() {
    assert_eq!(LZ4IO_SKIPPABLEMASK, 0xFFFF_FFF0);
}

#[test]
fn legacy_magicnumber_value() {
    assert_eq!(LEGACY_MAGICNUMBER, 0x184C_2102);
}

/// Any frame magic with the lower 4 bits cleared should be recognised as
/// skippable (C: `(magic & LZ4IO_SKIPPABLEMASK) == LZ4IO_SKIPPABLE0`).
#[test]
fn skippable_mask_matches_skippable0_family() {
    // LZ4IO_SKIPPABLE0 itself
    assert_eq!(LZ4IO_SKIPPABLE0 & LZ4IO_SKIPPABLEMASK, LZ4IO_SKIPPABLE0);
    // LZ4IO_SKIPPABLE0 + 0xF is still in the same family
    assert_eq!(
        (LZ4IO_SKIPPABLE0 + 0xF) & LZ4IO_SKIPPABLEMASK,
        LZ4IO_SKIPPABLE0
    );
}

#[test]
fn refresh_rate_is_200ms_in_nanoseconds() {
    assert_eq!(REFRESH_RATE_NS, 200_000_000);
}

// ─────────────────────────────────────────────────────────────────────────────
// Default block-size ID  (lz4io.c line 87)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn block_size_id_default_is_7() {
    assert_eq!(LZ4IO_BLOCKSIZEID_DEFAULT, 7);
}

// ─────────────────────────────────────────────────────────────────────────────
// BlockMode enum  (lz4io.h lines 104–105)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn block_mode_linked_discriminant_is_0() {
    assert_eq!(BlockMode::Linked as i32, 0);
}

#[test]
fn block_mode_independent_discriminant_is_1() {
    assert_eq!(BlockMode::Independent as i32, 1);
}

#[test]
fn block_mode_equality() {
    assert_eq!(BlockMode::Linked, BlockMode::Linked);
    assert_eq!(BlockMode::Independent, BlockMode::Independent);
    assert_ne!(BlockMode::Linked, BlockMode::Independent);
}

#[test]
fn block_mode_copy_clone() {
    let a = BlockMode::Independent;
    let b = a;
    let c = a.clone();
    assert_eq!(b, BlockMode::Independent);
    assert_eq!(c, BlockMode::Independent);
}

#[test]
fn block_mode_debug_does_not_panic() {
    let _ = format!("{:?}", BlockMode::Linked);
    let _ = format!("{:?}", BlockMode::Independent);
}

// ─────────────────────────────────────────────────────────────────────────────
// DISPLAY_LEVEL / set_notification_level  (lz4io.c lines 100, 315–319)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn set_notification_level_returns_value_stored() {
    let ret = set_notification_level(2);
    assert_eq!(ret, 2);
    // Clean up
    set_notification_level(0);
}

#[test]
fn set_notification_level_updates_display_level_atomic() {
    set_notification_level(5);
    assert_eq!(DISPLAY_LEVEL.load(Ordering::Relaxed), 5);
    set_notification_level(0);
}

#[test]
fn set_notification_level_negative_is_stored() {
    // C code stores whatever int is given; Rust should do the same.
    let ret = set_notification_level(-1);
    assert_eq!(ret, -1);
    assert_eq!(DISPLAY_LEVEL.load(Ordering::Relaxed), -1);
    set_notification_level(0);
}

#[test]
fn display_level_suppressed_below_threshold() {
    // Level set to 0; calling display_level(1, …) must be a no-op (no panic).
    set_notification_level(0);
    display_level(1, "should not panic\n");
    set_notification_level(0);
}

#[test]
fn display_level_executes_at_matching_level() {
    // Just verifies no panic when the level matches (output goes to stderr).
    set_notification_level(2);
    display_level(2, ""); // empty message — no visible output
    set_notification_level(0);
}

// ─────────────────────────────────────────────────────────────────────────────
// default_nb_workers  (lz4io.c lines 167–177)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn default_nb_workers_is_at_least_1() {
    assert!(default_nb_workers() >= 1);
}

#[test]
fn default_nb_workers_does_not_exceed_200() {
    // NB_WORKERS_MAX = 200; even on a very large machine the function must
    // stay at or below this limit.
    assert!(default_nb_workers() <= 200);
}

// ─────────────────────────────────────────────────────────────────────────────
// Prefs defaults  (lz4io.c lines 206–226, LZ4IO_defaultPreferences)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn prefs_new_equals_default() {
    let a = Prefs::new();
    let b = Prefs::default();
    // Compare field-by-field (no PartialEq derived, but fields are public).
    assert_eq!(a.pass_through, b.pass_through);
    assert_eq!(a.overwrite, b.overwrite);
    assert_eq!(a.test_mode, b.test_mode);
    assert_eq!(a.block_size_id, b.block_size_id);
    assert_eq!(a.block_size, b.block_size);
    assert_eq!(a.block_checksum, b.block_checksum);
    assert_eq!(a.stream_checksum, b.stream_checksum);
    assert_eq!(a.block_independence, b.block_independence);
    assert_eq!(a.sparse_file_support, b.sparse_file_support);
    assert_eq!(a.content_size_flag, b.content_size_flag);
    assert_eq!(a.use_dictionary, b.use_dictionary);
    assert_eq!(a.favor_dec_speed, b.favor_dec_speed);
    assert_eq!(a.dictionary_filename, b.dictionary_filename);
    assert_eq!(a.remove_src_file, b.remove_src_file);
    assert_eq!(a.nb_workers, b.nb_workers);
}

#[test]
fn prefs_default_pass_through_is_false() {
    assert!(!Prefs::default().pass_through);
}

#[test]
fn prefs_default_overwrite_is_true() {
    assert!(Prefs::default().overwrite);
}

#[test]
fn prefs_default_test_mode_is_false() {
    assert!(!Prefs::default().test_mode);
}

#[test]
fn prefs_default_block_size_is_zero() {
    assert_eq!(Prefs::default().block_size, 0);
}

#[test]
fn prefs_default_block_checksum_is_false() {
    assert!(!Prefs::default().block_checksum);
}

#[test]
fn prefs_default_stream_checksum_is_true() {
    assert!(Prefs::default().stream_checksum);
}

#[test]
fn prefs_default_block_independence_is_true() {
    assert!(Prefs::default().block_independence);
}

#[test]
fn prefs_default_sparse_file_support_is_1() {
    assert_eq!(Prefs::default().sparse_file_support, 1);
}

#[test]
fn prefs_default_content_size_flag_is_false() {
    assert!(!Prefs::default().content_size_flag);
}

#[test]
fn prefs_default_use_dictionary_is_false() {
    assert!(!Prefs::default().use_dictionary);
}

#[test]
fn prefs_default_favor_dec_speed_is_false() {
    assert!(!Prefs::default().favor_dec_speed);
}

#[test]
fn prefs_default_dictionary_filename_is_none() {
    assert!(Prefs::default().dictionary_filename.is_none());
}

#[test]
fn prefs_default_remove_src_file_is_false() {
    assert!(!Prefs::default().remove_src_file);
}

#[test]
fn prefs_default_nb_workers_matches_default_nb_workers() {
    assert_eq!(Prefs::default().nb_workers, default_nb_workers());
}

#[test]
fn prefs_default_block_size_id_is_7() {
    assert_eq!(Prefs::default().block_size_id, LZ4IO_BLOCKSIZEID_DEFAULT);
}

// ─────────────────────────────────────────────────────────────────────────────
// set_nb_workers  (lz4io.c lines 228–234)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn set_nb_workers_clamps_zero_to_one() {
    let mut p = Prefs::default();
    assert_eq!(p.set_nb_workers(0), 1);
    assert_eq!(p.nb_workers, 1);
}

#[test]
fn set_nb_workers_clamps_negative_to_one() {
    let mut p = Prefs::default();
    assert_eq!(p.set_nb_workers(-5), 1);
    assert_eq!(p.nb_workers, 1);
}

#[test]
fn set_nb_workers_clamps_large_to_max() {
    let mut p = Prefs::default();
    let ret = p.set_nb_workers(i32::MAX);
    assert_eq!(ret, 200); // NB_WORKERS_MAX = 200
    assert_eq!(p.nb_workers, 200);
}

#[test]
fn set_nb_workers_accepts_valid_value() {
    let mut p = Prefs::default();
    assert_eq!(p.set_nb_workers(4), 4);
    assert_eq!(p.nb_workers, 4);
}

#[test]
fn set_nb_workers_returns_stored_value() {
    let mut p = Prefs::default();
    let ret = p.set_nb_workers(7);
    assert_eq!(ret, p.nb_workers);
}

// ─────────────────────────────────────────────────────────────────────────────
// set_dictionary_filename  (lz4io.c lines 236–241)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn set_dictionary_filename_sets_use_dictionary_true() {
    let mut p = Prefs::default();
    assert!(p.set_dictionary_filename(Some("my.dict")));
    assert!(p.use_dictionary);
    assert_eq!(p.dictionary_filename.as_deref(), Some("my.dict"));
}

#[test]
fn set_dictionary_filename_clears_when_none() {
    let mut p = Prefs::default();
    p.set_dictionary_filename(Some("x.dict"));
    assert!(!p.set_dictionary_filename(None));
    assert!(!p.use_dictionary);
    assert!(p.dictionary_filename.is_none());
}

#[test]
fn set_dictionary_filename_returns_bool_indicating_active() {
    let mut p = Prefs::default();
    let with = p.set_dictionary_filename(Some("d"));
    let without = p.set_dictionary_filename(None);
    assert!(with);
    assert!(!without);
}

// ─────────────────────────────────────────────────────────────────────────────
// set_pass_through  (lz4io.c lines 244–248)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn set_pass_through_enables() {
    let mut p = Prefs::default();
    assert!(p.set_pass_through(true));
    assert!(p.pass_through);
}

#[test]
fn set_pass_through_disables() {
    let mut p = Prefs::default();
    p.set_pass_through(true);
    assert!(!p.set_pass_through(false));
    assert!(!p.pass_through);
}

// ─────────────────────────────────────────────────────────────────────────────
// set_overwrite  (lz4io.c lines 251–255)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn set_overwrite_disables() {
    let mut p = Prefs::default();
    assert!(!p.set_overwrite(false));
    assert!(!p.overwrite);
}

#[test]
fn set_overwrite_re_enables() {
    let mut p = Prefs::default();
    p.set_overwrite(false);
    assert!(p.set_overwrite(true));
    assert!(p.overwrite);
}

// ─────────────────────────────────────────────────────────────────────────────
// set_test_mode  (lz4io.c lines 258–262)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn set_test_mode_enables() {
    let mut p = Prefs::default();
    assert!(p.set_test_mode(true));
    assert!(p.test_mode);
}

#[test]
fn set_test_mode_disables() {
    let mut p = Prefs::default();
    p.set_test_mode(true);
    assert!(!p.set_test_mode(false));
    assert!(!p.test_mode);
}

// ─────────────────────────────────────────────────────────────────────────────
// set_block_size_id  (lz4io.c lines 265–274)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn set_block_size_id_4_returns_64kb() {
    let mut p = Prefs::default();
    assert_eq!(p.set_block_size_id(4), 64 * KB);
    assert_eq!(p.block_size, 64 * KB);
    assert_eq!(p.block_size_id, 4);
}

#[test]
fn set_block_size_id_5_returns_256kb() {
    let mut p = Prefs::default();
    assert_eq!(p.set_block_size_id(5), 256 * KB);
    assert_eq!(p.block_size_id, 5);
}

#[test]
fn set_block_size_id_6_returns_1mb() {
    let mut p = Prefs::default();
    assert_eq!(p.set_block_size_id(6), MB);
    assert_eq!(p.block_size_id, 6);
}

#[test]
fn set_block_size_id_7_returns_4mb() {
    let mut p = Prefs::default();
    assert_eq!(p.set_block_size_id(7), 4 * MB);
    assert_eq!(p.block_size_id, 7);
}

#[test]
fn set_block_size_id_3_is_out_of_range_returns_0() {
    let mut p = Prefs::default();
    assert_eq!(p.set_block_size_id(3), 0);
}

#[test]
fn set_block_size_id_8_is_out_of_range_returns_0() {
    let mut p = Prefs::default();
    assert_eq!(p.set_block_size_id(8), 0);
}

#[test]
fn set_block_size_id_0_is_out_of_range_returns_0() {
    let mut p = Prefs::default();
    assert_eq!(p.set_block_size_id(0), 0);
}

// ─────────────────────────────────────────────────────────────────────────────
// set_block_size  (lz4io.c lines 276–291)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn set_block_size_below_min_clamps_to_32() {
    let mut p = Prefs::default();
    assert_eq!(p.set_block_size(0), 32);
    assert_eq!(p.block_size, 32);
}

#[test]
fn set_block_size_above_max_clamps_to_4mb() {
    let mut p = Prefs::default();
    assert_eq!(p.set_block_size(usize::MAX), 4 * MB);
    assert_eq!(p.block_size, 4 * MB);
}

#[test]
fn set_block_size_exactly_at_min() {
    let mut p = Prefs::default();
    assert_eq!(p.set_block_size(32), 32);
}

#[test]
fn set_block_size_exactly_at_max() {
    let mut p = Prefs::default();
    assert_eq!(p.set_block_size(4 * MB), 4 * MB);
}

#[test]
fn set_block_size_stores_block_size() {
    let mut p = Prefs::default();
    p.set_block_size(MB);
    assert_eq!(p.block_size, MB);
}

#[test]
fn set_block_size_derives_block_size_id() {
    // After setting a valid power-of-two block size, block_size_id should be set.
    let mut p = Prefs::default();
    p.set_block_size(4 * MB); // max block → ID must be ≥ 4
    assert!(p.block_size_id >= 4);
}

// ─────────────────────────────────────────────────────────────────────────────
// set_block_mode  (lz4io.c lines 294–298)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn set_block_mode_independent_sets_independence_true() {
    let mut p = Prefs::default();
    assert!(p.set_block_mode(BlockMode::Independent));
    assert!(p.block_independence);
}

#[test]
fn set_block_mode_linked_sets_independence_false() {
    let mut p = Prefs::default();
    assert!(!p.set_block_mode(BlockMode::Linked));
    assert!(!p.block_independence);
}

// ─────────────────────────────────────────────────────────────────────────────
// set_block_checksum_mode  (lz4io.c lines 301–305)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn set_block_checksum_mode_enables() {
    let mut p = Prefs::default();
    assert!(p.set_block_checksum_mode(true));
    assert!(p.block_checksum);
}

#[test]
fn set_block_checksum_mode_disables() {
    let mut p = Prefs::default();
    p.set_block_checksum_mode(true);
    assert!(!p.set_block_checksum_mode(false));
    assert!(!p.block_checksum);
}

// ─────────────────────────────────────────────────────────────────────────────
// set_stream_checksum_mode  (lz4io.c lines 308–312)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn set_stream_checksum_mode_disables() {
    let mut p = Prefs::default();
    assert!(!p.set_stream_checksum_mode(false));
    assert!(!p.stream_checksum);
}

#[test]
fn set_stream_checksum_mode_re_enables() {
    let mut p = Prefs::default();
    p.set_stream_checksum_mode(false);
    assert!(p.set_stream_checksum_mode(true));
    assert!(p.stream_checksum);
}

// ─────────────────────────────────────────────────────────────────────────────
// set_sparse_file  (lz4io.c lines 322–326)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn set_sparse_file_true_returns_2() {
    let mut p = Prefs::default();
    assert_eq!(p.set_sparse_file(true), 2);
    assert_eq!(p.sparse_file_support, 2);
}

#[test]
fn set_sparse_file_false_returns_0() {
    let mut p = Prefs::default();
    assert_eq!(p.set_sparse_file(false), 0);
    assert_eq!(p.sparse_file_support, 0);
}

// ─────────────────────────────────────────────────────────────────────────────
// set_content_size  (lz4io.c lines 329–333)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn set_content_size_enables() {
    let mut p = Prefs::default();
    assert!(p.set_content_size(true));
    assert!(p.content_size_flag);
}

#[test]
fn set_content_size_disables() {
    let mut p = Prefs::default();
    p.set_content_size(true);
    assert!(!p.set_content_size(false));
    assert!(!p.content_size_flag);
}

// ─────────────────────────────────────────────────────────────────────────────
// favor_dec_speed  (lz4io.c lines 336–339)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn favor_dec_speed_enables() {
    let mut p = Prefs::default();
    p.favor_dec_speed(true);
    assert!(p.favor_dec_speed);
}

#[test]
fn favor_dec_speed_disables() {
    let mut p = Prefs::default();
    p.favor_dec_speed(true);
    p.favor_dec_speed(false);
    assert!(!p.favor_dec_speed);
}

// ─────────────────────────────────────────────────────────────────────────────
// set_remove_src_file  (lz4io.c lines 341–344)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn set_remove_src_file_enables() {
    let mut p = Prefs::default();
    p.set_remove_src_file(true);
    assert!(p.remove_src_file);
}

#[test]
fn set_remove_src_file_disables() {
    let mut p = Prefs::default();
    p.set_remove_src_file(true);
    p.set_remove_src_file(false);
    assert!(!p.remove_src_file);
}

// ─────────────────────────────────────────────────────────────────────────────
// cpu_load_sec  (lz4io.c lines 112–124)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn cpu_load_sec_returns_non_negative() {
    // Passing 0 as the start time; result must be non-negative.
    let load = cpu_load_sec(0);
    assert!(load >= 0.0, "cpu_load_sec must be non-negative; got {load}");
}

#[test]
fn cpu_load_sec_reasonable_upper_bound() {
    // For a short-lived test, CPU time since clock_t=0 should be well under
    // 10 seconds (POSIX: 0 is effectively program start or very small).
    let load = cpu_load_sec(0);
    assert!(load < 600.0, "cpu_load_sec unreasonably large: {load}");
}

// ─────────────────────────────────────────────────────────────────────────────
// Prefs clone  (Rust ownership parity)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn prefs_clone_is_independent() {
    let mut p = Prefs::default();
    let q = p.clone();
    p.set_overwrite(false);
    // Cloned copy must not be affected.
    assert!(q.overwrite);
}

#[test]
fn prefs_debug_does_not_panic() {
    let p = Prefs::default();
    let _ = format!("{:?}", p);
}
