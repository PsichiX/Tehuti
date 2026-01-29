use std::{error::Error, time::Duration};

pub fn unbounded<T>() -> (Sender<T>, Receiver<T>) {
    let (tx, rx) = flume::unbounded();
    (Sender(tx), Receiver(rx))
}

pub fn bounded<T>(capacity: usize) -> (Sender<T>, Receiver<T>) {
    let (tx, rx) = flume::bounded(capacity);
    (Sender(tx), Receiver(rx))
}

pub struct Sender<T>(flume::Sender<T>);

impl<T> Sender<T> {
    pub fn send(&self, value: T) -> Result<(), Box<dyn Error>>
    where
        T: 'static,
    {
        self.0.send(value)?;
        Ok(())
    }

    pub async fn send_async(&self, value: T) -> Result<(), Box<dyn Error>>
    where
        T: 'static,
    {
        self.0.send_async(value).await?;
        Ok(())
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn is_full(&self) -> bool {
        self.0.is_full()
    }

    pub fn inner(&self) -> &flume::Sender<T> {
        &self.0
    }
}

impl<T> Clone for Sender<T> {
    fn clone(&self) -> Self {
        Sender(self.0.clone())
    }
}

impl<T> std::fmt::Debug for Sender<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Debug::fmt(&self.0, f)
    }
}

pub struct Receiver<T>(flume::Receiver<T>);

impl<T> Receiver<T> {
    pub fn recv(&self) -> Result<Option<T>, Box<dyn Error>> {
        match self.0.try_recv() {
            Ok(value) => Ok(Some(value)),
            Err(flume::TryRecvError::Empty) => Ok(None),
            Err(e) => Err(Box::new(e)),
        }
    }

    pub fn try_recv(&self) -> Option<T> {
        self.0.try_recv().ok()
    }

    pub fn recv_blocking(&self) -> Result<T, Box<dyn Error>> {
        Ok(self.0.recv()?)
    }

    pub fn recv_blocking_timeout(&self, duration: Duration) -> Result<T, Box<dyn Error>> {
        Ok(self.0.recv_timeout(duration)?)
    }

    pub async fn recv_async(&self) -> Result<T, Box<dyn Error>> {
        Ok(self.0.recv_async().await?)
    }

    pub fn iter(&self) -> impl Iterator<Item = T> + '_ {
        std::iter::from_fn(|| self.try_recv())
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn is_full(&self) -> bool {
        self.0.is_full()
    }

    pub fn inner(&self) -> &flume::Receiver<T> {
        &self.0
    }
}

impl<T> Clone for Receiver<T> {
    fn clone(&self) -> Self {
        Receiver(self.0.clone())
    }
}

impl<T> std::fmt::Debug for Receiver<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Debug::fmt(&self.0, f)
    }
}

pub struct Duplex<T> {
    pub sender: Sender<T>,
    pub receiver: Receiver<T>,
}

impl<T> Duplex<T> {
    pub fn unbounded() -> Self {
        let (tx, rx) = unbounded();
        Self {
            sender: tx,
            receiver: rx,
        }
    }

    pub fn bounded(capacity: usize) -> Self {
        let (tx, rx) = bounded(capacity);
        Self {
            sender: tx,
            receiver: rx,
        }
    }

    pub fn crossing_unbounded() -> (Self, Self) {
        let (tx1, rx1) = unbounded();
        let (tx2, rx2) = unbounded();
        (
            Self {
                sender: tx1,
                receiver: rx2,
            },
            Self {
                sender: tx2,
                receiver: rx1,
            },
        )
    }

    pub fn crossing_bounded(capacity: usize) -> (Self, Self) {
        let (tx1, rx1) = bounded(capacity);
        let (tx2, rx2) = bounded(capacity);
        (
            Self {
                sender: tx1,
                receiver: rx2,
            },
            Self {
                sender: tx2,
                receiver: rx1,
            },
        )
    }

    pub fn new(sender: Sender<T>, receiver: Receiver<T>) -> Self {
        Self { sender, receiver }
    }
}

impl<T> Clone for Duplex<T> {
    fn clone(&self) -> Self {
        Self {
            sender: self.sender.clone(),
            receiver: self.receiver.clone(),
        }
    }
}

impl<T> std::fmt::Debug for Duplex<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Duplex")
            .field("sender", &self.sender)
            .field("receiver", &self.receiver)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_channels() {
        let (sender, receiver) = unbounded::<u8>();

        assert_eq!(receiver.try_recv(), None);
        sender.send(1).unwrap();
        sender.send(2).unwrap();
        assert_eq!(receiver.try_recv(), Some(1));
        sender.send(3).unwrap();
        assert_eq!(receiver.try_recv(), Some(2));
        assert_eq!(receiver.try_recv(), Some(3));
        assert_eq!(receiver.try_recv(), None);
    }
}
