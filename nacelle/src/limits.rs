use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::error::NacelleError;

#[derive(Debug, Clone)]
pub struct NacelleLimits {
    pub max_connections: usize,
    pub max_in_flight_requests: usize,
    pub max_streaming_tasks: usize,
    pub max_memory_bytes: usize,
    pub max_request_body_bytes: usize,
    pub max_response_body_bytes: usize,
    pub read_timeout: Option<Duration>,
    pub write_timeout: Option<Duration>,
    pub handler_timeout: Option<Duration>,
    pub idle_timeout: Option<Duration>,
}

impl Default for NacelleLimits {
    fn default() -> Self {
        Self {
            max_connections: usize::MAX,
            max_in_flight_requests: usize::MAX,
            max_streaming_tasks: usize::MAX,
            max_memory_bytes: usize::MAX,
            max_request_body_bytes: 16 * 1024 * 1024,
            max_response_body_bytes: 16 * 1024 * 1024,
            read_timeout: None,
            write_timeout: None,
            handler_timeout: None,
            idle_timeout: None,
        }
    }
}

impl NacelleLimits {
    pub fn with_max_connections(mut self, max: usize) -> Self {
        self.max_connections = max.max(1);
        self
    }

    pub fn with_max_in_flight_requests(mut self, max: usize) -> Self {
        self.max_in_flight_requests = max.max(1);
        self
    }

    pub fn with_max_streaming_tasks(mut self, max: usize) -> Self {
        self.max_streaming_tasks = max.max(1);
        self
    }

    pub fn with_max_memory_bytes(mut self, max: usize) -> Self {
        self.max_memory_bytes = max.max(1);
        self
    }

    pub fn with_max_request_body_bytes(mut self, max: usize) -> Self {
        self.max_request_body_bytes = max;
        self
    }

    pub fn with_max_response_body_bytes(mut self, max: usize) -> Self {
        self.max_response_body_bytes = max;
        self
    }

    pub fn with_read_timeout(mut self, timeout: Duration) -> Self {
        self.read_timeout = Some(timeout);
        self
    }

    pub fn with_write_timeout(mut self, timeout: Duration) -> Self {
        self.write_timeout = Some(timeout);
        self
    }

    pub fn with_handler_timeout(mut self, timeout: Duration) -> Self {
        self.handler_timeout = Some(timeout);
        self
    }

    pub fn with_idle_timeout(mut self, timeout: Duration) -> Self {
        self.idle_timeout = Some(timeout);
        self
    }
}

#[derive(Debug, Clone)]
pub struct NacelleRuntimeState {
    inner: Arc<NacelleRuntimeStateInner>,
}

#[derive(Debug)]
struct NacelleRuntimeStateInner {
    limits: NacelleLimits,
    connections: Option<Arc<Semaphore>>,
    requests: Option<Arc<Semaphore>>,
    streaming_tasks: Option<Arc<Semaphore>>,
    memory_used: AtomicUsize,
}

impl Default for NacelleRuntimeState {
    fn default() -> Self {
        Self::new(NacelleLimits::default())
    }
}

impl NacelleRuntimeState {
    pub fn new(limits: NacelleLimits) -> Self {
        Self {
            inner: Arc::new(NacelleRuntimeStateInner {
                connections: finite_semaphore(limits.max_connections),
                requests: finite_semaphore(limits.max_in_flight_requests),
                streaming_tasks: finite_semaphore(limits.max_streaming_tasks),
                limits,
                memory_used: AtomicUsize::new(0),
            }),
        }
    }

    pub fn limits(&self) -> &NacelleLimits {
        &self.inner.limits
    }

    pub fn acquire_connection(&self) -> Result<Option<OwnedSemaphorePermit>, NacelleError> {
        acquire(&self.inner.connections, "connections")
    }

    pub fn acquire_request(&self) -> Result<Option<OwnedSemaphorePermit>, NacelleError> {
        acquire(&self.inner.requests, "in_flight_requests")
    }

    pub fn acquire_streaming_task(&self) -> Result<Option<OwnedSemaphorePermit>, NacelleError> {
        acquire(&self.inner.streaming_tasks, "streaming_tasks")
    }

    pub fn reserve_memory(&self, bytes: usize) -> Result<MemoryReservation, NacelleError> {
        if bytes == 0 || self.inner.limits.max_memory_bytes == usize::MAX {
            return Ok(MemoryReservation::empty());
        }

        let limit = self.inner.limits.max_memory_bytes;
        let mut current = self.inner.memory_used.load(Ordering::Relaxed);
        loop {
            let Some(next) = current.checked_add(bytes) else {
                return Err(NacelleError::ResourceLimit("memory_bytes"));
            };
            if next > limit {
                return Err(NacelleError::ResourceLimit("memory_bytes"));
            }
            match self.inner.memory_used.compare_exchange_weak(
                current,
                next,
                Ordering::AcqRel,
                Ordering::Relaxed,
            ) {
                Ok(_) => {
                    return Ok(MemoryReservation {
                        state: Some(self.clone()),
                        bytes,
                    });
                }
                Err(observed) => current = observed,
            }
        }
    }

    fn release_memory(&self, bytes: usize) {
        if bytes != 0 && self.inner.limits.max_memory_bytes != usize::MAX {
            self.inner.memory_used.fetch_sub(bytes, Ordering::AcqRel);
        }
    }
}

fn finite_semaphore(limit: usize) -> Option<Arc<Semaphore>> {
    if limit == usize::MAX {
        None
    } else {
        Some(Arc::new(Semaphore::new(limit)))
    }
}

fn acquire(
    semaphore: &Option<Arc<Semaphore>>,
    name: &'static str,
) -> Result<Option<OwnedSemaphorePermit>, NacelleError> {
    let Some(semaphore) = semaphore else {
        return Ok(None);
    };
    semaphore
        .clone()
        .try_acquire_owned()
        .map(Some)
        .map_err(|_| NacelleError::ResourceLimit(name))
}

#[derive(Debug)]
pub struct MemoryReservation {
    state: Option<NacelleRuntimeState>,
    bytes: usize,
}

impl MemoryReservation {
    fn empty() -> Self {
        Self {
            state: None,
            bytes: 0,
        }
    }
}

impl Drop for MemoryReservation {
    fn drop(&mut self) {
        if let Some(state) = &self.state {
            state.release_memory(self.bytes);
        }
    }
}
