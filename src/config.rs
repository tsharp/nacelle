#[derive(Debug, Clone)]
pub struct CascadeConfig {
    pub read_buffer_capacity: usize,
    pub response_buffer_capacity: usize,
    pub max_frame_len: usize,
    pub request_body_chunk_size: usize,
    pub request_body_channel_capacity: usize,
    pub max_concurrent_requests_per_connection: usize,
    pub max_buffered_request_body_per_request: usize,
}

impl Default for CascadeConfig {
    fn default() -> Self {
        Self {
            read_buffer_capacity: 8 * 1024,
            response_buffer_capacity: 8 * 1024,
            max_frame_len: 8 * 1024 * 1024,
            request_body_chunk_size: 8 * 1024,
            request_body_channel_capacity: 4,
            max_concurrent_requests_per_connection: 1,
            max_buffered_request_body_per_request: 64 * 1024,
        }
    }
}

impl CascadeConfig {
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
