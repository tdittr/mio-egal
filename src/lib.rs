use crate::reactor::Reactor;
use flume::{Receiver, Sender, WeakSender};
use once_cell::sync::Lazy;
use std::cell::Cell;
use std::collections::BTreeMap;
use std::error::Error;
use std::fmt::{Debug, Display, Formatter};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Weak};
use std::task::{Context, Poll};
use std::thread::JoinHandle;

pub mod net;
pub(crate) mod reactor;
mod waker;

pub static GLOBAL_REACTOR: Lazy<Reactor> = Lazy::new(Reactor::spawn);

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

thread_local! {
    pub static CURRENT_TASK: Cell<TaskId> = const { Cell::new(TaskId::NONE) };
}

fn worker(todo_recv: Receiver<Task>, done_send: Sender<Task>, ready_send: Sender<TaskId>) {
    for mut task in todo_recv.iter() {
        let waker = waker::StateInner::new(task.id, ready_send.clone()).into_waker();
        let mut ctx = Context::from_waker(&waker);

        let fut = task.fut.as_mut();

        let old_task = CURRENT_TASK.replace(task.id);
        assert_eq!(old_task, TaskId::NONE);

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

        CURRENT_TASK.set(TaskId::NONE);
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
            .map(|i| {
                let todo = todo_recv.clone();
                let done = done_sender.clone();
                let ready = ready_sender.clone();

                std::thread::Builder::new()
                    .name(format!("worker{i}"))
                    .spawn(move || worker(todo, done, ready))
                    .unwrap()
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

#[derive(Debug)]
pub struct RunTimeDead;

impl Display for RunTimeDead {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("RunTimeDead").finish()
    }
}

impl Error for RunTimeDead {}

#[derive(Clone)]
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
