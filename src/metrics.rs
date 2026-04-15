#[derive(Debug, Clone, Default)]
pub struct Metrics;

impl Metrics {
    pub fn request_started(&self) {}
    pub fn request_completed(&self) {}
    pub fn request_failed(&self) {}
}
