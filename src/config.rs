#[derive(Debug, Clone)]
pub struct NacelleConfig {
    pub read_buffer_capacity: usize,
    pub response_buffer_capacity: usize,
    pub max_frame_len: usize,
    pub request_body_chunk_size: usize,
    pub request_body_channel_capacity: usize,
    pub max_concurrent_requests_per_connection: usize,
    pub max_buffered_request_body_per_request: usize,
}

impl Default for NacelleConfig {
    fn default() -> Self {
        Self {
            // 64 KB read/write buffers match typical network MTU aggregation and
            // saturate a socket receive buffer in one syscall for payloads up to 64 KB.
            read_buffer_capacity: 64 * 1024,
            response_buffer_capacity: 64 * 1024,
            // 16 MB is the practical upper bound for a single frame; payloads of
            // 1–10 KB are fastest, but streaming splits anything larger into chunks.
            max_frame_len: 16 * 1024 * 1024,
            // 64 KB chunks align with OS socket buffer granularity, minimising the
            // number of send(2) syscalls for large streaming responses.
            request_body_chunk_size: 64 * 1024,
            request_body_channel_capacity: 4,
            max_concurrent_requests_per_connection: 1,
            max_buffered_request_body_per_request: 64 * 1024,
        }
    }
}

impl NacelleConfig {
    pub fn with_read_buffer_capacity(mut self, capacity: usize) -> Self {
        self.read_buffer_capacity = capacity.max(1024);
        self
    }

    pub fn with_response_buffer_capacity(mut self, capacity: usize) -> Self {
        self.response_buffer_capacity = capacity.max(1024);
        self
    }

    pub fn with_max_frame_len(mut self, max_frame_len: usize) -> Self {
        self.max_frame_len = max_frame_len.max(24);
        self
    }

    pub fn with_request_body_chunk_size(mut self, chunk_size: usize) -> Self {
        self.request_body_chunk_size = chunk_size.max(1);
        self
    }

    pub fn with_request_body_channel_capacity(mut self, capacity: usize) -> Self {
        self.request_body_channel_capacity = capacity.max(1);
        self
    }

    pub fn with_max_concurrent_requests_per_connection(mut self, count: usize) -> Self {
        self.max_concurrent_requests_per_connection = count.max(1);
        self
    }

    pub fn with_max_buffered_request_body_per_request(mut self, bytes: usize) -> Self {
        self.max_buffered_request_body_per_request = bytes.max(1);
        self
    }
}
