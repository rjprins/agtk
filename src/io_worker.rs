use std::sync::mpsc;

use futures_channel::oneshot;

/// One ordered queue for durable writes and bounded filesystem operations.
/// GTK owns widgets, this worker only accepts Send data.
#[derive(Clone)]
pub struct IoWorker(mpsc::Sender<Box<dyn FnOnce() + Send>>);

impl Default for IoWorker {
    fn default() -> Self {
        let (sender, receiver) = mpsc::channel::<Box<dyn FnOnce() + Send>>();
        std::thread::spawn(move || {
            for work in receiver {
                work();
            }
        });
        Self(sender)
    }
}

impl IoWorker {
    pub fn submit<T: Send + 'static>(
        &self,
        work: impl FnOnce() -> T + Send + 'static,
    ) -> oneshot::Receiver<T> {
        let (sender, receiver) = oneshot::channel();
        let _ = self.0.send(Box::new(move || {
            let _ = sender.send(work());
        }));
        receiver
    }
}
