use super::super::*;
use std::io::Write;
#[test]
fn late_newline_is_not_accepted() {
    let (stream, mut writer) = std::os::unix::net::UnixStream::pair().unwrap();
    let producer = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(40));
        let _ = writer.write_all(b"x");
        std::thread::sleep(Duration::from_millis(60));
        let _ = writer.write_all(b"\n");
    });
    let mut conn = UnixConnStream {
        stream,
        read_timeout: Duration::from_millis(60),
    };
    assert_eq!(
        conn.read_until_timeout(&mut Vec::new()).unwrap_err().kind(),
        io::ErrorKind::TimedOut
    );
    producer.join().unwrap();
}
