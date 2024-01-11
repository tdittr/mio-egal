use flume::{Receiver, Sender, WeakSender};
use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Weak};
use std::task::{Context, Poll};
use std::thread::JoinHandle;

mod waker {
    use crate::TaskId;
    use flume::Sender;
    use std::sync::Arc;
    use std::task::{RawWaker, RawWakerVTable, Waker};

    type State = Arc<StateInner>;

    pub struct StateInner {
        task_id: TaskId,
        ready_sender: Sender<TaskId>,
    }

    impl StateInner {
        pub fn new(task_id: TaskId, ready_sender: Sender<TaskId>) -> Arc<Self> {
            Arc::new(Self {
                task_id,
                ready_sender,
            })
        }

        pub fn into_waker(self: Arc<Self>) -> Waker {
            unsafe { Waker::from_raw(RawWaker::new(Arc::into_raw(self) as *const (), &VTABLE)) }
        }

        pub fn wake(&self) {
            println!("Waking: {:?}", self.task_id);

            self.ready_sender.send(self.task_id).unwrap();
        }
    }

    unsafe fn from_ptr(p: *const ()) -> State {
        Arc::from_raw(p as *const StateInner)
    }

    unsafe fn clone(p: *const ()) -> RawWaker {
        let state = from_ptr(p);
        let ptr = Arc::into_raw(Arc::clone(&state)) as *const ();

        // Keep the state alive
        let _ = Arc::into_raw(state);

        RawWaker::new(ptr, &VTABLE)
    }

    unsafe fn wake(p: *const ()) {
        let state = from_ptr(p);
        state.wake();

        // State drops here
    }

    unsafe fn wake_by_ref(p: *const ()) {
        let state = from_ptr(p);
        state.wake();

        // Keep the state alive
        let _ = Arc::into_raw(state);
    }

    unsafe fn drop(p: *const ()) {
        from_ptr(p); // Dropped here
    }

    pub(crate) const VTABLE: RawWakerVTable = RawWakerVTable::new(clone, wake, wake_by_ref, drop);
}

#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub struct TaskId(pub usize);

impl TaskId {
    pub const NONE: Self = Self(0);

    fn next() -> TaskId {
        static NEXT: AtomicUsize = AtomicUsize::new(1);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

pub struct Task {
    id: TaskId,
    fut: Pin<Box<dyn Future<Output = ()> + Send + 'static>>,
    counter: Arc<()>,
}

fn worker(todo_recv: Receiver<Task>, done_send: Sender<Task>, ready_send: Sender<TaskId>) {
    for mut task in todo_recv.iter() {
        let waker = waker::StateInner::new(task.id, ready_send.clone()).into_waker();
        let mut ctx = Context::from_waker(&waker);

        let fut = task.fut.as_mut();

        match fut.poll(&mut ctx) {
            Poll::Ready(()) => {
                // done, drop the task
                if Arc::strong_count(&task.counter) == 2 {
                    // If this was the last task wake up main thread to make progress
                    ready_send.send(TaskId::NONE).unwrap()
                }
            }
            Poll::Pending => done_send.send(task).unwrap(),
        }
    }
}

pub struct RunTime {
    todo_sender: Sender<Task>,
    done_recv: Receiver<Task>,
    ready_recv: Receiver<TaskId>,
    workers: Vec<JoinHandle<()>>,
    idle_tasks: BTreeMap<TaskId, Task>,
    task_cnt: Arc<()>,
}

impl Default for RunTime {
    fn default() -> Self {
        Self::new()
    }
}

impl RunTime {
    pub fn new() -> Self {
        let (todo_sender, todo_recv) = flume::unbounded();
        let (done_sender, done_recv) = flume::unbounded();
        let (ready_sender, ready_recv) = flume::unbounded();

        let workers = (0..8)
            .map(|_| {
                let todo = todo_recv.clone();
                let done = done_sender.clone();
                let ready = ready_sender.clone();

                std::thread::spawn(move || worker(todo, done, ready))
            })
            .collect();

        Self {
            todo_sender,
            done_recv,
            ready_recv,
            workers,
            idle_tasks: Default::default(),
            task_cnt: Arc::new(()),
        }
    }

    pub fn poll(&mut self) {
        let next_ready = self.ready_recv.recv().unwrap();

        // Fill up our idle tasks
        self.idle_tasks
            .extend(self.done_recv.try_iter().map(|t| (t.id, t)));

        match self.idle_tasks.remove(&next_ready) {
            None if next_ready == TaskId::NONE => {}
            None => println!("Could not find task: {next_ready:?}"),
            Some(task) => self.todo_sender.send(task).unwrap(),
        }
    }

    pub fn spawner(&self) -> Spawner {
        Spawner {
            todo_sender: self.todo_sender.clone().downgrade(),
            counter: Arc::downgrade(&self.task_cnt),
        }
    }
    pub fn spawn(&mut self, fut: impl Future<Output = ()> + Send + 'static) {
        self.todo_sender
            .send(Task {
                id: TaskId::next(),
                fut: Box::pin(fut),
                counter: self.task_cnt.clone(),
            })
            .unwrap()
    }

    pub fn done(&self) -> bool {
        Arc::strong_count(&self.task_cnt) == 1
    }

    pub fn shutdown(self) {
        let Self {
            todo_sender,
            done_recv,
            ready_recv,
            workers,
            idle_tasks,
            task_cnt,
        } = self;

        // Signal there will be no more tasks
        drop(todo_sender);

        // Wait for workers to stop
        for worker in workers {
            worker.join().unwrap();
        }

        // Drop remaining tasks
        drop(done_recv);
        drop(ready_recv);
        drop(idle_tasks);

        assert_eq!(Arc::strong_count(&task_cnt), 1);
    }
}

pub struct RunTimeDead;

pub struct Spawner {
    todo_sender: WeakSender<Task>,
    counter: Weak<()>,
}

impl Spawner {
    pub fn spawn(&self, fut: impl Future<Output = ()> + Send + 'static) -> Result<(), RunTimeDead> {
        self.todo_sender
            .upgrade()
            .ok_or(RunTimeDead)?
            .send(Task {
                id: TaskId::next(),
                fut: Box::pin(fut),
                counter: self.counter.upgrade().ok_or(RunTimeDead)?,
            })
            .map_err(|_| RunTimeDead)
    }
}
