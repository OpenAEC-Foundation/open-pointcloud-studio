//! The places to compute, the waiting room of large requests, the time
//! limit with the cancellation it brings, and the watchdog of work that does
//! not stop.
//!
//! A request whose body may be large takes a place in the waiting room
//! before its body is read; small requests (lists, the handshake, a ping)
//! never wait. A call then waits for one of the places to compute, and runs
//! on a blocking thread that keeps its place until it returns. At the time
//! limit the call is answered with 504 and its `keep_going` answers
//! `Cancelled` from then on; work that still runs a grace period after that
//! turns the health route to 503, so that the orchestrator replaces the task.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use pointcloud_core::ops::KeepGoing;
use pointcloud_core::LoadError;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::config::Config;
use crate::error::ApiError;

/// A request with a body of at most this many bytes needs no place in the
/// waiting room.
pub const SMALL_REQUEST: u64 = 64 * 1024;

/// A place in the waiting room. The work of the call takes it along, so
/// that it is given back when the work ends, also after a 504: the memory
/// of a body is held until then.
#[derive(Debug, Clone, Default)]
pub struct Place(Arc<Mutex<Option<OwnedSemaphorePermit>>>);

impl Place {
    pub fn new(permit: Option<OwnedSemaphorePermit>) -> Self {
        Self(Arc::new(Mutex::new(permit)))
    }

    /// The place, for the work that takes it along.
    pub fn take(&self) -> Option<OwnedSemaphorePermit> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).take()
    }
}

/// What one call shares with the thread it runs on.
#[derive(Debug, Default)]
struct CallState {
    cancelled: AtomicBool,
    ended: AtomicBool,
}

/// Marks a call ended when its thread is done, also when it panicked.
struct Ended(Arc<CallState>);

impl Drop for Ended {
    fn drop(&mut self) {
        self.0.ended.store(true, Ordering::Release);
    }
}

/// A call answered with 504 whose work may still run.
#[derive(Debug)]
struct Cancelled {
    state: Arc<CallState>,
    stuck_at: Instant,
}

#[derive(Debug)]
pub struct Limits {
    pub(crate) running: Arc<Semaphore>,
    pub(crate) waiting_room: Arc<Semaphore>,
    wait: Duration,
    time_limit: Duration,
    grace: Duration,
    cancelled: Mutex<Vec<Cancelled>>,
}

impl Limits {
    pub fn new(config: &Config) -> Self {
        Self {
            running: Arc::new(Semaphore::new(config.concurrent.max(1))),
            waiting_room: Arc::new(Semaphore::new(
                config.concurrent.max(1) + config.waiting_room,
            )),
            wait: config.wait,
            time_limit: config.time_limit,
            grace: config.grace,
            cancelled: Mutex::new(Vec::new()),
        }
    }

    /// A place in the waiting room for a request whose body is larger than
    /// `SMALL_REQUEST` or of unknown length, or at once 503 when the room is
    /// full. A small request needs none.
    pub fn admit(
        &self,
        content_length: Option<u64>,
    ) -> Result<Option<OwnedSemaphorePermit>, ApiError> {
        if content_length.is_some_and(|length| length <= SMALL_REQUEST) {
            return Ok(None);
        }
        Arc::clone(&self.waiting_room)
            .try_acquire_owned()
            .map(Some)
            .map_err(|_| ApiError::busy())
    }

    /// Run `work` on a blocking thread once a place to compute is free:
    /// 503 when none is free within the wait, 504 at the time limit (the
    /// work is then cancelled and keeps its place until it returns), 500
    /// when it panics.
    pub async fn run<T, F>(&self, work: F) -> Result<T, ApiError>
    where
        T: Send + 'static,
        F: FnOnce(KeepGoing<'_>) -> T + Send + 'static,
    {
        let place = match tokio::time::timeout(self.wait, Arc::clone(&self.running).acquire_owned())
            .await
        {
            Ok(Ok(place)) => place,
            _ => return Err(ApiError::busy()),
        };
        let state = Arc::new(CallState::default());
        let shared = Arc::clone(&state);
        let task = tokio::task::spawn_blocking(move || {
            let _place = place;
            let _ended = Ended(Arc::clone(&shared));
            let keep_going = || {
                if shared.cancelled.load(Ordering::Acquire) {
                    Err(LoadError::Cancelled)
                } else {
                    Ok(())
                }
            };
            work(&keep_going)
        });
        match tokio::time::timeout(self.time_limit, task).await {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(_)) => Err(ApiError::internal("The work stopped unexpectedly.")),
            Err(_) => {
                state.cancelled.store(true, Ordering::Release);
                self.cancelled
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push(Cancelled {
                        state,
                        stuck_at: Instant::now() + self.grace,
                    });
                Err(ApiError::timeout(self.time_limit.as_secs()))
            }
        }
    }

    /// Whether a cancelled call still runs a grace period after its time
    /// limit.
    pub fn stuck(&self) -> bool {
        let mut cancelled = self
            .cancelled
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        cancelled.retain(|call| !call.state.ended.load(Ordering::Acquire));
        let now = Instant::now();
        cancelled.iter().any(|call| now >= call.stuck_at)
    }
}
