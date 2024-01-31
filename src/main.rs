use mio_egal::net::TcpStroom;
use mio_egal::{RunTime, CURRENT_TASK};
use std::time::Duration;

fn main() {
    let mut rt = RunTime::new();
    let spawner = rt.spawner();

    let stream = std::net::TcpStream::connect("127.0.0.1:1337").unwrap();
    let mut stroom = TcpStroom::from_std(stream);

    spawner
        .spawn(async move {
            let mut buf = [0u8; 1024];
            while let Ok(n) = stroom.read(&mut buf).await {
                let data = &buf[..n];
                let pretty_data = String::from_utf8_lossy(data);
                println!("Received: {pretty_data:?}");

                if n == 0 {
                    break;
                }
            }

            println!("Done listening...");
        })
        .unwrap();

    let (tx, rx) = flume::unbounded();

    for _ in 0..2 {
        let rx = rx.clone();
        rt.spawn(async move {
            println!(
                "({:?}, {:?}) Hello",
                std::thread::current().id(),
                CURRENT_TASK.get()
            );

            while let Ok(msg) = rx.recv_async().await {
                println!(
                    "({:?}) {} Received: {msg}",
                    std::thread::current().id(),
                    CURRENT_TASK.get().0
                );
            }
        });
    }

    spawner
        .clone()
        .spawn(async move {
            for i in 0..3 {
                spawner
                    .spawn(async move {
                        println!(
                            "({:?}, {:?}) Hoi {i}",
                            std::thread::current().id(),
                            CURRENT_TASK.get()
                        );
                    })
                    .unwrap();
            }
        })
        .unwrap();

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
    assert!(mio_egal::GLOBAL_REACTOR.is_alive());
}
