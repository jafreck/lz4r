// timefn - portable high-resolution monotonic timer abstraction
// Migrated from timefn.c / timefn.h (lz4 1.10.0)
//
// Rust's std::time::Instant is monotonic and MT-safe on all supported platforms,
// replacing the platform-specific C implementations (QueryPerformanceCounter,
// mach_absolute_time, clock_gettime, timespec_get, clock()).

use std::time::Instant;

/// Nanosecond duration type (equivalent to C `Duration_ns` / `unsigned long long`).
pub type DurationNs = u64;

/// Opaque timestamp container. The absolute value is not meaningful;
/// use it only to compute a duration between two measurements.
/// Equivalent to C `TIME_t`.
#[derive(Clone, Copy)]
pub struct TimeT {
    pub(crate) t: Instant,
}

impl TimeT {
    /// Equivalent to `TIME_INITIALIZER { 0 }` — returns a timestamp from now.
    pub fn new() -> Self {
        TimeT { t: Instant::now() }
    }
}

impl Default for TimeT {
    fn default() -> Self {
        TimeT::new()
    }
}

/// Returns current monotonic timestamp.
/// Equivalent to `TIME_t TIME_getTime(void)`.
pub fn get_time() -> TimeT {
    TimeT { t: Instant::now() }
}

/// Returns the nanosecond duration between `clock_start` and `clock_end`.
/// Equivalent to `Duration_ns TIME_span_ns(TIME_t clockStart, TIME_t clockEnd)`.
pub fn span_ns(clock_start: TimeT, clock_end: TimeT) -> DurationNs {
    clock_end
        .t
        .duration_since(clock_start.t)
        .as_nanos() as DurationNs
}

/// Measures nanoseconds elapsed since `clock_start` (captures current time internally).
/// Equivalent to `Duration_ns TIME_clockSpan_ns(TIME_t clockStart)`.
pub fn clock_span_ns(clock_start: TimeT) -> DurationNs {
    clock_start.t.elapsed().as_nanos() as DurationNs
}

/// Busy-waits until the clock advances by at least 1 ns.
/// Used before benchmark loops to synchronize with a clock tick.
/// Equivalent to `void TIME_waitForNextTick(void)`.
pub fn wait_for_next_tick() {
    let clock_start = get_time();
    loop {
        if span_ns(clock_start, get_time()) > 0 {
            break;
        }
    }
}

/// Returns `true` if `get_time()` is safe to use across threads.
/// Rust's `Instant` is always MT-safe, so this always returns `true`.
/// Equivalent to `int TIME_support_MT_measurements(void)` returning 1.
pub fn support_mt_measurements() -> bool {
    true
}
