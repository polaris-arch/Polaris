use super::*;

fn synthetic_post(url: &str) -> UnlockRequest {
    UnlockRequest::post(url)
        .header("Authorization", "Bearer synthetic-auth")
        .header("Proxy-Authorization", "Basic synthetic-proxy")
        .header("Cookie", "session=synthetic-cookie")
        .header("Content-Type", "application/json")
        .body("synthetic-body-token")
}

#[test]
fn redirect_method_origin_and_downgrade_matrix() {
    for status in [301, 302, 303, 307, 308] {
        let original = synthetic_post("https://service.example/device");
        let same = redirect_request(&original, status, "/next").unwrap();
        let preserves_post = matches!(status, 307 | 308);
        assert_eq!(same.method == HttpMethod::Post, preserves_post);
        assert_eq!(same.body.is_some(), preserves_post);
        assert!(same.headers.contains_key("Authorization"));
        let cross = redirect_request(&original, status, "https://other.example/next");
        if preserves_post {
            assert!(cross.is_err(), "cross-origin body replay must fail before dispatch");
        } else {
            let cross = cross.unwrap();
            assert_eq!(cross.method, HttpMethod::Get);
            assert!(cross.body.is_none());
            assert!(!cross.headers.keys().any(|key| matches!(key.to_ascii_lowercase().as_str(),
                "authorization" | "proxy-authorization" | "content-type")));
            assert_eq!(cross.headers.get("Cookie").map(String::as_str), Some(""));
        }
        assert!(redirect_request(&original, status, "http://service.example/next").is_err());
        assert!(redirect_request(&original, status, "file:///tmp/private").is_err());
        assert!(redirect_request(&original, status, "https://user:synthetic@other.example/next").is_err());
        let get = UnlockRequest::get("https://service.example/a");
        assert_eq!(redirect_request(&get, status, "/next").unwrap().method, HttpMethod::Get);
    }
    // Default-port equivalence is same-origin; an actual port change is cross-origin.
    let request = synthetic_post("https://service.example/device");
    assert!(redirect_request(&request, 307, "https://service.example:443/next").is_ok());
    assert!(redirect_request(&request, 307, "https://service.example:444/next").is_err());
}

// Capture complete request headers/body rather than assuming one read receives them all.
fn read_request(socket: &mut std::net::TcpStream) -> String {
    socket.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    let mut data = Vec::new();
    loop {
        let mut bytes = [0u8; 4096];
        let count = socket.read(&mut bytes).unwrap_or(0);
        if count == 0 { break; }
        data.extend_from_slice(&bytes[..count]);
        if let Some(end) = data.windows(4).position(|w| w == b"\r\n\r\n") {
            let headers = String::from_utf8_lossy(&data[..end]);
            let length = headers.lines().filter_map(|line| line.split_once(':'))
                .find(|(key, _)| key.eq_ignore_ascii_case("content-length"))
                .and_then(|(_, value)| value.trim().parse::<usize>().ok()).unwrap_or(0);
            if data.len() >= end + 4 + length { break; }
        }
    }
    String::from_utf8_lossy(&data).into_owned()
}

#[tokio::test]
async fn actual_redirect_hops_rewrite_post_and_strip_cross_origin_secrets() {
    for status in [301, 302, 303, 307, 308] {
        for same_origin in [true, false] {
            let first = TcpListener::bind("127.0.0.1:0").unwrap();
            let first_addr = first.local_addr().unwrap();
            let other = TcpListener::bind("127.0.0.1:0").unwrap();
            let target_addr = if same_origin { first_addr } else { other.local_addr().unwrap() };
            let (tx, rx) = std::sync::mpsc::channel();
            let reject = !same_origin && matches!(status, 307 | 308);
            let server = thread::spawn(move || {
                let (mut socket, _) = first.accept().unwrap();
                let _initial = read_request(&mut socket);
                let location = format!("http://{target_addr}/next");
                socket.write_all(&http_response(&format!("{status} Redirect"), &[("Location", &location), ("Set-Cookie", "jar=synthetic-jar; Path=/")], b"")).unwrap();
                drop(socket);
                let target = if same_origin { first } else { other };
                target.set_nonblocking(true).unwrap();
                let deadline = std::time::Instant::now() + Duration::from_secs(2);
                loop {
                    match target.accept() {
                        Ok((mut socket, _)) => {
                            let request = read_request(&mut socket);
                            let _ = socket.write_all(&http_response("200 OK", &[], b"ok"));
                            tx.send(Some(request)).unwrap();
                            return;
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            if std::time::Instant::now() >= deadline { tx.send(None).unwrap(); return; }
                            thread::sleep(Duration::from_millis(5));
                        }
                        Err(_) => { tx.send(None).unwrap(); return; }
                    }
                }
            });
            let client = UnlockClient::direct().unwrap();
            let response = client.request(&synthetic_post(&format!("http://{first_addr}/device"))).await;
            let captured = rx.recv_timeout(Duration::from_secs(3)).unwrap();
            server.join().unwrap();
            if reject {
                assert_eq!(response.status, 0);
                assert!(captured.is_none(), "rejected body must never reach target");
                continue;
            }
            assert_eq!(response.status, 200);
            let captured = captured.unwrap();
            assert_eq!(captured.starts_with("POST "), matches!(status, 307 | 308));
            assert_eq!(captured.contains("synthetic-body-token"), matches!(status, 307 | 308));
            if !same_origin {
                for secret in ["synthetic-auth", "synthetic-proxy", "synthetic-cookie", "synthetic-jar"] {
                    assert!(!captured.contains(secret), "credential sentinel reached a different origin");
                }
            }
        }
    }
}
