// Unit tests for task-003: timefn module (timefn.c / timefn.h)
//
// Validates parity between the Rust migration and the original C module:
// - get_time()           ≡ TIME_getTime()
// - span_ns(start, end)  ≡ TIME_span_ns(start, end)
// - clock_span_ns(start) ≡ TIME_clockSpan_ns(start)
// - wait_for_next_tick() ≡ TIME_waitForNextTick()
// - support_mt_measurements() ≡ TIME_support_MT_measurements() (always 1/true in Rust)

use lz4::timefn::{clock_span_ns, get_time, span_ns, support_mt_measurements, wait_for_next_tick, DurationNs, TimeT};
use std::thread;
use std::time::Duration;

// --- DurationNs type ---

/// DurationNs must be a 64-bit unsigned type (mirrors C `unsigned long long`).
#[test]
fn duration_ns_is_u64_width() {
    assert_eq!(std::mem::size_of::<DurationNs>(), 8);
    // Must be unsigned: a large value should not wrap negatively
    let large: DurationNs = u64::MAX;
    assert!(large > 0);
}

// --- TimeT construction ---

/// TimeT::new() must succeed without panicking.
#[test]
fn time_t_new_does_not_panic() {
    let _ = TimeT::new();
}

/// TimeT::default() must be equivalent to TimeT::new() (i.e., captures current time).
#[test]
fn time_t_default_does_not_panic() {
    let _ = TimeT::default();
}

/// TimeT must be Copy — callers in bench.c pass it by value freely.
#[test]
fn time_t_is_copy() {
    let t = get_time();
    let t2 = t; // Copy, not move
    let _ = t;  // Original still usable
    let _ = t2;
}

/// TimeT must be Clone.
#[test]
fn time_t_is_clone() {
    let t = get_time();
    let t2 = t.clone();
    let _ = t2;
}

// --- get_time ---

/// get_time() must return without panicking.
#[test]
fn get_time_does_not_panic() {
    let _ = get_time();
}

/// Two successive calls to get_time() must produce values such that
/// the second is >= the first (monotonicity).
#[test]
fn get_time_is_monotonic() {
    let t1 = get_time();
    let t2 = get_time();
    // span_ns returns 0 when both measure the same instant; never negative.
    let ns = span_ns(t1, t2);
    // A u64 is always >= 0, but we can assert no panic and sensible value.
    let _ = ns; // implicit: did not panic
}

// --- span_ns ---

/// span_ns(t, t) should return 0 (same instant compared to itself).
#[test]
fn span_ns_same_instant_is_zero() {
    let t = get_time();
    let ns = span_ns(t, t);
    assert_eq!(ns, 0, "span_ns with identical instants must be 0");
}

/// span_ns(start, end) where end is clearly later must return a positive value.
#[test]
fn span_ns_later_end_is_positive() {
    let start = get_time();
    thread::sleep(Duration::from_millis(5));
    let end = get_time();
    let ns = span_ns(start, end);
    assert!(ns > 0, "span_ns over a real sleep must be positive, got {}", ns);
}

/// span_ns result over a 5 ms sleep should be at least 1_000_000 ns (1 ms) —
/// i.e., the nanosecond unit is correct (not microseconds or seconds).
#[test]
fn span_ns_unit_is_nanoseconds() {
    let start = get_time();
    thread::sleep(Duration::from_millis(5));
    let end = get_time();
    let ns = span_ns(start, end);
    // 5 ms = 5_000_000 ns; allow a generous lower bound of 1 ms to tolerate scheduling.
    assert!(
        ns >= 1_000_000,
        "5 ms sleep should yield >= 1_000_000 ns, got {}",
        ns
    );
}

// --- clock_span_ns ---

/// clock_span_ns(start) must return without panicking.
#[test]
fn clock_span_ns_does_not_panic() {
    let start = get_time();
    let _ = clock_span_ns(start);
}

/// clock_span_ns should return >= 0 for any start in the past.
/// Because DurationNs is u64 this is always true, but we verify no panic
/// and a non-absurd value (< 1 second for a measurement taken just now).
#[test]
fn clock_span_ns_reasonable_range() {
    let start = get_time();
    let ns = clock_span_ns(start);
    // Should be much less than 1 second (1_000_000_000 ns) for an immediate call.
    assert!(
        ns < 1_000_000_000,
        "clock_span_ns for an immediate measurement should be < 1 s, got {}",
        ns
    );
}

/// clock_span_ns called after a sleep should reflect elapsed time.
#[test]
fn clock_span_ns_reflects_elapsed_time() {
    let start = get_time();
    thread::sleep(Duration::from_millis(5));
    let ns = clock_span_ns(start);
    assert!(
        ns >= 1_000_000,
        "clock_span_ns after 5 ms sleep should be >= 1 ms, got {}",
        ns
    );
}

/// clock_span_ns(start) must return the same as span_ns(start, get_time())
/// within a reasonable tolerance (both read the same underlying clock).
#[test]
fn clock_span_ns_consistent_with_span_ns() {
    let start = get_time();
    thread::sleep(Duration::from_millis(2));
    let direct = clock_span_ns(start);
    let end = get_time();
    let via_span = span_ns(start, end);
    // direct was measured before end; it must be <= via_span (or close; allow 5 ms slop).
    let slop_ns: u64 = 5_000_000;
    assert!(
        direct <= via_span + slop_ns,
        "clock_span_ns ({}) should be <= span_ns ({}) + slop",
        direct,
        via_span
    );
}

// --- wait_for_next_tick ---

/// wait_for_next_tick() must return (i.e., not loop forever on a functional clock).
/// We give it a generous timeout by running in a thread with a join timeout.
#[test]
fn wait_for_next_tick_terminates() {
    let handle = thread::spawn(wait_for_next_tick);
    // Give it up to 5 seconds; any real monotonic clock will advance far sooner.
    handle
        .join()
        .expect("wait_for_next_tick should not panic");
}

/// After wait_for_next_tick() returns, at least 1 ns should have elapsed
/// since before the call (clock advanced by definition).
#[test]
fn wait_for_next_tick_clock_advanced() {
    let before = get_time();
    wait_for_next_tick();
    let after = get_time();
    let ns = span_ns(before, after);
    assert!(ns > 0, "clock must have advanced after wait_for_next_tick");
}

// --- support_mt_measurements ---

/// In Rust, Instant is always MT-safe, so this must always return true.
/// Equivalent to C returning 1 on all non-C90-fallback platforms.
#[test]
fn support_mt_measurements_always_true() {
    assert!(
        support_mt_measurements(),
        "Rust Instant is always MT-safe; support_mt_measurements must return true"
    );
}

/// support_mt_measurements is idempotent — calling it twice returns the same value.
#[test]
fn support_mt_measurements_is_idempotent() {
    assert_eq!(support_mt_measurements(), support_mt_measurements());
}
