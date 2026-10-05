use std::collections::VecDeque;

mod kind;
mod launch;
mod protocol;
mod session_host;

pub use kind::{SessionKind, SessionState};
pub use launch::{
    LaunchPlanError, SessionHostLaunchPlan, SessionLaunchPlan, is_installed, warm_shell_path,
};
pub use protocol::{Attachment, receive_attachment, send_attachment};
pub use session_host::run_session_host;

#[derive(Debug)]
pub struct ReplayBuffer {
    bytes: VecDeque<u8>,
    capacity: usize,
}

impl ReplayBuffer {
    pub fn new(capacity: usize) -> Self {
        Self {
            bytes: VecDeque::with_capacity(capacity),
            capacity,
        }
    }

    pub fn push(&mut self, chunk: &[u8]) {
        if chunk.len() >= self.capacity {
            self.bytes.clear();
            self.bytes
                .extend(chunk[chunk.len() - self.capacity..].iter().copied());
            return;
        }

        let excess = self
            .bytes
            .len()
            .saturating_add(chunk.len())
            .saturating_sub(self.capacity);
        self.bytes.drain(..excess);
        self.bytes.extend(chunk.iter().copied());
    }

    pub fn snapshot(&self) -> Vec<u8> {
        self.bytes.iter().copied().collect()
    }

    pub fn take(&mut self) -> Vec<u8> {
        self.bytes.drain(..).collect()
    }
}
