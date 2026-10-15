use super::*;
const CA: &str = concat!(
    "3082019f30820144a003020102020101300a06082a8648ce3d04030230353133303106035504030c2a506f6c61726973",
    "206d6561737572656d656e7420746573742043412c206e6576657220696e7374616c6c301e170d323430313031303030",
    "3030305a170d3330303130313030303030305a30353133303106035504030c2a506f6c61726973206d6561737572656d",
    "656e7420746573742043412c206e6576657220696e7374616c6c3059301306072a8648ce3d020106082a8648ce3d0301",
    "0703420004d65604ffe5188b1d1ca32639762dde09786c6633bd12db49e6420b481c3fdbba859a42a2483f26cfaf1e0e",
    "32913ca5189568ddf389788e6557eb916ee5d11a2aa345304330120603551d130101ff040830060101ff020100300e06",
    "03551d0f0101ff040403020106301d0603551d0e0416041403bd8b677716e9cf5ca09da1fdaaa0ffd04f9e17300a0608",
    "2a8648ce3d0403020349003046022100df890ee3eaaaac71e7125b2f10ea8b295cd288f1baba1335f272316bad7ecc83",
    "022100adb23ff6c26243c020bffc4f6c908867a037a6e5209bf4f66c2999c1a9246611",
);
const LEAF: &str = concat!(
    "308201d73082017da003020102020102300a06082a8648ce3d04030230353133303106035504030c2a506f6c61726973",
    "206d6561737572656d656e7420746573742043412c206e6576657220696e7374616c6c301e170d323630313031303030",
    "3030305a170d3237303130313030303030305a301d311b301906035504030c1273706565642e6578616d706c652e7465",
    "73743059301306072a8648ce3d020106082a8648ce3d030107034200049f908186618a2ebb2348bf7218c368adc8ca2f",
    "f83736c83a726b1790e267777a678a3e498edd210923c6649c43446afbd7f9a68b402d88bd0943c6185b3627fca38195",
    "308192300c0603551d130101ff04023000300e0603551d0f0101ff040403020780301d0603551d110416301482127370",
    "6565642e6578616d706c652e7465737430130603551d25040c300a06082b06010505070301301d0603551d0e04160414",
    "298effd1fe9189e0c581a2f22f3ab726ebcb38d1301f0603551d2304183016801403bd8b677716e9cf5ca09da1fdaaa0",
    "ffd04f9e17300a06082a8648ce3d0403020348003045022025f7714ba0d9ecf6a3388d2f89960305abfa3bd64f6f4d97",
    "2f64133dfdfa237b022100bdea0570115b5c0e5dac83793c1b6f924b9cb0f0cfc5577a3c48fd2adab21664",
);
const SELF_SIGNED: &str = concat!(
    "308201bf30820165a003020102020103300a06082a8648ce3d040302301d311b301906035504030c1273706565642e65",
    "78616d706c652e74657374301e170d3236303130313030303030305a170d3237303130313030303030305a301d311b30",
    "1906035504030c1273706565642e6578616d706c652e746573743059301306072a8648ce3d020106082a8648ce3d0301",
    "07034200049f908186618a2ebb2348bf7218c368adc8ca2ff83736c83a726b1790e267777a678a3e498edd210923c664",
    "9c43446afbd7f9a68b402d88bd0943c6185b3627fca38195308192300c0603551d130101ff04023000300e0603551d0f",
    "0101ff040403020780301d0603551d1104163014821273706565642e6578616d706c652e7465737430130603551d2504",
    "0c300a06082b06010505070301301d0603551d0e04160414298effd1fe9189e0c581a2f22f3ab726ebcb38d1301f0603",
    "551d23041830168014298effd1fe9189e0c581a2f22f3ab726ebcb38d1300a06082a8648ce3d04030203480030450221",
    "00edb22c3e12f6e92e0394f248bd788c6e2ad2085ec3f14da5739fcea0186bc24e02201a4869127474f8faa5e85f1491",
    "d741df4b52f8282caabd6ae4a09cec62fd5e46",
);
const EXPIRED: &str = concat!(
    "308201d73082017da003020102020104300a06082a8648ce3d04030230353133303106035504030c2a506f6c61726973",
    "206d6561737572656d656e7420746573742043412c206e6576657220696e7374616c6c301e170d323430313031303030",
    "3030305a170d3235303130313030303030305a301d311b301906035504030c1273706565642e6578616d706c652e7465",
    "73743059301306072a8648ce3d020106082a8648ce3d030107034200049f908186618a2ebb2348bf7218c368adc8ca2f",
    "f83736c83a726b1790e267777a678a3e498edd210923c6649c43446afbd7f9a68b402d88bd0943c6185b3627fca38195",
    "308192300c0603551d130101ff04023000300e0603551d0f0101ff040403020780301d0603551d110416301482127370",
    "6565642e6578616d706c652e7465737430130603551d25040c300a06082b06010505070301301d0603551d0e04160414",
    "298effd1fe9189e0c581a2f22f3ab726ebcb38d1301f0603551d2304183016801403bd8b677716e9cf5ca09da1fdaaa0",
    "ffd04f9e17300a06082a8648ce3d0403020348003045022042f1c2493b7e2faacdf09f9ff1205f5cb475d28d2abadd9f",
    "15c1271904b1983a022100fcdf67b9b224f7e498cb814ce37b12f83bbdc7d0b859aed7368920610d4ec2da",
);
const KEY: &str = concat!(
    "308187020100301306072a8648ce3d020106082a8648ce3d030107046d306b0201010420165801d46060b926e7e32204",
    "dc0e681ef05a55ce4a1522383cef8a2ce1fa05afa144034200049f908186618a2ebb2348bf7218c368adc8ca2ff83736",
    "c83a726b1790e267777a678a3e498edd210923c6649c43446afbd7f9a68b402d88bd0943c6185b3627fc",
);

fn der(hex: &str) -> Vec<u8> {
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
        .collect()
}

// Only independently owned test configurations use this clock. Production's
// OnceLock and platform trust configuration retain their real time provider.
const FIXED_NOW: u64 = 1_780_272_000; // 2026-06-01T00:00:00Z

#[derive(Debug)]
struct FixedTimeProvider;

impl rustls::time_provider::TimeProvider for FixedTimeProvider {
    fn current_time(&self) -> Option<rustls::pki_types::UnixTime> {
        Some(fixture_now())
    }
}

fn fixture_now() -> rustls::pki_types::UnixTime {
    rustls::pki_types::UnixTime::since_unix_epoch(Duration::from_secs(FIXED_NOW))
}

fn trusted_verifier() -> rustls_platform_verifier::Verifier {
    rustls_platform_verifier::Verifier::new_with_extra_roots(
        [rustls::pki_types::CertificateDer::from(der(CA))],
        Arc::new(rustls::crypto::ring::default_provider()),
    )
    .expect("offline fixture CA")
}

fn verify_case(
    verifier: &rustls_platform_verifier::Verifier,
    cert: &str,
    host: &str,
    case: &str,
    accepted: bool,
) {
    use rustls::client::danger::ServerCertVerifier;
    let result = verifier.verify_server_cert(
        &rustls::pki_types::CertificateDer::from(der(cert)),
        &[],
        &rustls::pki_types::ServerName::try_from(host).unwrap(),
        &[],
        fixture_now(),
    );
    assert_eq!(
        result.is_ok(),
        accepted,
        "case={case} now={FIXED_NOW} certificate_error={:?}",
        result.err()
    );
}

fn fixed_config(verifier: rustls_platform_verifier::Verifier) -> Arc<rustls::ClientConfig> {
    let config =
        verified_tls_client_config(Arc::new(rustls::crypto::ring::default_provider()), verifier)
            .expect("verified TLS configuration");
    let mut owned = config.as_ref().clone();
    owned.time_provider = Arc::new(FixedTimeProvider);
    Arc::new(owned)
}

// Diagnostics deliberately contain only the stage, tool error and GET count.
// Request path/query and proxy credentials never enter the returned error.
type OriginFailure = (&'static str, String, usize);
type OriginResult = Result<usize, OriginFailure>;

fn origin_error(phase: &'static str, error: impl std::fmt::Display, gets: usize) -> OriginFailure {
    (phase, error.to_string(), gets)
}

async fn tls_origin(
    cert: &str,
) -> (
    u16,
    Arc<std::sync::Mutex<Vec<String>>>,
    tokio::task::JoinHandle<OriginResult>,
) {
    use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let server = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(
            vec![CertificateDer::from(der(cert))],
            PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(der(KEY))),
        )
        .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let requests = Arc::new(std::sync::Mutex::new(Vec::new()));
    let observed = requests.clone();
    let task = tokio::spawn(async move {
        let (mut socket, _) = listener
            .accept()
            .await
            .map_err(|e| origin_error("proxy-accept", e, 0))?;
        let mut header = Vec::new();
        let mut byte = [0u8; 1];
        while !header.ends_with(b"\r\n\r\n") {
            socket
                .read_exact(&mut byte)
                .await
                .map_err(|e| origin_error("connect-read", e, 0))?;
            header.push(byte[0]);
            if header.len() > MAX_HEAD_BYTES {
                return Err(origin_error("connect-read", "header limit", 0));
            }
        }
        if String::from_utf8_lossy(&header).contains("fixture-token") {
            return Err(origin_error("connect-authority", "query disclosed", 0));
        }
        socket
            .write_all(OK_204.as_bytes())
            .await
            .map_err(|e| origin_error("connect-write", e, 0))?;
        let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(server));
        let mut tls = acceptor
            .accept(socket)
            .await
            .map_err(|e| origin_error("tls-accept", e, 0))?;
        for gets in 0..2 {
            header.clear();
            while !header.ends_with(b"\r\n\r\n") {
                tls.read_exact(&mut byte)
                    .await
                    .map_err(|e| origin_error("get-read", e, gets))?;
                header.push(byte[0]);
                if header.len() > MAX_HEAD_BYTES {
                    return Err(origin_error("get-read", "header limit", gets));
                }
            }
            observed.lock().unwrap().push(
                String::from_utf8(header.clone())
                    .map_err(|e| origin_error("get-decode", e, gets))?,
            );
            tls.write_all(b"HTTP/1.1 204 No Content\r\n\r\n")
                .await
                .map_err(|e| origin_error("get-write", e, gets + 1))?;
        }
        Ok(2)
    });
    (port, requests, task)
}

async fn finish(mut task: tokio::task::JoinHandle<OriginResult>) -> OriginResult {
    match tokio::time::timeout(Duration::from_secs(5), &mut task).await {
        Ok(Ok(origin)) => origin,
        Ok(Err(error)) => Err(origin_error("task-join", error, 0)),
        Err(error) => {
            task.abort();
            let _ = task.await;
            Err(origin_error("task-deadline", error, 0))
        }
    }
}

fn tunnel_result_label(
    result: &Result<Result<SpeedTestTunnel, TunnelError>, tokio::time::error::Elapsed>,
) -> String {
    match result {
        Ok(Ok(_)) => "accepted".to_owned(),
        Ok(Err(error)) => format!("{error:?}"),
        Err(error) => format!("deadline: {error}"),
    }
}

#[tokio::test]
async fn trusted_origin_receives_the_complete_authenticated_query() {
    let verifier = trusted_verifier();
    verify_case(&verifier, LEAF, "speed.example.test", "trusted", true);
    let config = fixed_config(verifier);
    let (port, requests, task) = tls_origin(LEAF).await;
    let target = SpeedTestTarget::parse(
        "https://speed.example.test/private?signature=fixture-token%2Bvalue",
    )
    .unwrap();
    let result = crate::commands::speedtest::measure_warm_ttfb(
        Duration::from_secs(5),
        Duration::from_secs(5),
        open_tunnel_with_tls_config(port, None, &target, &|_| {}, move || Some(config)),
    )
    .await;
    let origin = finish(task).await;
    let requests = requests.lock().unwrap();
    assert!(
        result.is_ok() && origin == Ok(2),
        "case=trusted now={FIXED_NOW} measurement={result:?}, observed_gets={}, origin={origin:?}",
        requests.len()
    );
    assert_eq!(requests.len(), 2);
    for request in requests.iter() {
        assert!(request.starts_with("GET /private?signature=fixture-token%2Bvalue HTTP/1.1\r\n"));
        assert!(!request.to_ascii_lowercase().contains("proxy-authorization"));
    }
}

#[tokio::test]
async fn wrong_hostname_self_signed_and_expired_certificates_receive_no_get() {
    for (case, cert, host) in [
        ("wrong-hostname", LEAF, "wrong.example.test"),
        ("self-signed", SELF_SIGNED, "speed.example.test"),
        ("expired", EXPIRED, "speed.example.test"),
    ] {
        let verifier = trusted_verifier();
        verify_case(&verifier, cert, host, case, false);
        let config = fixed_config(verifier);
        let (port, requests, task) = tls_origin(cert).await;
        let target =
            SpeedTestTarget::parse(&format!("https://{host}/private?signature=fixture-token"))
                .unwrap();
        let result = tokio::time::timeout(
            Duration::from_secs(5),
            open_tunnel_with_tls_config(port, None, &target, &|_| {}, move || Some(config)),
        )
        .await;
        let origin = finish(task).await;
        let gets = requests.lock().unwrap().len();
        assert!(
            matches!(&result, Ok(Err(TunnelError::Transport)))
                && matches!(&origin, Err(("tls-accept", _, 0)))
                && gets == 0,
            "case={case} now={FIXED_NOW} tunnel={}, observed_gets={gets}, origin={origin:?}",
            tunnel_result_label(&result)
        );
    }
}

#[test]
fn default_trust_rejects_a_valid_leaf_at_fixed_time() {
    // Establish validity independently so an expired fixture cannot make the
    // uninstalled-root negative control pass for the wrong reason in the future.
    verify_case(
        &trusted_verifier(),
        LEAF,
        "speed.example.test",
        "valid-leaf",
        true,
    );
    let verifier =
        rustls_platform_verifier::Verifier::new(Arc::new(rustls::crypto::ring::default_provider()))
            .expect("default platform verifier");
    verify_case(
        &verifier,
        LEAF,
        "speed.example.test",
        "uninstalled-root",
        false,
    );
}

#[tokio::test]
async fn production_trust_store_rejects_the_uninstalled_test_ca_before_get() {
    let (port, requests, task) = tls_origin(LEAF).await;
    let target =
        SpeedTestTarget::parse("https://speed.example.test/private?signature=fixture-token")
            .unwrap();
    let result = tokio::time::timeout(
        Duration::from_secs(5),
        open_tunnel(port, None, &target, &|_| {}),
    )
    .await;
    let origin = finish(task).await;
    let gets = requests.lock().unwrap().len();
    assert!(
        matches!(&result, Ok(Err(TunnelError::Transport)))
            && matches!(&origin, Err(("tls-accept", _, 0)))
            && gets == 0,
        "case=production-uninstalled-root tunnel={}, observed_gets={gets}, origin={origin:?}",
        tunnel_result_label(&result)
    );
}
