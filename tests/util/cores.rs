// Integration tests for task-008: util/cores.rs — CPU core count
//
// Tests verify behavioural parity with UTIL_countCores() from
// lz4-1.10.0/programs/util.c:
//   - Returns at least 1 (matches C default fallback of 1)
//   - Returns a reasonable upper-bound value
//   - Is deterministic across multiple calls
//   - Returns a positive (non-zero) value

use lz4::util::cores::count_cores;

// ─────────────────────────────────────────────────────────────────────────────
// Basic contract
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn count_cores_returns_at_least_one() {
    // C implementation falls back to 1 on all unknown/error platforms.
    // Rust implementation unwrap_or(1) preserves this guarantee.
    assert!(count_cores() >= 1, "count_cores() must be >= 1");
}

#[test]
fn count_cores_returns_reasonable_upper_bound() {
    // No real machine has more than 65536 logical cores; sanity check
    // that the function isn't returning garbage.
    let cores = count_cores();
    assert!(
        cores <= 65536,
        "count_cores() returned suspiciously large value: {cores}"
    );
}

#[test]
fn count_cores_is_deterministic() {
    // UTIL_countCores() caches the result after the first call; in Rust,
    // available_parallelism() is also stable across calls.
    let first = count_cores();
    let second = count_cores();
    assert_eq!(
        first, second,
        "count_cores() must return the same value on repeated calls"
    );
}

#[test]
fn count_cores_return_type_is_nonzero() {
    // The function returns usize; verifying it is non-zero matches the C
    // guarantee that UTIL_countCores() never returns 0.
    let cores = count_cores();
    assert_ne!(cores, 0, "count_cores() must never return 0");
}
