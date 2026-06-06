use tokio::sync::watch;

#[derive(Debug, Clone)]
pub struct NacelleShutdown {
    sender: watch::Sender<bool>,
}

#[derive(Debug, Clone)]
pub struct NacelleShutdownToken {
    receiver: watch::Receiver<bool>,
}

impl Default for NacelleShutdown {
    fn default() -> Self {
        Self::new()
    }
}

impl NacelleShutdown {
    pub fn new() -> Self {
        Self::pair().0
    }

    pub fn pair() -> (Self, NacelleShutdownToken) {
        let (sender, receiver) = watch::channel(false);
        (Self { sender }, NacelleShutdownToken { receiver })
    }

    pub fn shutdown(&self) {
        self.sender.send_replace(true);
    }

    pub fn token(&self) -> NacelleShutdownToken {
        NacelleShutdownToken {
            receiver: self.sender.subscribe(),
        }
    }
}

impl NacelleShutdownToken {
    pub fn is_shutdown(&self) -> bool {
        *self.receiver.borrow()
    }

    pub async fn changed(&mut self) -> bool {
        if self.is_shutdown() {
            return true;
        }
        self.receiver.changed().await.is_ok() && self.is_shutdown()
    }
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn token_created_after_shutdown_is_immediately_ready() {
        let shutdown = super::NacelleShutdown::new();
        shutdown.shutdown();
        let mut token = shutdown.token();

        let observed = tokio::time::timeout(std::time::Duration::from_millis(25), token.changed())
            .await
            .expect("already-shutdown token should not wait");
        assert!(observed);
    }
}
