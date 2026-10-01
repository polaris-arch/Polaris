use crate::runtime::proxy::android_capacity::CapacityClosed;

#[test]
fn only_exact_typed_code_is_capacity() {
    assert_eq!(
        CapacityClosed::from_code(Some(
            super::super::code::ANDROID_NATIVE_LEDGER_CAPACITY_CLOSED
        )),
        Some(CapacityClosed)
    );
    for code in [
        None,
        Some("STARTUP_FAILED"),
        Some("ANDROID_NATIVE_ADMISSION_CLOSED"),
        Some("ANDROID_NATIVE_LEDGER_CAPACITY_CLOSED "),
        Some("完全关闭并重新启动应用"),
    ] {
        assert_eq!(CapacityClosed::from_code(code), None);
    }
}

#[test]
fn typed_spawn_source_survives_but_same_raw_message_does_not_classify() {
    assert_eq!(
        CapacityClosed::from_spawn(&CapacityClosed.spawn_error()),
        Some(CapacityClosed)
    );
    let text = CapacityClosed.to_string();
    let raw = polaris_core_supervisor::SpawnError::Spawn {
        bin: "android-libbox".into(),
        source: std::io::Error::other(text),
    };
    assert_eq!(CapacityClosed::from_spawn(&raw), None);
}
