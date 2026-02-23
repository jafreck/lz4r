/// Returns the number of logical CPU cores available to the current process.
///
/// Delegates to [`std::thread::available_parallelism`], which honours OS-level
/// CPU affinity masks where supported. Returns at least `1`: if the query
/// fails the fallback prevents callers from creating zero-sized thread pools.
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
