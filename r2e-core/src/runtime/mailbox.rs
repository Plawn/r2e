//! `Mailboxes<M>` — explicit, counted message passing between planes.
//!
//! One bounded channel per sharded worker. Receivers are attached from
//! *inside* the worker ([`Mailboxes::attach`], typically in a per-worker
//! service factory); senders are shared and usable from anywhere — the
//! control plane (`#[scheduled]` aggregators, admin endpoints), another
//! worker, or the worker itself.
//!
//! Every delivery is attributed to the target worker's
//! [`WorkerSlot`](super::worker_set::WorkerSlot):
//!
//! - a **local crossing** when the sender runs on the target worker's thread;
//! - a **remote crossing** otherwise;
//! - plus queue depth, total sends and queued time.
//!
//! A service that claims to be shared-nothing must show ~zero remote
//! crossings on its hot path; `WorkerSet::snapshot()` (and the Prometheus
//! `WorkerCollector`) make that a number instead of a belief.
//!
//! ```ignore
//! let workers = WorkerSet::new();
//! let mail: Mailboxes<Cmd> = Mailboxes::new(workers.clone(), 256);
//!
//! AppBuilder::new()
//!     .provide(workers.clone())
//!     .provide(mail.clone())
//!     .per_worker_service({
//!         let mail = mail.clone();
//!         move |worker: WorkerContext| {
//!             let mut inbox = mail.attach(&worker).expect("attached once per worker");
//!             let state = Rc::new(RefCell::new(ShardState::default()));
//!             worker.spawn_local(async move {
//!                 while let Some(cmd) = inbox.recv().await {
//!                     match cmd {
//!                         Cmd::Stats(reply) => { let _ = reply.send(state.borrow().stats()); }
//!                     }
//!                 }
//!             });
//!             async { Ok::<_, BoxError>(()) }
//!         }
//!     })
//!
//! // Control plane, off the hot path:
//! let stats = mail.ask(2, Cmd::Stats).await?;          // one worker
//! let all = mail.ask_all(Cmd::Stats).await;              // every worker
//! ```

use std::sync::Arc;
use std::time::Instant;

use arc_swap::ArcSwap;

use super::worker::{WorkerContext, WorkerInfo};
use super::worker_set::{WorkerSet, WorkerSlot};
use crate::rt::sync::{mpsc, oneshot};

/// Why a message could not be delivered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MailboxError {
    /// No worker with that index (the set has fewer workers).
    NoSuchWorker(usize),
    /// The worker exists but has not attached its mailbox (its services are
    /// not up yet, or it never attached).
    NotAttached(usize),
    /// The worker dropped its mailbox (it is shutting down).
    Closed(usize),
    /// `try_send_to`: the mailbox is at capacity.
    Full(usize),
    /// `ask`: the worker dropped the reply sender without answering.
    NoReply(usize),
}

impl std::fmt::Display for MailboxError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoSuchWorker(w) => write!(f, "no worker {w}"),
            Self::NotAttached(w) => write!(f, "worker {w} has not attached its mailbox"),
            Self::Closed(w) => write!(f, "worker {w} closed its mailbox"),
            Self::Full(w) => write!(f, "worker {w}'s mailbox is full"),
            Self::NoReply(w) => write!(f, "worker {w} dropped the reply without answering"),
        }
    }
}

impl std::error::Error for MailboxError {}

struct Envelope<M> {
    msg: M,
    sent_at: Instant,
}

struct Inner<M> {
    set: WorkerSet,
    capacity: usize,
    senders: ArcSwap<Vec<Option<mpsc::Sender<Envelope<M>>>>>,
}

/// Shared sender side of every worker's mailbox. Cheap to clone; a bean.
pub struct Mailboxes<M> {
    inner: Arc<Inner<M>>,
}

impl<M> Clone for Mailboxes<M> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl<M> std::fmt::Debug for Mailboxes<M> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let attached = self
            .inner
            .senders
            .load()
            .iter()
            .filter(|s| s.is_some())
            .count();
        f.debug_struct("Mailboxes")
            .field("attached", &attached)
            .field("capacity", &self.inner.capacity)
            .finish()
    }
}

impl<M: Send + 'static> Mailboxes<M> {
    /// Mailboxes over `set`'s workers, each bounded to `capacity` messages.
    /// Slots are created lazily by [`attach`](Self::attach), so the set need
    /// not be sized yet.
    pub fn new(set: WorkerSet, capacity: usize) -> Self {
        Self {
            inner: Arc::new(Inner {
                set,
                capacity: capacity.max(1),
                senders: ArcSwap::from_pointee(Vec::new()),
            }),
        }
    }

    /// The [`WorkerSet`] these mailboxes report into.
    pub fn worker_set(&self) -> &WorkerSet {
        &self.inner.set
    }

    /// Number of workers that have attached a mailbox.
    pub fn attached(&self) -> usize {
        self.inner
            .senders
            .load()
            .iter()
            .filter(|s| s.is_some())
            .count()
    }

    /// Create worker `ctx.id()`'s mailbox and hand back its receiver. Call
    /// once per worker, on the worker; the receiver is `!Send`-free but
    /// intended to be consumed by a `spawn_local` task.
    ///
    /// Fails with [`MailboxError::NotAttached`]'s sibling — a second attach on
    /// the same worker — as `Err(MailboxError::Full)` is never returned here;
    /// re-attaching replaces nothing and is reported as an error.
    pub fn attach(&self, ctx: &WorkerContext) -> Result<Mailbox<M>, MailboxError> {
        let id = ctx.id();
        let (tx, rx) = mpsc::channel(self.inner.capacity);
        let mut duplicate = false;
        self.inner.senders.rcu(|cur| {
            let mut v: Vec<Option<mpsc::Sender<Envelope<M>>>> = (**cur).clone();
            if v.len() <= id {
                v.resize_with(id + 1, || None);
            }
            if v[id].is_some() {
                duplicate = true;
            } else {
                v[id] = Some(tx.clone());
            }
            v
        });
        if duplicate {
            return Err(MailboxError::Closed(id));
        }
        Ok(Mailbox {
            id,
            rx,
            slot: self.inner.set.slot(id),
        })
    }

    fn slot(&self, id: usize) -> Option<Arc<WorkerSlot>> {
        self.inner.set.slot(id)
    }

    fn account(&self, target: usize) {
        if let Some(slot) = self.slot(target) {
            let local = WorkerInfo::current().is_some_and(|w| w.is_data_plane() && w.id() == target);
            slot.record_crossing(local);
            slot.mailbox_enqueued();
        }
    }

    fn undo_account(&self, target: usize) {
        if let Some(slot) = self.slot(target) {
            slot.mailbox_dequeued(std::time::Duration::ZERO);
        }
    }

    /// Deliver `msg` to worker `target`, waiting for room in its mailbox.
    pub async fn send_to(&self, target: usize, msg: M) -> Result<(), MailboxError> {
        let senders = self.inner.senders.load();
        let Some(slot) = senders.get(target) else {
            return Err(self.missing(target));
        };
        let Some(tx) = slot else {
            return Err(self.missing(target));
        };
        self.account(target);
        let env = Envelope {
            msg,
            sent_at: Instant::now(),
        };
        match tx.send(env).await {
            Ok(()) => Ok(()),
            Err(_) => {
                self.undo_account(target);
                Err(MailboxError::Closed(target))
            }
        }
    }

    /// Deliver `msg` to worker `target` without waiting; returns the message
    /// back on failure.
    pub fn try_send_to(&self, target: usize, msg: M) -> Result<(), (MailboxError, M)> {
        let senders = self.inner.senders.load();
        let Some(Some(tx)) = senders.get(target) else {
            return Err((self.missing(target), msg));
        };
        self.account(target);
        let env = Envelope {
            msg,
            sent_at: Instant::now(),
        };
        match tx.try_send(env) {
            Ok(()) => Ok(()),
            Err(mpsc::error::TrySendError::Full(env)) => {
                self.undo_account(target);
                Err((MailboxError::Full(target), env.msg))
            }
            Err(mpsc::error::TrySendError::Closed(env)) => {
                self.undo_account(target);
                Err((MailboxError::Closed(target), env.msg))
            }
        }
    }

    /// Deliver a clone of `msg` to every attached worker. Returns the number
    /// of workers reached; workers that are not attached or closed are
    /// skipped.
    pub async fn broadcast(&self, msg: M) -> usize
    where
        M: Clone,
    {
        let n = self.inner.senders.load().len();
        let mut delivered = 0;
        for target in 0..n {
            if self.send_to(target, msg.clone()).await.is_ok() {
                delivered += 1;
            }
        }
        delivered
    }

    /// [`broadcast`](Self::broadcast) for messages that are not `Clone`:
    /// `make` builds one message per attached worker.
    pub async fn broadcast_with(&self, make: impl Fn() -> M) -> usize {
        let n = self.inner.senders.load().len();
        let mut delivered = 0;
        for target in 0..n {
            if self.send_to(target, make()).await.is_ok() {
                delivered += 1;
            }
        }
        delivered
    }

    /// Request/reply: build the message from a one-shot reply sender, deliver
    /// it, await the answer.
    pub async fn ask<R: Send + 'static>(
        &self,
        target: usize,
        make: impl FnOnce(oneshot::Sender<R>) -> M,
    ) -> Result<R, MailboxError> {
        let (tx, rx) = oneshot::channel();
        self.send_to(target, make(tx)).await?;
        rx.await.map_err(|_| MailboxError::NoReply(target))
    }

    /// [`ask`](Self::ask) every attached worker in turn; one result per
    /// worker index (`Err` for workers that could not be reached).
    pub async fn ask_all<R: Send + 'static>(
        &self,
        make: impl Fn(oneshot::Sender<R>) -> M,
    ) -> Vec<Result<R, MailboxError>> {
        let n = self.inner.senders.load().len();
        let mut out = Vec::with_capacity(n);
        for target in 0..n {
            out.push(self.ask(target, &make).await);
        }
        out
    }

    fn missing(&self, target: usize) -> MailboxError {
        let workers = self.inner.set.workers();
        if workers > 0 && target >= workers {
            MailboxError::NoSuchWorker(target)
        } else {
            MailboxError::NotAttached(target)
        }
    }
}

/// Receiving end of one worker's mailbox. Lives on the worker; each
/// [`recv`](Self::recv) records queue depth and wait time into the worker's
/// slot.
pub struct Mailbox<M> {
    id: usize,
    rx: mpsc::Receiver<Envelope<M>>,
    slot: Option<Arc<WorkerSlot>>,
}

impl<M> std::fmt::Debug for Mailbox<M> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Mailbox").field("worker", &self.id).finish()
    }
}

impl<M> Mailbox<M> {
    /// The worker this mailbox belongs to.
    pub fn worker(&self) -> usize {
        self.id
    }

    /// Next message, or `None` once every [`Mailboxes`] handle is gone.
    pub async fn recv(&mut self) -> Option<M> {
        let env = self.rx.recv().await?;
        if let Some(slot) = &self.slot {
            slot.mailbox_dequeued(env.sent_at.elapsed());
        }
        Some(env.msg)
    }

    /// Next message if one is queued.
    pub fn try_recv(&mut self) -> Option<M> {
        let env = self.rx.try_recv().ok()?;
        if let Some(slot) = &self.slot {
            slot.mailbox_dequeued(env.sent_at.elapsed());
        }
        Some(env.msg)
    }

    /// Stop accepting messages; senders see [`MailboxError::Closed`].
    pub fn close(&mut self) {
        self.rx.close();
    }
}
