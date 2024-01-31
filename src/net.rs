use crate::GLOBAL_REACTOR;
use mio::Interest;
use std::future::Future;
use std::io::{ErrorKind, Read};
use std::task::Poll;

pub struct TcpStroom {
    inner: mio::net::TcpStream,
    token: Option<mio::Token>,
}

impl TcpStroom {
    pub fn from_std(socket: std::net::TcpStream) -> Self {
        socket.set_nonblocking(true).unwrap();
        Self {
            inner: mio::net::TcpStream::from_std(socket),
            token: None,
        }
    }

    pub fn read<'a>(
        &'a mut self,
        buf: &'a mut [u8],
    ) -> impl Future<Output = std::io::Result<usize>> + Send + 'a {
        std::future::poll_fn(|ctx| {
            let result = match self.inner.read(buf) {
                Ok(n) => Poll::Ready(Ok(n)),
                Err(e) if e.kind() == ErrorKind::WouldBlock => Poll::Pending,
                Err(e) => Poll::Ready(Err(e)),
            };

            dbg!(&result);

            match (&result, self.token) {
                (Poll::Ready(_), Some(token)) => {
                    // De register ourselves as we are done waiting
                    GLOBAL_REACTOR.deregister(&mut self.inner, token);
                    self.token.take();
                }
                (Poll::Pending, None) => {
                    // Reading did not work so inform reactor about our desire to be woken up again
                    let token = GLOBAL_REACTOR.register(
                        &mut self.inner,
                        ctx.waker().clone(),
                        Interest::READABLE,
                    );

                    self.token = Some(token);
                }
                (Poll::Ready(_), None) | (Poll::Pending, Some(_)) => { /* everything okay */ }
            }

            // Now lets take a nap
            result
        })
    }
}
