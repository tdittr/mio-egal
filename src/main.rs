use mio_egal::RunTime;
use std::time::Duration;

fn main() {
    let mut rt = RunTime::new();

    let (tx, rx) = flume::unbounded();

    rt.spawn(async move {
        println!("({:?}) Hello", std::thread::current().id());

        while let Ok(msg) = rx.recv_async().await {
            println!("({:?}) Received: {msg}", std::thread::current().id());
        }
    });

    rt.spawn(async move {
        println!("({:?}) Hoi", std::thread::current().id());
    });

    std::thread::spawn(move || {
        for i in 0..10 {
            tx.send(i).unwrap();
            std::thread::sleep(Duration::from_secs(1));
        }
    });

    while !rt.done() {
        rt.poll();
    }

    rt.shutdown();
}
