use std::time::Duration;

use tokio::net::TcpStream;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NacelleTcpOptions {
    pub nodelay: bool,
    pub keepalive: Option<NacelleTcpKeepalive>,
}

impl Default for NacelleTcpOptions {
    fn default() -> Self {
        Self {
            nodelay: true,
            keepalive: None,
        }
    }
}

impl NacelleTcpOptions {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_nodelay(mut self, nodelay: bool) -> Self {
        self.nodelay = nodelay;
        self
    }

    pub fn with_keepalive(mut self, keepalive: NacelleTcpKeepalive) -> Self {
        self.keepalive = Some(keepalive);
        self
    }

    pub fn without_keepalive(mut self) -> Self {
        self.keepalive = None;
        self
    }

    pub(crate) fn apply_to_stream(&self, stream: &TcpStream) -> std::io::Result<()> {
        stream.set_nodelay(self.nodelay)?;
        if let Some(keepalive) = &self.keepalive {
            socket2::SockRef::from(stream).set_tcp_keepalive(&keepalive.as_socket2())?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NacelleTcpKeepalive {
    pub time: Option<Duration>,
    pub interval: Option<Duration>,
}

impl NacelleTcpKeepalive {
    pub fn new() -> Self {
        Self {
            time: None,
            interval: None,
        }
    }

    pub fn with_time(mut self, time: Duration) -> Self {
        self.time = Some(time);
        self
    }

    pub fn with_interval(mut self, interval: Duration) -> Self {
        self.interval = Some(interval);
        self
    }

    fn as_socket2(&self) -> socket2::TcpKeepalive {
        let mut keepalive = socket2::TcpKeepalive::new();
        if let Some(time) = self.time {
            keepalive = keepalive.with_time(time);
        }
        if let Some(interval) = self.interval {
            keepalive = keepalive.with_interval(interval);
        }
        keepalive
    }
}

impl Default for NacelleTcpKeepalive {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(unix)]
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NacelleUnixSocketOptions {
    pub unlink_stale_path: bool,
    pub permissions: Option<u32>,
}

#[cfg(unix)]
impl NacelleUnixSocketOptions {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_unlink_stale_path(mut self, unlink_stale_path: bool) -> Self {
        self.unlink_stale_path = unlink_stale_path;
        self
    }

    pub fn with_permissions(mut self, permissions: u32) -> Self {
        self.permissions = Some(permissions);
        self
    }

    pub fn without_permissions(mut self) -> Self {
        self.permissions = None;
        self
    }

    pub(crate) fn prepare_path(&self, path: &std::path::Path) -> std::io::Result<()> {
        if !self.unlink_stale_path {
            return Ok(());
        }
        match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
    }

    pub(crate) fn apply_to_path(&self, path: &std::path::Path) -> std::io::Result<()> {
        let Some(permissions) = self.permissions else {
            return Ok(());
        };
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(permissions))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tcp_options_default_to_nodelay_without_keepalive() {
        let options = NacelleTcpOptions::default();

        assert!(options.nodelay);
        assert_eq!(options.keepalive, None);
    }

    #[test]
    fn tcp_keepalive_builder_sets_time_and_interval() {
        let keepalive = NacelleTcpKeepalive::new()
            .with_time(Duration::from_secs(10))
            .with_interval(Duration::from_secs(5));

        assert_eq!(keepalive.time, Some(Duration::from_secs(10)));
        assert_eq!(keepalive.interval, Some(Duration::from_secs(5)));
    }
}
