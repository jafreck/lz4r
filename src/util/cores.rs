/// Returns the number of logical CPU cores available on the system.
///
/// Migrated from `UTIL_countCores()` in `util.c`. The original implementation
/// used platform-specific APIs (_WIN32: GetSystemInfo, __APPLE__: sysctlbyname,
/// __linux__: sysconf, FreeBSD: sysctlbyname kern.smp.*). Rust's
/// `std::thread::available_parallelism` provides a portable equivalent.
///
/// Guaranteed to return a value ≥ 1 (falls back to 1 on error, matching
/// the C implementation's default behavior).
pub fn count_cores() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_count_cores_at_least_one() {
        assert!(count_cores() >= 1);
    }
}
