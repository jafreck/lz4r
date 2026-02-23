// Unit tests for task-010: TPool thread pool (threadpool.rs)
//
// Tests verify behavioural parity with lz4-1.10.0/programs/threadpool.c.
// The Rust implementation replaces pthread/IOCP with rayon + crossbeam_channel,
// but the public API semantics must match the C API.
//
// Coverage:
//   - TPool::new returns None for invalid parameters (nb_threads=0, queue_size=0)
//   - TPool::new returns Some for valid parameters
//   - submit_job executes the job function
//   - submit_job executes all jobs with multiple submissions
//   - jobs_completed blocks until all pending jobs finish
//   - jobs_completed returns immediately when no jobs are pending
//   - jobs_completed can be called multiple times (pool reusable after barrier)
//   - Drop waits for all in-flight jobs before returning
//   - Jobs run concurrently (parallel execution across threads)
//   - Closure captures are moved into the job correctly (void* replacement)

use lz4::threadpool::TPool;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};
use std::time::Duration;

// ─────────────────────────────────────────────────────────────────────────────
// TPool::new — parameter validation (mirrors TPool_create returning NULL)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn new_returns_none_for_zero_threads() {
    // C: TPool_create(0, 1) → NULL  (nbThreads must be >= 1)
    let pool = TPool::new(0, 1);
    assert!(pool.is_none(), "nb_threads=0 should return None");
}

#[test]
fn new_returns_none_for_zero_queue_size() {
    // C: TPool_create(1, 0) → NULL  (queueSize must be >= 1)
    let pool = TPool::new(1, 0);
    assert!(pool.is_none(), "queue_size=0 should return None");
}

#[test]
fn new_returns_none_for_both_zero() {
    let pool = TPool::new(0, 0);
    assert!(pool.is_none());
}

#[test]
fn new_returns_some_for_single_thread_single_queue() {
    // Minimum valid configuration: 1 thread, 1 queue slot
    let pool = TPool::new(1, 1);
    assert!(pool.is_some());
}

#[test]
fn new_returns_some_for_multiple_threads() {
    let pool = TPool::new(4, 8);
    assert!(pool.is_some());
}

// ─────────────────────────────────────────────────────────────────────────────
// submit_job — basic execution (mirrors TPool_submitJob calling fn(arg))
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn submit_job_executes_closure() {
    // A submitted job must be called exactly once.
    let pool = TPool::new(1, 4).expect("valid pool");
    let counter = Arc::new(AtomicUsize::new(0));

    let c = Arc::clone(&counter);
    pool.submit_job(Box::new(move || {
        c.fetch_add(1, Ordering::SeqCst);
    }));

    pool.jobs_completed();
    assert_eq!(counter.load(Ordering::SeqCst), 1);
}

#[test]
fn submit_multiple_jobs_all_execute() {
    // Each submitted job runs exactly once — no jobs are silently dropped.
    let pool = TPool::new(2, 8).expect("valid pool");
    let counter = Arc::new(AtomicUsize::new(0));
    const N: usize = 16;

    for _ in 0..N {
        let c = Arc::clone(&counter);
        pool.submit_job(Box::new(move || {
            c.fetch_add(1, Ordering::SeqCst);
        }));
    }

    pool.jobs_completed();
    assert_eq!(counter.load(Ordering::SeqCst), N);
}

#[test]
fn submit_job_captures_closure_environment() {
    // The Rust API replaces the C `void* arg` pattern with a closure that
    // captures its environment by value.  Verify the captured value is
    // accessible inside the job.
    let pool = TPool::new(1, 4).expect("valid pool");
    let expected: u64 = 0xDEAD_BEEF_CAFE_1234;
    let result = Arc::new(std::sync::Mutex::new(0u64));

    let r = Arc::clone(&result);
    pool.submit_job(Box::new(move || {
        *r.lock().unwrap() = expected;
    }));

    pool.jobs_completed();
    assert_eq!(*result.lock().unwrap(), expected);
}

// ─────────────────────────────────────────────────────────────────────────────
// jobs_completed — barrier semantics (mirrors TPool_jobsCompleted)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn jobs_completed_returns_immediately_when_no_jobs() {
    // C: TPool_jobsCompleted on an idle pool returns immediately.
    let pool = TPool::new(2, 4).expect("valid pool");
    // Should not block; wrap in a tight timeout via thread to detect hangs.
    let done = Arc::new(AtomicUsize::new(0));
    let d = Arc::clone(&done);
    let handle = std::thread::spawn(move || {
        pool.jobs_completed();
        d.store(1, Ordering::SeqCst);
    });
    handle.join().expect("jobs_completed should not deadlock on idle pool");
    assert_eq!(done.load(Ordering::SeqCst), 1);
}

#[test]
fn jobs_completed_waits_for_slow_job() {
    // jobs_completed must block until the submitted (slow) job finishes.
    let pool = TPool::new(1, 4).expect("valid pool");
    let flag = Arc::new(AtomicUsize::new(0));

    let f = Arc::clone(&flag);
    pool.submit_job(Box::new(move || {
        std::thread::sleep(Duration::from_millis(50));
        f.store(1, Ordering::SeqCst);
    }));

    // At this point the job may not be done yet.
    pool.jobs_completed();

    // After jobs_completed returns the job must have finished.
    assert_eq!(flag.load(Ordering::SeqCst), 1);
}

#[test]
fn jobs_completed_is_reusable() {
    // C: TPool_jobsCompleted does not shut down the pool; more jobs can be
    // submitted afterwards.
    let pool = TPool::new(2, 4).expect("valid pool");
    let counter = Arc::new(AtomicUsize::new(0));

    // First batch
    for _ in 0..4 {
        let c = Arc::clone(&counter);
        pool.submit_job(Box::new(move || {
            c.fetch_add(1, Ordering::SeqCst);
        }));
    }
    pool.jobs_completed();
    assert_eq!(counter.load(Ordering::SeqCst), 4);

    // Second batch — pool must still accept new work after jobs_completed
    for _ in 0..4 {
        let c = Arc::clone(&counter);
        pool.submit_job(Box::new(move || {
            c.fetch_add(1, Ordering::SeqCst);
        }));
    }
    pool.jobs_completed();
    assert_eq!(counter.load(Ordering::SeqCst), 8);
}

// ─────────────────────────────────────────────────────────────────────────────
// Drop — TPool_free waits for running jobs before destroying the pool
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn drop_waits_for_in_flight_jobs() {
    // When TPool is dropped, all submitted jobs must have completed.
    let flag = Arc::new(AtomicUsize::new(0));

    {
        let pool = TPool::new(1, 4).expect("valid pool");
        let f = Arc::clone(&flag);
        pool.submit_job(Box::new(move || {
            std::thread::sleep(Duration::from_millis(50));
            f.store(1, Ordering::SeqCst);
        }));
        // pool drops here — must wait for the job
    }

    // After drop, the job must have finished
    assert_eq!(flag.load(Ordering::SeqCst), 1);
}

// ─────────────────────────────────────────────────────────────────────────────
// Parallelism — jobs run concurrently across worker threads
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn jobs_run_concurrently_across_threads() {
    // Submit N jobs that all rendezvous at a barrier; if the pool runs them
    // serially this would deadlock (barrier.wait blocks forever).
    // This validates that the rayon-backed pool actually uses multiple threads.
    const N: usize = 4;
    let pool = TPool::new(N, N).expect("valid pool");
    let barrier = Arc::new(Barrier::new(N));
    let counter = Arc::new(AtomicUsize::new(0));

    for _ in 0..N {
        let b = Arc::clone(&barrier);
        let c = Arc::clone(&counter);
        pool.submit_job(Box::new(move || {
            b.wait(); // would deadlock if fewer than N threads exist
            c.fetch_add(1, Ordering::SeqCst);
        }));
    }

    pool.jobs_completed();
    assert_eq!(counter.load(Ordering::SeqCst), N);
}

// ─────────────────────────────────────────────────────────────────────────────
// Bounded-queue / back-pressure — many jobs with a small pool
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn submit_many_jobs_small_pool() {
    // Stress test: more jobs than threads + queue_size, verifying back-pressure
    // (submit_job blocks as needed) and correct final count.
    let pool = TPool::new(2, 2).expect("valid pool");
    let counter = Arc::new(AtomicUsize::new(0));
    const N: usize = 50;

    for _ in 0..N {
        let c = Arc::clone(&counter);
        pool.submit_job(Box::new(move || {
            c.fetch_add(1, Ordering::SeqCst);
        }));
    }

    pool.jobs_completed();
    assert_eq!(counter.load(Ordering::SeqCst), N);
}
