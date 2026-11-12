use super::*;

#[cfg(target_os = "macos")]
#[test]
fn system_client_rejects_frame_injection_without_connecting() {
    for bad in ["", "token\nstatus", "token\rstatus"] {
        assert!(SystemMacCheckClient::system(bad.into()).is_err());
    }
    // Construction is lazy and gives no Root completion. There is deliberately
    // no constructor accepting our Queue, MockStream or a generic HelperClient.
    assert!(SystemMacCheckClient::system("bounded-auth-data".into()).is_ok());
}

#[cfg(windows)]
#[test]
fn closed_system_windows_client_only_accepts_bounded_authentication_data() {
    for token in ["", "line\nsecond", "line\rsecond"] {
        assert!(SystemWindowsCheckClient::system(token.into()).is_err());
    }
    // Constructing the connector does not connect, issue authority or retire custody.
    assert!(SystemWindowsCheckClient::system("bounded-auth-data".into()).is_ok());
}
