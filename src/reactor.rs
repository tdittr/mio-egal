use mio::Interest;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::Waker;
use std::thread::JoinHandle;

struct ReactorInner {
    poll: mio::Poll,
    wakers: Vec<Option<Waker>>,
}

impl ReactorInner {
    const WAKER_TOKEN: mio::Token = mio::Token(usize::MAX);

    fn run(reactor: Arc<Mutex<Self>>, want_poll: Arc<AtomicUsize>) {
        let mut events = mio::Events::with_capacity(1024);
        let mut guard = reactor.lock().expect("The poll will never be poisoned");

        loop {
            // Sleep until somebody wakes us up
            guard
                .poll
                .poll(&mut events, None)
                .expect("polling never fails");

            // Wake everybody who got an event
            for event in events.iter() {
                if event.token() == Self::WAKER_TOKEN {
                    println!(
                        "Somebody woke me to steal my lock! want_poll: {}",
                        want_poll.load(Ordering::Acquire)
                    );
                    continue;
                }

                let waker = guard
                    .wakers
                    .get(event.token().0)
                    .expect("Can only be woken by a token of an existing waker")
                    .as_ref()
                    .expect("Can only be woken by a token of an existing waker");

                waker.wake_by_ref();
            }

            // Check if anybody wants to register something
            while want_poll.load(Ordering::Acquire) != 0 {
                // Unlock the mutex
                drop(guard);

                // Try to wait for a bit
                std::thread::yield_now();

                // Try to lock it again, hoping that the registering thread now holds the lock
                guard = reactor.lock().expect("The poll will never be poisoned");
            }
        }
    }

    fn store_waker(&mut self, waker: Waker) -> mio::Token {
        let spot = self
            .wakers
            .iter()
            .position(|spot| spot.is_none())
            .unwrap_or_else(|| {
                self.wakers.push(None);
                self.wakers.len() - 1
            });
        if spot == usize::MAX {
            panic!("Too many tasks to await");
        }

        let old = self.wakers[spot].replace(waker);
        if old.is_some() {
            panic!("Found a waker in a free spot");
        }

        mio::Token(spot)
    }

    fn remove_waker(&mut self, token: mio::Token) {
        let old_waker = self.wakers[token.0].take();
        assert!(old_waker.is_some());
    }
}

#[derive(Clone)]
pub struct Reactor {
    reactor: Arc<Mutex<ReactorInner>>,
    waker: Arc<mio::Waker>,
    want_poll: Arc<AtomicUsize>,
    join_handle: Arc<JoinHandle<()>>,
}

impl Reactor {
    pub fn spawn() -> Reactor {
        let poll = mio::Poll::new().unwrap();
        let waker = mio::Waker::new(poll.registry(), ReactorInner::WAKER_TOKEN).unwrap();
        let want_poll = Arc::new(AtomicUsize::new(0));
        let reactor = Arc::new(Mutex::new(ReactorInner {
            poll,
            wakers: vec![],
        }));

        let join_handle = {
            let reactor = Arc::clone(&reactor);
            let want_poll = Arc::clone(&want_poll);
            std::thread::Builder::new()
                .name("reactor".to_string())
                .spawn(move || ReactorInner::run(reactor, want_poll))
                .unwrap()
        };

        Reactor {
            reactor,
            waker: Arc::new(waker),
            join_handle: Arc::new(join_handle),
            want_poll,
        }
    }

    fn with_lock<R>(&self, f: impl FnOnce(&mut ReactorInner) -> R) -> R {
        // Inform the reactor that it should wait for us to be done
        self.want_poll.fetch_add(1, Ordering::Acquire);

        // Wake the reactor via the OS so we can grab its lock
        self.waker.wake().expect("Waking never fails");

        // Grab the reactor and do whatever we must
        let mut reactor = self.reactor.lock().expect("Reactor never gets poisoned");
        let ret = f(&mut reactor);

        // Inform the reactor that it can continue
        self.want_poll.fetch_sub(1, Ordering::Release);

        ret
    }

    pub fn is_alive(&self) -> bool {
        !self.join_handle.is_finished()
    }

    pub fn register<S>(&self, source: &mut S, waker: Waker, interests: Interest) -> mio::Token
    where
        S: mio::event::Source,
    {
        self.with_lock(|reactor| {
            let token = reactor.store_waker(waker);
            reactor
                .poll
                .registry()
                .register(source, token, interests)
                .expect("Registering never fails");

            token
        })
    }

    pub fn deregister<S>(&self, source: &mut S, token: mio::Token)
    where
        S: mio::event::Source,
    {
        self.with_lock(|reactor| {
            reactor.remove_waker(token);
            reactor
                .poll
                .registry()
                .deregister(source)
                .expect("Registering never fails");
        });
    }
}
