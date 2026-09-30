//! Private native PC echo admission. JSON consistency is not root authority: only the
//! original-session plugin's authenticated root-driver challenge produces this input.
use super::android_probe_loan::DebugCoreProbeSessionScope;

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct PrivatePcReady {
    target: PcEchoTarget,
    request_id: String,
    deadline_elapsed: u64,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PcEchoTarget {
    schema: String,
    pc_run_id: String,
    pc_plan_sha256: String,
    ready_receipt_sha256: String,
    pc_candidate_sha: String,
    android_candidate_sha: String,
    receiver_source_sha256: String,
    android_package_sha256: String,
    receiver_instance_id: String,
    tcp_socket_instance_id: String,
    udp_socket_instance_id: String,
    #[serde(rename = "destinationIPv4")]
    destination_i_pv4: String,
    #[serde(rename = "expectedSenderIPv4")]
    expected_sender_i_pv4: String,
    tcp_port: u16,
    udp_port: u16,
    echo_nonce: String,
    echo_nonce_sha256: String,
    lifetime_seconds: u64,
    max_requests: u64,
}

impl Drop for PcEchoTarget {
    fn drop(&mut self) {
        // Rust String has no safe in-place byte mutator. Move its owned bytes and erase them;
        // no Debug/Clone/Serialize/public return exists for this private nonce type.
        let mut bytes = std::mem::take(&mut self.echo_nonce).into_bytes();
        bytes.fill(0);
    }
}

pub(super) struct AdmittedPcEchoAttempt {
    request_id: String,
    deadline_elapsed: u64,
}
fn hex(s: &str, min: usize, max: usize) -> bool {
    (min..=max).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn lan(s: &str) -> bool {
    let Ok(ip) = s.parse::<std::net::Ipv4Addr>() else {
        return false;
    };
    let a = ip.octets();
    s == ip.to_string() && ip.is_private() && (1..=254).contains(&a[3])
}
impl PrivatePcReady {
    /// Not exported through a Tauri command. The bridge is called before the live loan
    /// collector, which rechecks actual generation/birth/revision/digest/selector.
    pub(super) fn admit(
        self,
        scope: &DebugCoreProbeSessionScope,
    ) -> Result<AdmittedPcEchoAttempt, String> {
        let t = &self.target;
        let valid = t.schema == "polaris-pc-echo-target-v1"
            && hex(&t.pc_run_id, 32, 64)
            && [
                &t.pc_plan_sha256,
                &t.ready_receipt_sha256,
                &t.receiver_source_sha256,
                &t.android_package_sha256,
                &t.echo_nonce_sha256,
            ]
            .iter()
            .all(|s| hex(s, 64, 64))
            && [&t.pc_candidate_sha, &t.android_candidate_sha]
                .iter()
                .all(|s| hex(s, 40, 40))
            && [
                &t.receiver_instance_id,
                &t.tcp_socket_instance_id,
                &t.udp_socket_instance_id,
            ]
            .iter()
            .all(|s| hex(s, 32, 32))
            && t.tcp_socket_instance_id != t.udp_socket_instance_id
            && lan(&t.destination_i_pv4)
            && lan(&t.expected_sender_i_pv4)
            && t.tcp_port >= 49152
            && t.udp_port >= 49152
            && t.tcp_port != t.udp_port
            && hex(&t.echo_nonce, 32, 64)
            && polaris_updater::verify::sha256_hex(t.echo_nonce.as_bytes()) == t.echo_nonce_sha256
            && (1..=120).contains(&t.lifetime_seconds)
            && t.max_requests == 32
            && t.android_package_sha256 == scope.apk_sha256
            && hex(&self.request_id, 32, 32)
            && self.deadline_elapsed > scope.sampled_elapsed
            && self.deadline_elapsed <= scope.deadline_elapsed
            && self.deadline_elapsed - scope.sampled_elapsed <= 120000;
        if !valid {
            return Err("Current PC Ready unavailable".into());
        }
        Ok(AdmittedPcEchoAttempt {
            request_id: self.request_id,
            deadline_elapsed: self.deadline_elapsed,
        })
    }
}
impl AdmittedPcEchoAttempt {
    pub(super) fn request_id(&self) -> &str {
        &self.request_id
    }
    pub(super) fn deadline_elapsed(&self) -> u64 {
        self.deadline_elapsed
    }
}

#[cfg(test)]
mod tests {
    use crate::runtime::proxy::android_probe_loan::DebugCoreProbeSessionScope;
    use crate::runtime::proxy::debug_pc_echo::PrivatePcReady;
    fn scope() -> DebugCoreProbeSessionScope {
        DebugCoreProbeSessionScope {
            boot_nonce: "1".repeat(32),
            session_id: "2".repeat(32),
            nonce: "3".repeat(48),
            plan_sha256: "4".repeat(64),
            apk_sha256: "5".repeat(64),
            expected_source_pin: "6".repeat(64),
            run_id: "logic-only".into(),
            birth_nonce: "logic-birth".into(),
            revision: 1,
            config_digest: "7".repeat(64),
            deadline_elapsed: 9000,
            sampled_elapsed: 1000,
        }
    }
    fn fixture() -> serde_json::Value {
        serde_json::json!({"requestId":"8".repeat(32),"deadlineElapsed":2800,"target":{
            "schema":"polaris-pc-echo-target-v1","pcRunId":"1".repeat(32),"pcPlanSha256":"2".repeat(64),
            "readyReceiptSha256":"3".repeat(64),"pcCandidateSha":"4".repeat(40),"androidCandidateSha":"5".repeat(40),
            "receiverSourceSha256":"6".repeat(64),"androidPackageSha256":"5".repeat(64),"receiverInstanceId":"7".repeat(32),
            "tcpSocketInstanceId":"8".repeat(32),"udpSocketInstanceId":"9".repeat(32),"destinationIPv4":"192.168.1.1",
            "expectedSenderIPv4":"192.168.1.2","tcpPort":50001,"udpPort":50002,"echoNonce":"a".repeat(32),
            "echoNonceSha256":polaris_updater::verify::sha256_hex("a".repeat(32).as_bytes()),"lifetimeSeconds":120,"maxRequests":32}})
    }
    #[test]
    fn private_bridge_shape_is_closed_and_native_scope_bounds_the_original_deadline() {
        let valid: PrivatePcReady = serde_json::from_value(fixture()).unwrap();
        let attempt = valid.admit(&scope()).unwrap();
        assert_eq!(attempt.request_id(), "8".repeat(32));
        assert_eq!(attempt.deadline_elapsed(), 2800);
        for (key, value) in [
            ("approved", serde_json::json!(true)),
            ("deadlineElapsed", serde_json::json!(true)),
        ] {
            let mut v = fixture();
            v[key] = value;
            assert!(serde_json::from_value::<PrivatePcReady>(v).is_err());
        }
        for (key, value) in [
            ("tcpPort", serde_json::json!(true)),
            ("extra", serde_json::json!("authority")),
        ] {
            let mut v = fixture();
            v["target"][key] = value;
            assert!(serde_json::from_value::<PrivatePcReady>(v).is_err());
        }
        for (key, value) in [
            ("androidPackageSha256", serde_json::json!("0".repeat(64))),
            ("destinationIPv4", serde_json::json!("8.8.8.8")),
            ("udpPort", serde_json::json!(50001)),
            ("maxRequests", serde_json::json!(31)),
        ] {
            let mut v = fixture();
            v["target"][key] = value;
            assert!(serde_json::from_value::<PrivatePcReady>(v)
                .unwrap()
                .admit(&scope())
                .is_err());
        }
        for deadline in [1000, 9001, 122000] {
            let mut v = fixture();
            v["deadlineElapsed"] = serde_json::json!(deadline);
            assert!(serde_json::from_value::<PrivatePcReady>(v)
                .unwrap()
                .admit(&scope())
                .is_err());
        }
        let raw = serde_json::to_string(&fixture()).unwrap();
        let duplicate = raw.replacen("{", "{\"requestId\":\"copied\",", 1);
        assert!(serde_json::from_str::<PrivatePcReady>(&duplicate).is_err());
    }
}
