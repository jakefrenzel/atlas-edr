//! Bounded queues whose memory follows their contents (plan 1b-4a-p2, R6b).
//!
//! std's bounded channel allocates every slot when it is made and stamps each
//! one, so all of it is resident from the start: 45 MB for the sink's 65,536
//! events of 688 bytes, 11.5 MB for the kernel queue. Its unbounded channel
//! allocates blocks of 31 messages and frees each one once read. These queues
//! are the unbounded channel with the cap kept by a counter: a full queue
//! refuses a `try_send` exactly as before, and the caller counts the drop.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, TryRecvError, TrySendError};
use std::time::Duration;

/// A queue of at most `cap` messages for [`QueueSender::try_send`].
pub fn bounded<T>(cap: usize) -> (QueueSender<T>, QueueReceiver<T>) {
    let (tx, rx) = mpsc::channel();
    let len = Arc::new(AtomicUsize::new(0));
    (QueueSender { tx, len: len.clone(), cap: cap.max(1) }, QueueReceiver { rx, len })
}

pub struct QueueSender<T> {
    tx: mpsc::Sender<T>,
    len: Arc<AtomicUsize>,
    cap: usize,
}

impl<T> Clone for QueueSender<T> {
    fn clone(&self) -> Self {
        QueueSender { tx: self.tx.clone(), len: self.len.clone(), cap: self.cap }
    }
}

impl<T> QueueSender<T> {
    /// Queues `msg` unless the queue holds `cap` messages already. Never blocks.
    pub fn try_send(&self, msg: T) -> Result<(), TrySendError<T>> {
        if self.len.fetch_add(1, Ordering::AcqRel) >= self.cap {
            self.len.fetch_sub(1, Ordering::AcqRel);
            return Err(TrySendError::Full(msg));
        }
        self.tx.send(msg).map_err(|e| {
            self.len.fetch_sub(1, Ordering::AcqRel);
            TrySendError::Disconnected(e.0)
        })
    }

    /// The queue's fill, for the diagnostic log (R6, review 7).
    pub fn gauge(&self) -> QueueGauge {
        QueueGauge { len: self.len.clone(), cap: self.cap }
    }

    /// Queues `msg` even past the cap: for the rare records that must not be
    /// dropped (a Sensor Health report). Never blocks.
    pub fn send(&self, msg: T) -> Result<(), mpsc::SendError<T>> {
        self.len.fetch_add(1, Ordering::AcqRel);
        self.tx.send(msg).inspect_err(|_| {
            self.len.fetch_sub(1, Ordering::AcqRel);
        })
    }
}

/// A queue's messages and cap, readable from any thread.
#[derive(Clone)]
pub struct QueueGauge {
    len: Arc<AtomicUsize>,
    cap: usize,
}

impl QueueGauge {
    /// (messages queued now, the cap).
    pub fn read(&self) -> (u64, u64) {
        (self.len.load(Ordering::Acquire) as u64, self.cap as u64)
    }
}

pub struct QueueReceiver<T> {
    rx: mpsc::Receiver<T>,
    len: Arc<AtomicUsize>,
}

impl<T> QueueReceiver<T> {
    pub fn try_recv(&self) -> Result<T, TryRecvError> {
        self.rx.try_recv().inspect(|_| self.taken())
    }

    pub fn recv_timeout(&self, timeout: Duration) -> Result<T, RecvTimeoutError> {
        self.rx.recv_timeout(timeout).inspect(|_| self.taken())
    }

    /// What is queued now, without waiting.
    pub fn try_iter(&self) -> impl Iterator<Item = T> + '_ {
        std::iter::from_fn(|| self.try_recv().ok())
    }

    /// Messages queued now.
    pub fn len(&self) -> usize {
        self.len.load(Ordering::Acquire)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn taken(&self) {
        self.len.fetch_sub(1, Ordering::AcqRel);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_gauge_reads_the_fill_and_the_cap() {
        let (tx, rx) = bounded::<u8>(3);
        let g = tx.gauge();
        tx.try_send(1).unwrap();
        tx.try_send(2).unwrap();
        assert_eq!(g.read(), (2, 3));
        rx.try_recv().unwrap();
        assert_eq!(g.read(), (1, 3), "a receive is seen");
    }

    #[test]
    fn the_cap_refuses_a_try_send_and_a_receive_makes_room() {
        let (tx, rx) = bounded(2);
        assert!(tx.try_send(1).is_ok() && tx.clone().try_send(2).is_ok());
        assert!(matches!(tx.try_send(3), Err(TrySendError::Full(3))));
        assert_eq!(rx.len(), 2);
        assert_eq!(rx.try_recv(), Ok(1));
        assert!(tx.try_send(4).is_ok());
        assert_eq!(rx.try_iter().collect::<Vec<_>>(), [2, 4]);
        assert!(rx.is_empty());
    }

    #[test]
    fn send_queues_past_the_cap() {
        let (tx, rx) = bounded(1);
        tx.try_send(1).unwrap();
        tx.send(2).unwrap();
        assert_eq!(rx.len(), 2);
        assert!(matches!(tx.try_send(3), Err(TrySendError::Full(3))));
        assert_eq!(rx.recv_timeout(Duration::from_millis(10)), Ok(1));
        assert_eq!(rx.recv_timeout(Duration::from_millis(10)), Ok(2));
        assert_eq!(rx.recv_timeout(Duration::from_millis(10)), Err(RecvTimeoutError::Timeout));
    }

    #[test]
    fn a_closed_end_is_reported_and_leaves_the_count_right() {
        let (tx, rx) = bounded::<u8>(1);
        drop(rx);
        assert!(matches!(tx.try_send(1), Err(TrySendError::Disconnected(1))));
        assert!(tx.send(2).is_err());
        assert_eq!(tx.len.load(Ordering::Acquire), 0);
        let (tx, rx) = bounded::<u8>(1);
        tx.try_send(5).unwrap();
        drop(tx);
        assert_eq!(rx.try_recv(), Ok(5));
        assert_eq!(rx.try_recv(), Err(TryRecvError::Disconnected));
    }
}
