use super::super::*;
use std::{
    io::{BufReader, Write},
    time::{Duration, Instant},
};
#[test]
fn drip_feed_cannot_restart_the_connection_budget() {
    let (read, mut write) = std::os::unix::net::UnixStream::pair().unwrap();
    let producer = std::thread::spawn(move || {
        for _ in 0..30 {
            if write.write_all(b"x").is_err() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    });
    let start = Instant::now();
    let mut reader = BufReader::new(DeadlineStream::new(read, Duration::from_millis(60)));
    assert!(read_line_terminated_bounded(&mut reader, 1000).is_err());
    assert!(start.elapsed() < Duration::from_millis(250));
    drop(reader);
    producer.join().unwrap();
}
