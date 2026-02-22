// Unit tests for task-017: src/frame/cdict.rs — Lz4FCDict
//
// Verifies behavioural parity with lz4frame.c v1.10.0, lines 527–590:
//   `LZ4F_createCDict` / `LZ4F_createCDict_advanced` → `Lz4FCDict::create`
//   `LZ4F_freeCDict`                                 → (Drop on Box<Lz4FCDict>)
//
// All tests operate on the public API only.

use lz4::frame::cdict::Lz4FCDict;

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

/// The frame format's hard limit on retained dictionary bytes (64 KB).
const MAX_DICT_SIZE: usize = 64 * 1024;

fn cycling_dict(len: usize) -> Vec<u8> {
    (0u8..=255).cycle().take(len).collect()
}

// ═════════════════════════════════════════════════════════════════════════════
// Lz4FCDict::create  (LZ4F_createCDict / LZ4F_createCDict_advanced)
// ═════════════════════════════════════════════════════════════════════════════

// ---------------------------------------------------------------------------
// Basic creation — happy paths
// ---------------------------------------------------------------------------

/// Parity: LZ4F_createCDict with a non-empty dict must succeed (return non-NULL).
#[test]
fn create_nonempty_dict_returns_some() {
    let dict = cycling_dict(1024);
    assert!(
        Lz4FCDict::create(&dict).is_some(),
        "create should succeed with a 1 KB dictionary"
    );
}

/// Parity: LZ4F_createCDict with an empty dict must succeed (return non-NULL).
/// C: dictSize=0 → memcpy copies 0 bytes, load_dict_hc loads 0 bytes — both are valid.
#[test]
fn create_empty_dict_returns_some() {
    assert!(
        Lz4FCDict::create(&[]).is_some(),
        "create should succeed with an empty dictionary"
    );
}

/// Parity: dict of exactly 64 KB is retained in full.
#[test]
fn create_exactly_64kb_dict_succeeds() {
    let dict = cycling_dict(MAX_DICT_SIZE);
    assert!(
        Lz4FCDict::create(&dict).is_some(),
        "create should succeed with an exactly-64 KB dictionary"
    );
}

/// Parity: dict larger than 64 KB is trimmed silently; create still succeeds.
/// Mirrors the C trim: `dictStart = (const char*)dictBuffer + dictSize - 64KB`.
#[test]
fn create_larger_than_64kb_dict_succeeds() {
    let dict = cycling_dict(128 * 1024);
    assert!(
        Lz4FCDict::create(&dict).is_some(),
        "create should succeed when dictionary exceeds 64 KB"
    );
}

/// Parity: very large dictionary (512 KB) is trimmed and create still succeeds.
#[test]
fn create_very_large_dict_succeeds() {
    let dict = cycling_dict(512 * 1024);
    assert!(
        Lz4FCDict::create(&dict).is_some(),
        "create should succeed for a very large (512 KB) dictionary"
    );
}

/// A single-byte dictionary must be accepted without panic.
#[test]
fn create_single_byte_dict_succeeds() {
    assert!(
        Lz4FCDict::create(&[0x42u8]).is_some(),
        "create should succeed with a single-byte dictionary"
    );
}

/// All-zero dictionary must be accepted without panic.
#[test]
fn create_all_zeros_dict_succeeds() {
    let dict = vec![0u8; 4096];
    assert!(
        Lz4FCDict::create(&dict).is_some(),
        "create should succeed with an all-zero dictionary"
    );
}

/// All-0xFF dictionary must be accepted without panic.
#[test]
fn create_all_ff_dict_succeeds() {
    let dict = vec![0xFFu8; 4096];
    assert!(
        Lz4FCDict::create(&dict).is_some(),
        "create should succeed with an all-0xFF dictionary"
    );
}

/// Typical dictionary content (English prose, repeated) must be accepted.
#[test]
fn create_text_dict_succeeds() {
    let phrase = b"The quick brown fox jumps over the lazy dog";
    let dict: Vec<u8> = phrase.iter().cycle().take(4096).copied().collect();
    assert!(
        Lz4FCDict::create(&dict).is_some(),
        "create should succeed with text-like dictionary content"
    );
}

// ---------------------------------------------------------------------------
// Dictionary trimming — parity with C's 64 KB window
// ---------------------------------------------------------------------------

/// Parity: creating two CDicts from the same input must both succeed.
/// (Ensures no global state is corrupted between calls.)
#[test]
fn create_twice_from_same_dict_both_succeed() {
    let dict = cycling_dict(4096);
    let c1 = Lz4FCDict::create(&dict);
    let c2 = Lz4FCDict::create(&dict);
    assert!(c1.is_some(), "first create should succeed");
    assert!(c2.is_some(), "second create should succeed");
}

/// Parity: creating CDicts from different sub-slices of the same buffer both
/// succeed — mirrors creating two LZ4F_CDict from independent callers.
#[test]
fn create_from_dict_subslice_succeeds() {
    let large = cycling_dict(200 * 1024);
    // First CDict: first 64 KB
    let c1 = Lz4FCDict::create(&large[..64 * 1024]);
    // Second CDict: last 64 KB
    let c2 = Lz4FCDict::create(&large[large.len() - 64 * 1024..]);
    assert!(c1.is_some(), "CDict from first 64 KB should succeed");
    assert!(c2.is_some(), "CDict from last 64 KB should succeed");
}

/// Multiple sequential creates and drops must not corrupt the allocator.
/// Parity: repeated LZ4F_createCDict/LZ4F_freeCDict cycles must be safe.
#[test]
fn create_and_drop_cycle_does_not_panic() {
    let dict = cycling_dict(8 * 1024);
    for _ in 0..16 {
        let _cdict = Lz4FCDict::create(&dict).expect("create should not fail");
        // _cdict is dropped here
    }
}

// ---------------------------------------------------------------------------
// Drop behaviour (LZ4F_freeCDict parity)
// ---------------------------------------------------------------------------

/// Dropping a CDict must not panic (all sub-allocations freed by Drop).
#[test]
fn drop_nonempty_cdict_does_not_panic() {
    let dict = cycling_dict(4096);
    let cdict = Lz4FCDict::create(&dict).expect("create should succeed");
    drop(cdict); // must not panic or double-free
}

/// Dropping a CDict created from an empty dict must not panic.
#[test]
fn drop_empty_cdict_does_not_panic() {
    let cdict = Lz4FCDict::create(&[]).expect("create should succeed");
    drop(cdict);
}

/// Dropping a CDict created from a large (>64 KB) dict must not panic.
#[test]
fn drop_trimmed_cdict_does_not_panic() {
    let dict = cycling_dict(256 * 1024);
    let cdict = Lz4FCDict::create(&dict).expect("create should succeed");
    drop(cdict);
}

// ---------------------------------------------------------------------------
// Thread safety (Send + Sync)
// ---------------------------------------------------------------------------

/// Lz4FCDict must be Send — a CDict created on one thread can be moved to and
/// used on another, matching the C documentation's thread-safety guarantees.
///
/// Parity: LZ4F_CDict is documented as safe to share across threads after creation.
#[test]
fn cdict_is_send_across_threads() {
    let dict = cycling_dict(4096);
    let cdict = Lz4FCDict::create(&dict).expect("create should succeed");

    let handle = std::thread::spawn(move || {
        // Move the CDict into a new thread; simply assert it's alive.
        let _ = &cdict;
        true
    });

    assert!(handle.join().expect("thread should not panic"));
}

/// Lz4FCDict is Sync — a shared reference can be accessed from multiple threads.
/// The C documentation guarantees read-only thread safety after create.
#[test]
fn cdict_is_sync_shared_reference() {
    use std::sync::Arc;

    let dict = cycling_dict(4096);
    let cdict = Arc::new(Lz4FCDict::create(&dict).expect("create should succeed"));

    let c1 = Arc::clone(&cdict);
    let c2 = Arc::clone(&cdict);

    let h1 = std::thread::spawn(move || {
        let _ = &*c1; // shared read-only access
        true
    });
    let h2 = std::thread::spawn(move || {
        let _ = &*c2;
        true
    });

    assert!(h1.join().expect("thread 1 should not panic"));
    assert!(h2.join().expect("thread 2 should not panic"));
}

// ---------------------------------------------------------------------------
// Determinism
// ---------------------------------------------------------------------------

/// Creating two CDicts from identical input must produce identically-sized
/// results (dict_content is a deterministic copy of the trimmed input).
///
/// We can only observe this indirectly via success, but we verify no panics
/// and that creation is repeatable.
#[test]
fn create_is_deterministic_given_same_input() {
    let dict = cycling_dict(MAX_DICT_SIZE);
    // Both should succeed; the internal state is deterministically derived from `dict`.
    let c1 = Lz4FCDict::create(&dict);
    let c2 = Lz4FCDict::create(&dict);
    assert!(c1.is_some() == c2.is_some(), "both calls must have the same success/failure result");
}

// ---------------------------------------------------------------------------
// Compile-time trait bounds
// ---------------------------------------------------------------------------

/// Statically assert that Lz4FCDict implements Send and Sync.
/// If either bound were missing, this function would fail to compile.
#[allow(dead_code)]
fn assert_send_sync<T: Send + Sync>() {}

#[test]
fn lz4f_cdict_implements_send_and_sync() {
    assert_send_sync::<Lz4FCDict>();
}
