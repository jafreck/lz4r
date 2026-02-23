//! Fixed-size work-stealing thread pool.
//!
//! Provides the same API as the `TPool` family of functions from the LZ4
//! reference implementation (`threadpool.c`), backed by `rayon::ThreadPool`
//! rather than pthreads / Windows IOCP.  Bounded-queue / blocking-submit
//! semantics are preserved via a `crossbeam_channel::bounded` semaphore channel.
//!

use crossbeam_channel::{bounded, Receiver, Sender};
use rayon::ThreadPool as RayonPool;
use std::sync::{Arc, Condvar, Mutex};

// ---------------------------------------------------------------------------
// Job type — mirrors `TPool_job` from the C source.
// ---------------------------------------------------------------------------
type JobFn = Box<dyn FnOnce() + Send + 'static>;

// ---------------------------------------------------------------------------
// Internal shared state that workers and submitters both access.
// ---------------------------------------------------------------------------
struct PoolState {
    pending: usize, // number of submitted-but-not-yet-finished jobs
}

/// Thread pool handle — equivalent to `TPool*` in the C API.
///
/// `TPool_create` → `TPool::new`
/// `TPool_free`   → `Drop for TPool`  (joins workers automatically)
/// `TPool_submitJob` → `TPool::submit_job`
/// `TPool_jobsCompleted` → `TPool::jobs_completed`
pub struct TPool {
    /// rayon thread pool that executes jobs.
    pool: Arc<RayonPool>,
    /// Bounded channel used as a semaphore: the sender slot limits how many
    /// jobs can be in-flight simultaneously (queue_size + nb_threads slots).
    /// Submitters acquire a slot before posting; workers release it on finish.
    slot_tx: Sender<()>,
    slot_rx: Receiver<()>,
    /// Shared counter of pending jobs plus a condvar for `jobs_completed`.
    state: Arc<(Mutex<PoolState>, Condvar)>,
}

impl TPool {
    /// `TPool_create(nbThreads, queueSize)` — returns `None` on failure.
    ///
    /// *nbThreads* must be ≥ 1, *queueSize* must be ≥ 1.
    /// The C code allocates one extra queue slot to distinguish full vs. empty;
    /// here `crossbeam_channel::bounded(queue_size + nb_threads)` plays the
    /// same role as the semaphore initialised to `queueSize + nbWorkers` in the
    /// Windows implementation.
    pub fn new(nb_threads: usize, queue_size: usize) -> Option<Self> {
        if nb_threads < 1 || queue_size < 1 {
            return None;
        }
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(nb_threads)
            .build()
            .ok()?;

        // Total slots = queue_size + nb_threads mirrors the Windows semaphore.
        let capacity = queue_size + nb_threads;
        let (slot_tx, slot_rx) = bounded(capacity);
        // Pre-fill the channel so that `slot_rx.recv()` acts as "wait for a
        // free slot" (i.e. we send tokens to represent free slots).
        for _ in 0..capacity {
            slot_tx.send(()).ok()?;
        }

        let state = Arc::new((Mutex::new(PoolState { pending: 0 }), Condvar::new()));

        Some(TPool {
            pool: Arc::new(pool),
            slot_tx,
            slot_rx,
            state,
        })
    }

    /// `TPool_submitJob(ctx, job_function, arg)` — may block if queue is full.
    ///
    /// In C the caller passes a raw `void (*fn)(void*)` + `void* arg`.
    /// In Rust the equivalent is a `Box<dyn FnOnce() + Send>` closure that
    /// has already captured its argument, eliminating the `void*` anti-pattern.
    pub fn submit_job(&self, job: JobFn) {
        // Block until a slot is available (mirrors `WaitForSingleObject` on the
        // semaphore in the Windows path, or `pthread_cond_wait` in POSIX path).
        self.slot_rx.recv().expect("threadpool slot channel closed");

        // Increment pending count before spawning so `jobs_completed` cannot
        // observe zero between submit and actual execution start.
        {
            let (lock, _cvar) = &*self.state;
            let mut s = lock.lock().unwrap();
            s.pending += 1;
        }

        let state = Arc::clone(&self.state);
        let slot_tx = self.slot_tx.clone();
        self.pool.spawn(move || {
            job();

            // Release the slot and decrement pending count.
            let (lock, cvar) = &*state;
            let mut s = lock.lock().unwrap();
            s.pending -= 1;
            if s.pending == 0 {
                cvar.notify_all();
            }
            // Return the semaphore token.
            let _ = slot_tx.send(());
        });
    }

    /// `TPool_jobsCompleted(ctx)` — blocks until all submitted jobs have finished.
    ///
    /// Does NOT shut down the pool; it can accept further jobs afterwards,
    /// identical to the C semantics.
    pub fn jobs_completed(&self) {
        let (lock, cvar) = &*self.state;
        let mut s = lock.lock().unwrap();
        while s.pending > 0 {
            s = cvar.wait(s).unwrap();
        }
    }
}

impl Drop for TPool {
    /// `TPool_free` — waits for all running jobs to finish then tears down the
    /// rayon pool.  rayon's `ThreadPool` already joins workers on drop, so we
    /// only need to ensure no jobs are still in-flight first.
    fn drop(&mut self) {
        self.jobs_completed();
        // rayon::ThreadPool::drop joins all worker threads automatically.
    }
}
