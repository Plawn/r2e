//! [`TaskRegistryHandle`]: shared, tag-addressed collection of background
//! tasks handed from controller registration to subsystem serve hooks.

use std::any::{Any, TypeId};
use std::sync::{Arc, Mutex};

/// Handle to a task registry for collecting background tasks from
/// multiple subsystems (scheduler, gRPC, custom plugins, …).
///
/// Tasks are tagged at insertion time with an owner `TypeId` so each
/// subsystem's serve hook can drain only the tasks it owns. Cloneable
/// (internally `Arc`) so all hooks share the same backing store.
#[derive(Clone)]
pub struct TaskRegistryHandle {
    inner: Arc<Mutex<Vec<TaggedTask>>>,
}

struct TaggedTask {
    owner: TypeId,
    task: Box<dyn Any + Send>,
}

impl TaskRegistryHandle {
    /// Create a new empty task registry handle.
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Add type-erased tasks to the registry, tagged with the given owner
    /// marker type. The same marker must be used by the consuming serve
    /// hook's `take_of::<Tag>()` call.
    pub fn add_boxed_for<Tag: 'static>(&self, tasks: Vec<Box<dyn Any + Send>>) {
        let owner = TypeId::of::<Tag>();
        let mut guard = self.inner.lock().unwrap();
        guard.extend(tasks.into_iter().map(|task| TaggedTask { owner, task }));
    }

    /// Add type-erased tasks tagged as "anonymous" (retrievable only by
    /// `take_all`). Retained for the single-consumer case where no tag
    /// marker is available; new call sites should use `add_boxed_for`.
    pub fn add_boxed(&self, tasks: Vec<Box<dyn Any + Send>>) {
        self.add_boxed_for::<AnonymousTask>(tasks);
    }

    /// Drain all tasks tagged with the given owner marker.
    pub fn take_of<Tag: 'static>(&self) -> Vec<Box<dyn Any + Send>> {
        let wanted = TypeId::of::<Tag>();
        let mut guard = self.inner.lock().unwrap();
        let mut kept = Vec::with_capacity(guard.len());
        let mut taken = Vec::new();
        for entry in std::mem::take(&mut *guard) {
            if entry.owner == wanted {
                taken.push(entry.task);
            } else {
                kept.push(entry);
            }
        }
        *guard = kept;
        taken
    }

    /// Drain every task in the registry, regardless of owner tag.
    ///
    /// Intended for single-consumer subsystems (historically the scheduler).
    /// When multiple subsystems register serve hooks, prefer `take_of::<Tag>()`
    /// so each hook only sees its own tasks.
    pub fn take_all(&self) -> Vec<Box<dyn Any + Send>> {
        let mut guard = self.inner.lock().unwrap();
        std::mem::take(&mut *guard)
            .into_iter()
            .map(|e| e.task)
            .collect()
    }
}

/// Marker type used for tasks added via the untagged `add_boxed` path.
struct AnonymousTask;

/// Marker tag for scheduled tasks (interval / cron / delayed) produced by
/// controllers and consumed by the `r2e-scheduler` serve hook.
///
/// Defined in `r2e-core` so both sides — the controller registration path
/// (which doesn't depend on `r2e-scheduler`) and the scheduler's `on_serve`
/// hook — can agree on the tag without introducing a reverse dependency.
pub struct ScheduledTaskMarker;

impl Default for TaskRegistryHandle {
    fn default() -> Self {
        Self::new()
    }
}
