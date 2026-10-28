use super::read_probe_status;
use std::time::Duration;
use tokio::io::AsyncWriteExt;

#[tokio::test]
async fn complete_line_required_across_fragments() {
    let (mut writer, mut reader) = tokio::io::duplex(16);
    tokio::spawn(async move {
        for fragment in ["HTTP/1.", "1 2", "04 No Content\r", "\n"] {
            writer.write_all(fragment.as_bytes()).await.unwrap();
            tokio::task::yield_now().await;
        }
    });
    assert_eq!(read_probe_status(&mut reader).await, Some(true));
    let mut truncated = &b"HTTP/1.1 204"[..];
    assert_eq!(read_probe_status(&mut truncated).await, None);
    let mut rejected = &b"HTTP/1.1 407 Authentication Required\r\n"[..];
    assert_eq!(read_probe_status(&mut rejected).await, Some(false));
    let oversized = vec![b'x'; 1025];
    assert_eq!(read_probe_status(&mut oversized.as_slice()).await, None);
}

#[tokio::test]
async fn incomplete_slow_stream_obeys_one_outer_deadline() {
    let (mut writer, mut reader) = tokio::io::duplex(16);
    writer.write_all(b"HTTP/1.1 2").await.unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(5), read_probe_status(&mut reader))
            .await
            .is_err()
    );
    drop(writer);
}
