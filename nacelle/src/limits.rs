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
    pub http_header_read_timeout: Option<Duration>,
    pub http_request_body_read_timeout: Option<Duration>,
    pub http_response_write_timeout: Option<Duration>,
    pub http_keep_alive: bool,
    pub http_max_connection_age: Option<Duration>,
}

impl Default for NacelleLimits {
    fn default() -> Self {
        Self {
            max_connections: 16_384,
            max_in_flight_requests: 65_536,
            max_streaming_tasks: 8_192,
            max_memory_bytes: 512 * 1024 * 1024,
            max_request_body_bytes: 16 * 1024 * 1024,
            max_response_body_bytes: 16 * 1024 * 1024,
            read_timeout: Some(Duration::from_secs(30)),
            write_timeout: Some(Duration::from_secs(30)),
            handler_timeout: Some(Duration::from_secs(60)),
            idle_timeout: Some(Duration::from_secs(120)),
            http_header_read_timeout: Some(Duration::from_secs(30)),
            http_request_body_read_timeout: Some(Duration::from_secs(30)),
            http_response_write_timeout: Some(Duration::from_secs(30)),
            http_keep_alive: true,
            http_max_connection_age: None,
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

    pub fn with_http_header_read_timeout(mut self, timeout: Duration) -> Self {
        self.http_header_read_timeout = Some(timeout);
        self
    }

    pub fn with_http_request_body_read_timeout(mut self, timeout: Duration) -> Self {
        self.http_request_body_read_timeout = Some(timeout);
        self
    }

    pub fn with_http_response_write_timeout(mut self, timeout: Duration) -> Self {
        self.http_response_write_timeout = Some(timeout);
        self
    }

    pub fn with_http_keep_alive(mut self, keep_alive: bool) -> Self {
        self.http_keep_alive = keep_alive;
        self
    }

    pub fn with_http_max_connection_age(mut self, max_age: Duration) -> Self {
        self.http_max_connection_age = Some(max_age);
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
    active_connections: AtomicUsize,
    active_requests: AtomicUsize,
    active_streaming_tasks: AtomicUsize,
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
                active_connections: AtomicUsize::new(0),
                active_requests: AtomicUsize::new(0),
                active_streaming_tasks: AtomicUsize::new(0),
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

    pub fn acquire_connection_tracked(&self) -> Result<TrackedPermit, NacelleError> {
        let permit = self.acquire_connection()?;
        self.inner.active_connections.fetch_add(1, Ordering::AcqRel);
        Ok(TrackedPermit::new(
            self.clone(),
            PermitKind::Connection,
            permit,
        ))
    }

    pub fn acquire_request(&self) -> Result<Option<OwnedSemaphorePermit>, NacelleError> {
        acquire(&self.inner.requests, "in_flight_requests")
    }

    pub fn acquire_request_tracked(&self) -> Result<TrackedPermit, NacelleError> {
        let permit = self.acquire_request()?;
        self.inner.active_requests.fetch_add(1, Ordering::AcqRel);
        Ok(TrackedPermit::new(
            self.clone(),
            PermitKind::Request,
            permit,
        ))
    }

    pub fn acquire_streaming_task(&self) -> Result<Option<OwnedSemaphorePermit>, NacelleError> {
        acquire(&self.inner.streaming_tasks, "streaming_tasks")
    }

    pub fn acquire_streaming_task_tracked(&self) -> Result<TrackedPermit, NacelleError> {
        let permit = self.acquire_streaming_task()?;
        self.inner
            .active_streaming_tasks
            .fetch_add(1, Ordering::AcqRel);
        Ok(TrackedPermit::new(
            self.clone(),
            PermitKind::StreamingTask,
            permit,
        ))
    }

    pub fn active_connections(&self) -> usize {
        self.inner.active_connections.load(Ordering::Acquire)
    }

    pub fn active_requests(&self) -> usize {
        self.inner.active_requests.load(Ordering::Acquire)
    }

    pub fn active_streaming_tasks(&self) -> usize {
        self.inner.active_streaming_tasks.load(Ordering::Acquire)
    }

    pub fn memory_used_bytes(&self) -> usize {
        self.inner.memory_used.load(Ordering::Acquire)
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

#[derive(Debug, Clone, Copy)]
enum PermitKind {
    Connection,
    Request,
    StreamingTask,
}

#[derive(Debug)]
pub struct TrackedPermit {
    state: NacelleRuntimeState,
    kind: PermitKind,
    _permit: Option<OwnedSemaphorePermit>,
}

impl TrackedPermit {
    fn new(
        state: NacelleRuntimeState,
        kind: PermitKind,
        permit: Option<OwnedSemaphorePermit>,
    ) -> Self {
        Self {
            state,
            kind,
            _permit: permit,
        }
    }
}

impl Drop for TrackedPermit {
    fn drop(&mut self) {
        match self.kind {
            PermitKind::Connection => {
                self.state
                    .inner
                    .active_connections
                    .fetch_sub(1, Ordering::AcqRel);
            }
            PermitKind::Request => {
                self.state
                    .inner
                    .active_requests
                    .fetch_sub(1, Ordering::AcqRel);
            }
            PermitKind::StreamingTask => {
                self.state
                    .inner
                    .active_streaming_tasks
                    .fetch_sub(1, Ordering::AcqRel);
            }
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_limits_are_bounded_for_production_safety() {
        let limits = NacelleLimits::default();

        assert_ne!(limits.max_connections, usize::MAX);
        assert_ne!(limits.max_in_flight_requests, usize::MAX);
        assert_ne!(limits.max_streaming_tasks, usize::MAX);
        assert_ne!(limits.max_memory_bytes, usize::MAX);
        assert!(limits.read_timeout.is_some());
        assert!(limits.write_timeout.is_some());
        assert!(limits.handler_timeout.is_some());
        assert!(limits.idle_timeout.is_some());
        assert!(limits.http_header_read_timeout.is_some());
        assert!(limits.http_request_body_read_timeout.is_some());
        assert!(limits.http_response_write_timeout.is_some());
        assert!(limits.http_keep_alive);
    }

    #[test]
    fn runtime_state_reports_active_permits_and_memory() {
        let state = NacelleRuntimeState::new(
            NacelleLimits::default()
                .with_max_connections(1)
                .with_max_in_flight_requests(1)
                .with_max_streaming_tasks(1)
                .with_max_memory_bytes(1024),
        );

        let connection = state
            .acquire_connection_tracked()
            .expect("connection permit");
        let request = state.acquire_request_tracked().expect("request permit");
        let streaming = state
            .acquire_streaming_task_tracked()
            .expect("streaming permit");
        let memory = state.reserve_memory(512).expect("memory reservation");

        assert_eq!(state.active_connections(), 1);
        assert_eq!(state.active_requests(), 1);
        assert_eq!(state.active_streaming_tasks(), 1);
        assert_eq!(state.memory_used_bytes(), 512);

        drop(connection);
        drop(request);
        drop(streaming);
        drop(memory);

        assert_eq!(state.active_connections(), 0);
        assert_eq!(state.active_requests(), 0);
        assert_eq!(state.active_streaming_tasks(), 0);
        assert_eq!(state.memory_used_bytes(), 0);
    }
}
