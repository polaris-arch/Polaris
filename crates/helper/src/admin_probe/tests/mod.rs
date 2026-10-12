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
