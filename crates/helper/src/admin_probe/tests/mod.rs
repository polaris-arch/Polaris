use super::*;
use polaris_helper_proto::{HelperBirthTarget, LinuxBirthStart, Start};

#[test]
fn only_exact_native_custody_empty_permits_service_replacement() {
    assert_eq!(
        classify(
            Platform::Linux,
            &Response::Ok(ResponseKind::LinuxBirthStatus(LinuxBirthStatus::Empty))
        ),
        Slot::Empty
    );
    assert_eq!(
        classify(
            Platform::Win,
            &Response::Ok(ResponseKind::NativeBirthStatus(NativeBirthStatus::Empty))
        ),
        Slot::Empty
    );
    for response in [
        Response::parse("OK stopped"),
        Response::parse("OK notrunning"),
        Response::parse("OK native-birth-status unknown"),
        Response::parse("ERR unknown"),
        Response::Ok(ResponseKind::Start(Start::Started { pid: 4 })),
    ] {
        assert_eq!(classify(Platform::Win, &response), Slot::Unknown);
        assert_eq!(classify(Platform::Linux, &response), Slot::Unknown);
    }
    assert_eq!(
        classify(
            Platform::Win,
            &Response::Ok(ResponseKind::LinuxBirthStatus(LinuxBirthStatus::Empty))
        ),
        Slot::Unknown
    );
}

#[test]
fn any_running_or_stopping_birth_keeps_the_installed_helper() {
    let target = HelperBirthTarget::parse_wire("4", "00112233445566778899aabbccddeeff").unwrap();
    assert_eq!(
        classify(
            Platform::Mac,
            &Response::Ok(ResponseKind::NativeBirthStatus(
                NativeBirthStatus::Stopping { target }
            ))
        ),
        Slot::Occupied
    );
    assert_eq!(
        classify(
            Platform::Linux,
            &Response::Ok(ResponseKind::LinuxBirthStatus(LinuxBirthStatus::Running {
                target
            }))
        ),
        Slot::Occupied
    );
    assert_eq!(
        classify(
            Platform::Linux,
            &Response::Ok(ResponseKind::LinuxBirthStart(LinuxBirthStart::Already {
                target
            }))
        ),
        Slot::Unknown
    );
}

#[test]
fn retained_live_content_receipt_requires_exact_id_and_compatible_protocol() {
    use polaris_helper_client::MockStream;
    let id = polaris_helper_proto::core_manifest::CoreId::parse(&"a".repeat(64)).unwrap();
    for (reply, accepted) in [
        (
            format!("OK core-present {} 1.15.0-alpha.12\n", id.as_str()),
            true,
        ),
        (
            format!("OK core-present {} 1.15.0-alpha.12\n", "b".repeat(64)),
            false,
        ),
        (format!("OK core-absent {}\n", id.as_str()), false),
        (format!("OK core-invalid {} changed\n", id.as_str()), false),
        ("ERR unknown\n".into(), false),
    ] {
        let client = HelperClient::new(
            Box::new(MockConnector::with_stream(MockStream::with_response(
                reply.into_bytes(),
            ))),
            Platform::Linux,
            "",
        );
        assert_eq!(
            retained_ready_with_client(&client, polaris_helper_proto::proto_version::CURRENT, &id)
                .is_ok(),
            accepted
        );
    }
    let client = HelperClient::new(
        Box::new(MockConnector::with_stream(MockStream::with_response(
            format!("OK core-present {} 1.15.0-alpha.12\n", id.as_str()).into_bytes(),
        ))),
        Platform::Linux,
        "",
    );
    assert!(retained_ready_with_client(
        &client,
        polaris_helper_proto::proto_version::CURRENT - 1,
        &id
    )
    .is_err());
}

struct MockConnector(std::sync::Mutex<Option<polaris_helper_client::MockStream>>);
impl MockConnector {
    fn with_stream(stream: polaris_helper_client::MockStream) -> Self {
        Self(std::sync::Mutex::new(Some(stream)))
    }
}
impl polaris_helper_client::Connector for MockConnector {
    fn connect(
        &self,
    ) -> Result<Box<dyn polaris_helper_client::ConnectionStream>, polaris_helper_client::ClientError>
    {
        self.0
            .lock()
            .unwrap()
            .take()
            .map(|stream| Box::new(stream) as Box<dyn polaris_helper_client::ConnectionStream>)
            .ok_or_else(|| polaris_helper_client::ClientError::Connect("fixture consumed".into()))
    }
}
