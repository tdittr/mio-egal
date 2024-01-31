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
