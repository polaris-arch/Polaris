use super::*;

#[test]
fn global_utls_change_creates_runtime_debt_and_missing_restores_follow_projection() {
    let config = UserConfig::default();
    let applied = serde_json::to_value(config).unwrap();
    let mut disk = applied.clone();
    disk["utlsPolicy"] = "random".into();
    assert_eq!(
        non_node_runtime_debt(&applied, &disk, Platform::Linux),
        Some(true)
    );
    disk["utlsPolicy"] = "follow".into();
    assert_eq!(
        non_node_runtime_debt(&applied, &disk, Platform::Linux),
        Some(false)
    );
    disk["utlsPolicy"] = "default".into();
    assert_eq!(
        non_node_runtime_debt(&applied, &disk, Platform::Linux),
        Some(true)
    );
    disk.as_object_mut().unwrap().remove("utlsPolicy");
    assert_eq!(
        non_node_runtime_debt(&applied, &disk, Platform::Linux),
        Some(false)
    );
}

#[test]
fn host_dns_debt_is_effective_platform_state_not_a_typed_dns_change() {
    let mut applied = serde_json::to_value(UserConfig::default()).unwrap();
    applied["proxyModeType"] = "tun".into();
    applied["dnsConfig"] = serde_json::json!({"takeoverSystemDns": true});
    let mut disk = applied.clone();
    disk["dnsConfig"]["takeoverSystemDns"] = false.into();
    for platform in [Platform::Mac, Platform::Linux] {
        assert_eq!(non_node_runtime_debt(&applied, &disk, platform), Some(true));
        assert_eq!(non_node_runtime_debt(&disk, &applied, platform), Some(true));
        assert_eq!(
            non_node_runtime_debt(&applied, &applied, platform),
            Some(false)
        );
    }
    for platform in [
        Platform::Win,
        Platform::Android,
        Platform::Ios,
        Platform::Other,
    ] {
        assert_eq!(
            non_node_runtime_debt(&applied, &disk, platform),
            Some(false)
        );
    }
    disk["dnsConfig"]
        .as_object_mut()
        .unwrap()
        .remove("takeoverSystemDns");
    assert_eq!(
        non_node_runtime_debt(&applied, &disk, Platform::Mac),
        Some(false)
    );
}
