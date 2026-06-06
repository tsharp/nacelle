use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::error::NacelleError;

#[derive(Debug, Clone)]
pub struct NacelleLimits {
    pub max_connections: usize,
    pub max_in_flight_requests: usize,
    pub max_streaming_tasks: usize,
    pub max_connections_per_peer: Option<usize>,
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
            max_connections_per_peer: None,
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

    pub fn with_max_connections_per_peer(mut self, max: usize) -> Self {
        self.max_connections_per_peer = Some(max.max(1));
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
    active_connections: AtomicUsize,
    active_requests: AtomicUsize,
    active_streaming_tasks: AtomicUsize,
    peer_connections: Mutex<HashMap<IpAddr, usize>>,
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
                limits,
                active_connections: AtomicUsize::new(0),
                active_requests: AtomicUsize::new(0),
                active_streaming_tasks: AtomicUsize::new(0),
                peer_connections: Mutex::new(HashMap::new()),
                memory_used: AtomicUsize::new(0),
            }),
        }
    }

    pub fn limits(&self) -> &NacelleLimits {
        &self.inner.limits
    }

    pub fn acquire_connection(&self) -> Result<TrackedPermit, NacelleError> {
        self.acquire_connection_tracked()
    }

    pub fn acquire_connection_tracked(&self) -> Result<TrackedPermit, NacelleError> {
        acquire_counter(
            &self.inner.active_connections,
            self.inner.limits.max_connections,
            "connections",
        )?;
        Ok(TrackedPermit::new(
            self.clone(),
            PermitKind::Connection { peer: None },
        ))
    }

    pub fn acquire_connection_for_peer(&self, peer: IpAddr) -> Result<TrackedPermit, NacelleError> {
        acquire_counter(
            &self.inner.active_connections,
            self.inner.limits.max_connections,
            "connections",
        )?;
        if let Err(error) = self.acquire_peer_connection(peer) {
            self.inner
                .active_connections
                .fetch_sub(1, Ordering::Relaxed);
            return Err(error);
        }
        Ok(TrackedPermit::new(
            self.clone(),
            PermitKind::Connection { peer: Some(peer) },
        ))
    }

    pub fn acquire_request(&self) -> Result<TrackedPermit, NacelleError> {
        self.acquire_request_tracked()
    }

    pub fn acquire_request_tracked(&self) -> Result<TrackedPermit, NacelleError> {
        acquire_counter(
            &self.inner.active_requests,
            self.inner.limits.max_in_flight_requests,
            "in_flight_requests",
        )?;
        Ok(TrackedPermit::new(self.clone(), PermitKind::Request))
    }

    pub fn acquire_streaming_task(&self) -> Result<TrackedPermit, NacelleError> {
        self.acquire_streaming_task_tracked()
    }

    pub fn acquire_streaming_task_tracked(&self) -> Result<TrackedPermit, NacelleError> {
        acquire_counter(
            &self.inner.active_streaming_tasks,
            self.inner.limits.max_streaming_tasks,
            "streaming_tasks",
        )?;
        Ok(TrackedPermit::new(self.clone(), PermitKind::StreamingTask))
    }

    pub fn active_connections(&self) -> usize {
        self.inner.active_connections.load(Ordering::Relaxed)
    }

    pub fn active_requests(&self) -> usize {
        self.inner.active_requests.load(Ordering::Relaxed)
    }

    pub fn active_streaming_tasks(&self) -> usize {
        self.inner.active_streaming_tasks.load(Ordering::Relaxed)
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

    fn acquire_peer_connection(&self, peer: IpAddr) -> Result<(), NacelleError> {
        let Some(limit) = self.inner.limits.max_connections_per_peer else {
            return Ok(());
        };
        let mut peers = self
            .inner
            .peer_connections
            .lock()
            .expect("peer connection map poisoned");
        let current = peers.get(&peer).copied().unwrap_or(0);
        if current >= limit {
            return Err(NacelleError::ResourceLimit("peer_connections"));
        }
        peers.insert(peer, current + 1);
        Ok(())
    }

    fn release_peer_connection(&self, peer: IpAddr) {
        if self.inner.limits.max_connections_per_peer.is_none() {
            return;
        }
        let mut peers = self
            .inner
            .peer_connections
            .lock()
            .expect("peer connection map poisoned");
        match peers.get_mut(&peer) {
            Some(count) if *count > 1 => *count -= 1,
            Some(_) => {
                peers.remove(&peer);
            }
            None => {}
        }
    }
}

fn acquire_counter(
    counter: &AtomicUsize,
    limit: usize,
    name: &'static str,
) -> Result<(), NacelleError> {
    let mut current = counter.load(Ordering::Relaxed);
    loop {
        if current >= limit {
            return Err(NacelleError::ResourceLimit(name));
        }
        let Some(next) = current.checked_add(1) else {
            return Err(NacelleError::ResourceLimit(name));
        };
        match counter.compare_exchange_weak(current, next, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return Ok(()),
            Err(observed) => current = observed,
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum PermitKind {
    Connection { peer: Option<IpAddr> },
    Request,
    StreamingTask,
}

#[derive(Debug)]
pub struct TrackedPermit {
    state: NacelleRuntimeState,
    kind: PermitKind,
}

impl TrackedPermit {
    fn new(state: NacelleRuntimeState, kind: PermitKind) -> Self {
        Self { state, kind }
    }
}

impl Drop for TrackedPermit {
    fn drop(&mut self) {
        match self.kind {
            PermitKind::Connection { peer } => {
                self.state
                    .inner
                    .active_connections
                    .fetch_sub(1, Ordering::Relaxed);
                if let Some(peer) = peer {
                    self.state.release_peer_connection(peer);
                }
            }
            PermitKind::Request => {
                self.state
                    .inner
                    .active_requests
                    .fetch_sub(1, Ordering::Relaxed);
            }
            PermitKind::StreamingTask => {
                self.state
                    .inner
                    .active_streaming_tasks
                    .fetch_sub(1, Ordering::Relaxed);
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

    #[test]
    fn bounded_counters_reject_at_limit_and_recover_after_drop() {
        let state =
            NacelleRuntimeState::new(NacelleLimits::default().with_max_in_flight_requests(1));

        let request = state.acquire_request_tracked().expect("first request");
        assert!(matches!(
            state.acquire_request_tracked(),
            Err(NacelleError::ResourceLimit("in_flight_requests"))
        ));

        drop(request);
        let _request = state
            .acquire_request_tracked()
            .expect("request permit should recover after drop");
        assert_eq!(state.active_requests(), 1);
    }

    #[test]
    fn per_peer_connection_limit_rejects_and_recovers_after_drop() {
        let peer = "127.0.0.1".parse().expect("valid ip");
        let state =
            NacelleRuntimeState::new(NacelleLimits::default().with_max_connections_per_peer(1));

        let connection = state
            .acquire_connection_for_peer(peer)
            .expect("first peer connection");
        assert!(matches!(
            state.acquire_connection_for_peer(peer),
            Err(NacelleError::ResourceLimit("peer_connections"))
        ));

        drop(connection);
        let _connection = state
            .acquire_connection_for_peer(peer)
            .expect("peer connection should recover after drop");
        assert_eq!(state.active_connections(), 1);
    }
}
