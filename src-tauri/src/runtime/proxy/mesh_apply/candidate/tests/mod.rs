#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
use super::check::strict_check_pinned_linux_candidate;
use super::*;
use crate::runtime::config::ConfigManager;
#[cfg(unix)]
use crate::runtime::proxy::DirectCoreRun;
use crate::runtime::proxy::NoNetworkDoh;
use crate::runtime::{HelperRuntime, MeshRuntime};
use crate::test_support::TestDir;
use polaris_config_engine::builder::managed_mesh_plan::compile_managed_mesh_plan;
use polaris_store::mesh_guard::{POLICY_KEY, REQUIRED_MARKER_FILE, STATE_KEY};
use serde_json::{json, Value};
use std::fs;
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
use std::net::{Ipv4Addr, TcpListener};
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
use std::path::PathBuf;
use std::sync::Arc;

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
fn pinned_core_for_test() -> Option<PathBuf> {
    match std::env::var_os("POLARIS_TEST_CORE") {
        Some(path) => Some(PathBuf::from(path)),
        None if std::env::var_os("POLARIS_REQUIRE_KERNEL_GATE").is_some() => {
            panic!("POLARIS_TEST_CORE required for real pinned strict-check test")
        }
        None => None,
    }
}

fn fixture_with(
    mutate_raw: impl FnOnce(&mut Value),
) -> (
    TestDir,
    Arc<ProxyRuntime>,
    ApplyInputSnapshot,
    ManagedMeshPlanInput,
    ManagedMeshRoutePlan,
) {
    let dir = TestDir::new("polaris-mesh-candidate-");
    let golden: Value = serde_json::from_str(&crate::test_support::repo_file(
        "crates/config-engine/fixtures/config-snapshot.json",
    ))
    .unwrap();
    let mut raw = golden["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["name"] == "update-in 端口注入（direct）")
        .unwrap()["input"]
        .clone();
    let wire: Value = serde_json::from_str(&crate::test_support::repo_file(
        "ui/src/contracts/mesh-route-state.fixture.json",
    ))
    .unwrap();
    raw[POLICY_KEY] = wire[POLICY_KEY].clone();
    raw[POLICY_KEY]["candidateOrder"] = json!([]);
    raw[POLICY_KEY]["assignments"] = json!([]);
    raw[STATE_KEY] = wire[STATE_KEY].clone();
    raw[STATE_KEY]["revision"] = json!("1");
    raw[STATE_KEY]["identities"] = json!([]);
    raw[STATE_KEY]["observations"] = json!([]);
    raw[STATE_KEY]["reservations"] = json!([]);
    raw[STATE_KEY]["identityEffects"] = json!([]);
    raw[STATE_KEY]["intent"]["desiredRun"] = json!("running");
    mutate_raw(&mut raw);
    fs::write(dir.join("config.json"), serde_json::to_vec(&raw).unwrap()).unwrap();
    fs::write(
        dir.join(REQUIRED_MARKER_FILE),
        serde_json::to_vec(&json!({
            "phase":"enabled",
            "localId":raw[STATE_KEY]["localId"],
            "legacyConfigDigest":"0".repeat(64)
        }))
        .unwrap(),
    )
    .unwrap();
    let config = Arc::new(ConfigManager::new(dir.path().to_path_buf()));
    let helper = Arc::new(HelperRuntime::never_installed_for_tests(
        dir.path().to_path_buf(),
    ));
    let mesh = Arc::new(MeshRuntime::new(dir.path().to_path_buf()));
    let clearer = polaris_system_integration::production_proxy_controller(
        dir.join(polaris_system_integration::PROXY_MARKER_FILENAME)
            .to_string_lossy()
            .into_owned(),
    );
    let runtime = Arc::new(ProxyRuntime::new(
        Arc::clone(&config),
        helper,
        mesh,
        Box::new(clearer),
        Arc::new(NoNetworkDoh),
    ));
    let snapshot = config.read_mesh_apply_snapshot().unwrap();
    let input = ManagedMeshPlanInput {
        plan_id: "candidate-direct-vless".into(),
        config_version: snapshot.config_version().into(),
        policy: snapshot.policy().clone(),
        state: snapshot.state().clone(),
        candidates: vec![],
        scopeable_rule_matchers: BTreeMap::new(),
    };
    let plan = compile_managed_mesh_plan(input.clone()).unwrap();
    (dir, runtime, snapshot, input, plan)
}

fn fixture() -> (
    TestDir,
    Arc<ProxyRuntime>,
    ApplyInputSnapshot,
    ManagedMeshPlanInput,
    ManagedMeshRoutePlan,
) {
    fixture_with(|_| {})
}

#[test]
fn stored_system_proxy_is_rejected_when_mobile_effective_mode_is_tun() {
    let (dir, mut runtime, snapshot, input, plan) = fixture();
    let parsed: UserConfig = serde_json::from_value(snapshot.raw().clone()).unwrap();
    assert_eq!(parsed.proxy_mode_type, ProxyModeType::SystemProxy);
    assert!(conservative_user_input(&parsed, Platform::Linux));
    for platform in [Platform::Android, Platform::Ios] {
        assert_eq!(
            parsed.proxy_mode_type.effective_on(platform),
            ProxyModeType::Tun
        );
        assert!(!conservative_user_input(&parsed, platform));
        Arc::get_mut(&mut runtime).unwrap().helper = Arc::new(
            HelperRuntime::with_platform_for_tests(dir.path().to_path_buf(), platform),
        );
        assert!(matches!(
            generate_direct_vless_candidate(&runtime, &snapshot, &input, &plan),
            Err(CandidateError::Unsupported)
        ));
        assert!(!dir.join("mesh-apply").exists());
    }
}

#[tokio::test]
async fn full_builder_candidate_seals_direct_vless_and_preserves_old_resources() {
    let (dir, runtime, snapshot, input, plan) = fixture();
    let parsed: UserConfig = serde_json::from_value(snapshot.raw().clone()).unwrap();
    assert!(conservative_user_input(&parsed, runtime.helper.platform()));
    assert!(conservative_raw_server(snapshot.raw()));
    let cache_path = dir.join("cache.db");
    let runtime_config_path = dir.join("singbox-runtime.json");
    fs::write(&cache_path, b"persistent selected and dns cache").unwrap();
    fs::write(&runtime_config_path, b"old runtime config").unwrap();
    let gate_before = runtime.gate.generation();
    let sidecar_before = runtime.dns_race.config_projection();
    #[cfg(unix)]
    {
        let child = tokio::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .unwrap();
        runtime
            .child
            .lock()
            .unwrap()
            .install_running_for_test(DirectCoreRun::new(child));
    }

    let candidate = generate_direct_vless_candidate(&runtime, &snapshot, &input, &plan).unwrap();
    let exact_bytes = serde_json::to_vec_pretty(&candidate.materialized.emission.config).unwrap();
    assert_eq!(candidate.materialized.closure.config_bytes, exact_bytes);
    assert_eq!(
        candidate.facts.final_config_sha256,
        polaris_updater::verify::sha256_hex(&exact_bytes)
    );
    assert_eq!(
        candidate.facts.raw_document_sha256,
        snapshot.raw_document_sha256()
    );
    assert_eq!(candidate.facts.plan_digest, plan_digest(&plan).unwrap());
    assert_eq!(candidate.facts.cache.path, cache_path);
    assert_eq!(
        candidate.facts.cache.ownership,
        CacheOwnership::PersistentMutablePreserve
    );
    assert!(candidate.facts.cache.enabled);
    assert_eq!(candidate.facts.cache.store_fakeip, Some(true));
    assert_eq!(candidate.facts.cache.store_dns, Some(true));
    assert_eq!(
        candidate.facts.cache.cache_id.as_deref(),
        Some("polaris-dns-v2")
    );
    assert_eq!(
        candidate.facts.cache.selected_policy,
        SelectedCachePolicy::ImplicitCoreDefaultPreserve
    );
    assert!(candidate.materialized.closure.rule_files.is_empty());
    assert_eq!(
        candidate.facts.immutable_rule_sha256,
        candidate.materialized.closure.rule_file_sha256
    );
    assert_eq!(candidate.facts.source_config_sha256.len(), 64);
    assert!(candidate
        .facts
        .source_config_sha256
        .bytes()
        .all(|byte| byte.is_ascii_hexdigit()));
    assert_eq!(candidate.facts.config_version, snapshot.config_version());
    assert_eq!(
        candidate.facts.input_state_revision,
        snapshot.state().revision
    );
    assert!(candidate.facts.runtime_bind_interfaces.is_empty());
    assert!(candidate.facts.deps.loopback_auth.is_none());
    let final_config = &candidate.materialized.emission.config;
    let cache_wire = serde_json::to_value(
        final_config
            .experimental
            .as_ref()
            .unwrap()
            .cache_file
            .as_ref()
            .unwrap(),
    )
    .unwrap();
    assert!(cache_wire.get("store_selected").is_none());
    assert!(final_config
        .services
        .as_deref()
        .unwrap_or_default()
        .iter()
        .any(|service| {
            service.type_field == "api"
                && service.listen_port == candidate.facts.deps.tailscale_api_port
        }));
    for (tag, port) in [
        ("update-in", candidate.facts.deps.update_in_port),
        (
            "subscription-update-in",
            candidate.facts.deps.subscription_update_in_port,
        ),
    ] {
        if let Some(port) = port {
            assert!(
                final_config
                    .inbounds
                    .iter()
                    .any(|inbound| { inbound.tag == tag && inbound.listen_port == Some(port) }),
                "{tag} must use the captured port"
            );
        }
    }
    assert!(candidate.facts.mesh_route_candidates.is_empty());
    assert_eq!(candidate.facts.mesh_route_total_candidate_count, 0);
    assert!(!candidate.facts.mesh_route_diagnostics_limited);
    assert!(candidate.facts.mesh_route_dns_owner_server_id.is_none());
    assert!(candidate.facts.pruned_rule_set_tags.is_empty());
    assert!(candidate.facts.pruned_env_rules.is_empty());
    assert!(candidate.facts.invalid_nodes.is_empty());
    assert!(candidate.facts.network_canary.is_none());
    assert_eq!(
        fs::read(&cache_path).unwrap(),
        b"persistent selected and dns cache"
    );
    assert_eq!(
        fs::read(&runtime_config_path).unwrap(),
        b"old runtime config"
    );
    assert!(!dir
        .join("mesh-routes/plans/candidate-direct-vless/manifest.json")
        .exists());
    assert_eq!(runtime.gate.generation(), gate_before);
    assert_eq!(runtime.dns_race.config_projection(), sidecar_before);
    #[cfg(unix)]
    {
        let mut old = runtime
            .child
            .lock()
            .unwrap()
            .take_running_for_test()
            .unwrap();
        assert!(old.child_for_test().try_wait().unwrap().is_none());
        old.child_for_test().kill().await.unwrap();
    }

    // Caller-side document mutation after generation cannot rewrite the
    // sealed bytes, raw identity, cache policy, or captured ports.
    let frozen_port = candidate.facts.deps.tailscale_api_port;
    fs::write(dir.join("config.json"), b"changed after candidate").unwrap();
    assert_eq!(candidate.facts.deps.tailscale_api_port, frozen_port);
    assert_eq!(candidate.facts.cache.path, cache_path);
    assert_eq!(candidate.materialized.closure.config_bytes, exact_bytes);
}

#[test]
fn unsupported_rules_and_stale_snapshot_publish_no_manifest() {
    let (dir, runtime, snapshot, mut input, plan) = fixture();
    input.state.revision = "2".into();
    assert!(matches!(
        generate_direct_vless_candidate(&runtime, &snapshot, &input, &plan),
        Err(CandidateError::SnapshotMismatch)
    ));
    assert!(!dir.join("mesh-routes").exists());

    let (dir, runtime, snapshot, input, plan) = fixture();
    let mut raw: Value =
        serde_json::from_slice(&fs::read(dir.join("config.json")).unwrap()).unwrap();
    raw["singboxDashboard"] = json!(true);
    fs::write(dir.join("config.json"), serde_json::to_vec(&raw).unwrap()).unwrap();
    assert!(matches!(
        generate_direct_vless_candidate(&runtime, &snapshot, &input, &plan),
        Err(CandidateError::SnapshotChanged)
    ));
    assert!(!dir.join("mesh-routes").exists());
}

#[test]
fn pure_candidate_validator_binds_owned_effective_facts_plan_emission_and_profile() {
    let (_dir, runtime, snapshot, input, plan) = fixture();
    let mut candidate =
        generate_direct_vless_candidate(&runtime, &snapshot, &input, &plan).unwrap();
    let profile = CandidateProfile::DesktopNonTunDirectVlessV1;
    assert_eq!(
        candidate.validate_same_candidate(&snapshot, &plan, profile),
        Ok(())
    );
    assert_eq!(
        candidate.facts.plan.identity_bindings,
        plan.identity_bindings
    );
    assert_eq!(
        candidate.metadata().ports,
        PortsProvenance::ProbedNumbersNotReserved
    );
    assert_eq!(
        candidate.metadata().manifest,
        ManifestPublication::Unpublished
    );
    assert!(candidate.metadata().managed_launch_unsupported);
    assert!(
        candidate
            .metadata()
            .cache_writer_lease_and_selector_readback_required
    );
    assert_eq!(
        candidate.validate_same_candidate(
            &snapshot,
            &plan,
            CandidateProfile::ManagedMultiTsUnsupported
        ),
        Err(CandidateIntegrityError::ProfileMismatch)
    );
    let mut changed_plan = plan.clone();
    changed_plan.identity_bindings.push(
        polaris_config_engine::user_config::mesh_route_state::MeshOwnerRef {
            server_id: "different-owner".into(),
            identity_epoch: "2".into(),
        },
    );
    assert_eq!(
        candidate.validate_same_candidate(&snapshot, &changed_plan, profile),
        Err(CandidateIntegrityError::PlanMismatch)
    );
    let port = candidate.facts.deps.tailscale_api_port;
    candidate.facts.deps.tailscale_api_port = port.saturating_add(1);
    assert_eq!(
        candidate.validate_same_candidate(&snapshot, &plan, profile),
        Err(CandidateIntegrityError::EffectiveFactsMismatch)
    );
    candidate.facts.deps.tailscale_api_port = port;
    candidate.facts.effective_user_config.servers[0].uuid = Some("different-secret".into());
    assert_eq!(
        candidate.validate_same_candidate(&snapshot, &plan, profile),
        Err(CandidateIntegrityError::EffectiveFactsMismatch)
    );
}

#[test]
fn pure_validator_rejects_changed_original_bytes_duplicate_rules_and_emission() {
    let (_dir, runtime, snapshot, input, plan) = fixture();
    let mut candidate =
        generate_direct_vless_candidate(&runtime, &snapshot, &input, &plan).unwrap();
    let profile = CandidateProfile::DesktopNonTunDirectVlessV1;
    candidate.materialized.closure.config_bytes.push(b' ');
    assert_eq!(
        candidate.validate_same_candidate(&snapshot, &plan, profile),
        Err(CandidateIntegrityError::ClosureMismatch)
    );
    candidate.materialized.closure.config_bytes.pop();
    candidate.materialized.closure.rule_files =
        vec![("same.json".into(), vec![1]), ("same.json".into(), vec![2])];
    assert_eq!(
        candidate.validate_same_candidate(&snapshot, &plan, profile),
        Err(CandidateIntegrityError::ClosureMismatch)
    );
    candidate.materialized.closure.rule_files.clear();
    candidate
        .materialized
        .emission
        .internal_inbound_exceptions
        .push("foreign-inbound".into());
    assert_eq!(
        candidate.validate_same_candidate(&snapshot, &plan, profile),
        Err(CandidateIntegrityError::EmissionMismatch)
    );
}

#[test]
fn live_deps_changes_do_not_rebuild_candidate_and_fresh_snapshot_is_required() {
    use std::sync::atomic::Ordering;
    let (dir, runtime, snapshot, input, plan) = fixture();
    let candidate = generate_direct_vless_candidate(&runtime, &snapshot, &input, &plan).unwrap();
    let original = candidate.materialized.closure.config_bytes.clone();
    let captured_deps = candidate.facts.deps_sha256.clone();
    runtime.netenv_dhcp_suppressed.store(true, Ordering::SeqCst);
    runtime
        .observed_tailnet
        .write()
        .unwrap()
        .insert("new-live-owner".into(), vec!["100.64.1.2".into()]);
    assert_eq!(
        candidate.validate_same_candidate(
            &snapshot,
            &plan,
            CandidateProfile::DesktopNonTunDirectVlessV1
        ),
        Ok(())
    );
    assert_eq!(candidate.materialized.closure.config_bytes, original);
    assert_eq!(candidate.facts.deps_sha256, captured_deps);
    let mut changed = snapshot.raw().clone();
    changed["newFutureSetting"] = json!(true);
    fs::write(
        dir.join("config.json"),
        serde_json::to_vec(&changed).unwrap(),
    )
    .unwrap();
    let current = runtime.config.read_mesh_apply_snapshot().unwrap();
    assert!(runtime.config.admit_mesh_apply_snapshot(&snapshot).is_err());
    assert_eq!(
        candidate.validate_same_candidate(
            &current,
            &plan,
            CandidateProfile::DesktopNonTunDirectVlessV1
        ),
        Err(CandidateIntegrityError::SnapshotMismatch)
    );
    assert_eq!(candidate.materialized.closure.config_bytes, original);
}

#[test]
fn new_check_capability_metadata_does_not_remove_ordinary_platform_features() {
    let (_dir, runtime, snapshot, input, plan) = fixture();
    let candidate = generate_direct_vless_candidate(&runtime, &snapshot, &input, &plan).unwrap();
    let mut deps = candidate.facts.deps.clone();
    for platform in ["darwin", "win32", "android", "other"] {
        deps.platform = platform.into();
        let metadata = metadata_for(&candidate.facts.effective_user_config, &deps);
        assert_eq!(metadata.check_support, CheckSupport::UnsupportedProfile);
        assert!(metadata.managed_launch_unsupported);
    }
    deps.platform = "linux".into();
    deps.arch = "aarch64".into();
    assert_eq!(
        metadata_for(&candidate.facts.effective_user_config, &deps).check_support,
        CheckSupport::UnsupportedProfile
    );
}

#[test]
fn unsupported_resource_inputs_fail_closed_without_publishing_or_touching_cache() {
    type RawMutation = (&'static str, fn(&mut Value));
    let mutations: &[RawMutation] = &[
        ("dashboard", |raw| raw["singboxDashboard"] = json!(true)),
        ("tun", |raw| raw["proxyModeType"] = json!("tun")),
        ("tailscale", |raw| {
            raw["servers"][0]["protocol"] = json!("tailscale")
        }),
        ("ssh", |raw| raw["servers"][0]["protocol"] = json!("ssh")),
        ("tor", |raw| raw["servers"][0]["protocol"] = json!("tor")),
        ("custom outbound", |raw| {
            raw["servers"][0]["customSettings"] = json!({"type":"vless","path":"/tmp/unknown"})
        }),
        ("unknown file path", |raw| {
            raw["servers"][0]["privateKeyPath"] = json!("/tmp/key")
        }),
        ("websocket path", |raw| {
            raw["servers"][0]["network"] = json!("ws");
            raw["servers"][0]["wsSettings"] = json!({"path":"/ws"});
        }),
        ("hot rule", |raw| {
            raw["customRules"] = json!([{"id":"hot-rule"}])
        }),
        ("rule resource", |raw| {
            raw["ruleResources"] = json!([{"id":"remote-rule","source":"remote"}])
        }),
        (
            "hosts path",
            |raw| raw["dnsServers"] = json!([{"id":"hosts","type":"hosts","paths":["/tmp/hosts"]}]),
        ),
    ];
    for (name, mutate) in mutations {
        let (dir, runtime, snapshot, input, plan) = fixture_with(*mutate);
        let cache_path = dir.join("cache.db");
        fs::write(&cache_path, b"old cache").unwrap();
        assert!(
            matches!(
                generate_direct_vless_candidate(&runtime, &snapshot, &input, &plan),
                Err(CandidateError::Unsupported)
            ),
            "{name}"
        );
        assert_eq!(fs::read(&cache_path).unwrap(), b"old cache", "{name}");
        assert!(!dir.join("mesh-routes").exists(), "{name}");
        assert!(!dir.join("singbox-runtime.json").exists(), "{name}");
    }
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[tokio::test]
async fn checked_candidate_prepares_same_protected_images_without_managed_permit() {
    let Some(binary) = pinned_core_for_test() else {
        return;
    };
    let (_dir, runtime, snapshot, input, plan) = fixture();
    let candidate = generate_direct_vless_candidate(&runtime, &snapshot, &input, &plan).unwrap();
    let expected_config = candidate.facts.final_config_sha256.clone();
    let checked = strict_check_pinned_linux_candidate(&runtime, &snapshot, candidate, &binary)
        .await
        .unwrap();
    let protected =
        protected_inputs::prepare_checked_linux_inputs(checked, &snapshot, &plan, &binary).unwrap();
    assert_eq!(protected.checked().config_sha256(), expected_config);
    assert_eq!(
        protected.checked().execution_profile(),
        check::CHECK_PROFILE
    );
    assert!(
        protected
            .checked()
            .candidate()
            .metadata()
            .managed_launch_unsupported
    );
    assert_eq!(
        protected.checked().candidate().metadata().manifest,
        ManifestPublication::Unpublished
    );
    use polaris_core_supervisor::exact_spawn::{LinuxInputProfile, MutableObligation};
    assert_eq!(
        protected
            .inputs()
            .validate_protection(LinuxInputProfile::B609PlainTcpCheckV1),
        Ok(())
    );
    assert_eq!(
        protected.inputs().mutable_obligations(),
        &[MutableObligation::CacheWriterLeaseAndSelectorReadback]
    );
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[tokio::test]
async fn pinned_strict_check_uses_full_builder_bytes_without_cache_or_listener_side_effects() {
    let Some(binary) = pinned_core_for_test() else {
        return;
    };
    for state in ["absent", "existing", "corrupt", "locked"] {
        let (dir, runtime, snapshot, input, plan) = fixture();
        let cache_path = dir.join("cache.db");
        match state {
            "existing" | "locked" => fs::write(&cache_path, b"old persistent cache").unwrap(),
            "corrupt" => fs::write(&cache_path, b"not a sqlite database\0\xff").unwrap(),
            _ => {}
        }
        let cache_before = fs::read(&cache_path).ok();
        let cache_lock = if state == "locked" {
            Some(
                nix::fcntl::Flock::lock(
                    fs::File::open(&cache_path).unwrap(),
                    nix::fcntl::FlockArg::LockExclusiveNonblock,
                )
                .unwrap(),
            )
        } else {
            None
        };
        let gate_before = runtime.gate.generation();
        let sidecar_before = runtime.dns_race.config_projection();
        let old_child = tokio::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .unwrap();
        runtime
            .child
            .lock()
            .unwrap()
            .install_running_for_test(DirectCoreRun::new(old_child));
        let candidate =
            generate_direct_vless_candidate(&runtime, &snapshot, &input, &plan).unwrap();
        let exact_bytes = candidate.materialized.closure.config_bytes.clone();
        let exact_hash = candidate.facts.final_config_sha256.clone();
        let config_cache_path = candidate.facts.cache.path.clone();
        let mut listeners = Vec::<TcpListener>::new();
        let mut ports = std::collections::BTreeSet::new();
        for inbound in &candidate.materialized.emission.config.inbounds {
            if let Some(port) = inbound.listen_port.filter(|port| *port > 0) {
                ports.insert(port);
            }
        }
        for service in candidate
            .materialized
            .emission
            .config
            .services
            .as_deref()
            .unwrap_or_default()
        {
            if service.listen_port > 0 {
                ports.insert(service.listen_port);
            }
        }
        for port in ports {
            listeners.push(TcpListener::bind((Ipv4Addr::LOCALHOST, port)).unwrap());
        }
        let checked = strict_check_pinned_linux_candidate(&runtime, &snapshot, candidate, &binary)
            .await
            .unwrap_or_else(|error| panic!("{state}: {error:?}"));
        assert_eq!(checked.config_sha256(), exact_hash, "{state}");
        assert_eq!(
            checked.binary_sha256(),
            check::PINNED_B609_LINUX_X86_64_SHA256
        );
        assert_eq!(checked.execution_profile(), check::CHECK_PROFILE);
        assert_eq!(
            checked.candidate().materialized.closure.config_bytes,
            exact_bytes
        );
        assert_eq!(checked.candidate().facts.cache.path, config_cache_path);
        assert_eq!(fs::read(&cache_path).ok(), cache_before, "{state}");
        assert!(!dir.join("mesh-routes").exists(), "{state}");
        assert!(!dir.join("singbox-runtime.json").exists(), "{state}");
        assert_eq!(runtime.gate.generation(), gate_before, "{state}");
        assert_eq!(
            runtime.dns_race.config_projection(),
            sidecar_before,
            "{state}"
        );
        let mut old = runtime
            .child
            .lock()
            .unwrap()
            .take_running_for_test()
            .unwrap();
        assert!(
            old.child_for_test().try_wait().unwrap().is_none(),
            "{state}"
        );
        old.child_for_test().kill().await.unwrap();
        drop(cache_lock);
        drop(listeners);
    }
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[tokio::test]
async fn pinned_strict_check_rejects_wrong_binary_and_changed_snapshot_without_manifest() {
    let Some(binary) = pinned_core_for_test() else {
        return;
    };
    let (dir, runtime, snapshot, input, plan) = fixture();
    let candidate = generate_direct_vless_candidate(&runtime, &snapshot, &input, &plan).unwrap();
    let bogus = dir.join("bogus-core");
    fs::write(&bogus, b"different executable").unwrap();
    assert!(matches!(
        strict_check_pinned_linux_candidate(&runtime, &snapshot, candidate, &bogus).await,
        Err(check::CandidateCheckError::Unsupported)
    ));
    assert!(!dir.join("mesh-routes").exists());

    let candidate = generate_direct_vless_candidate(&runtime, &snapshot, &input, &plan).unwrap();
    fs::write(dir.join("config.json"), b"changed after candidate").unwrap();
    assert!(matches!(
        strict_check_pinned_linux_candidate(&runtime, &snapshot, candidate, &binary).await,
        Err(check::CandidateCheckError::SnapshotChanged)
    ));
    assert!(!dir.join("mesh-routes").exists());
}

fn userspace_fixture_with(
    mutate: impl FnOnce(&mut Value),
) -> (TestDir, Arc<ProxyRuntime>, ApplyInputSnapshot) {
    let (dir, runtime, snapshot, _, _) = fixture_with(|raw| {
        raw["selectedServerId"] = json!("ts-a");
        raw["dnsConfig"] = json!({"resolveNodeDomainsAhead":false});
        raw["servers"] = json!([
            {"id":"ts-a", "name":"Alpha tailnet", "protocol":"tailscale", "tailscaleSettings":{
                "controlUrl":"https://control.example.test", "authKey":"tskey-alpha", "hostname":"alpha",
                "allowInternet":false, "alwaysRouteSubnets":true, "acceptRoutes":true,
                "routes":["10.11.0.0/24", "fd00:11::/64"]}},
            {"id":"ts-b", "name":"Beta tailnet", "protocol":"tailscale", "tailscaleSettings":{
                "controlUrl":"https://control.example.test", "authKey":"tskey-beta", "hostname":"beta",
                "allowInternet":false, "alwaysRouteSubnets":true, "acceptRoutes":true,
                "routes":["10.12.0.0/24", "fd00:12::/64"]}}
        ]);
        raw[POLICY_KEY]["candidateOrder"] = json!(["ts-a", "ts-b"]);
        raw[STATE_KEY]["identities"] = json!([
            {"serverId":"ts-a", "identityEpoch":"epoch-a", "controlAuthority":"https://control.example.test",
                "selfStableId":"self-a", "bindingState":"bound", "evidenceSource":"status"},
            {"serverId":"ts-b", "identityEpoch":"epoch-b", "controlAuthority":"https://control.example.test",
                "selfStableId":"self-b", "bindingState":"bound", "evidenceSource":"status"}
        ]);
    });
    // The legacy fixture compiles an empty VLESS plan. Apply mesh-negative
    // mutations afterward so the production candidate, rather than fixture
    // plan setup, actually evaluates missing mesh evidence and matchers.
    let mut raw = snapshot.raw().clone();
    mutate(&mut raw);
    fs::write(dir.join("config.json"), serde_json::to_vec(&raw).unwrap()).unwrap();
    let snapshot = runtime.config.read_mesh_apply_snapshot().unwrap();
    (dir, runtime, snapshot)
}

#[test]
fn production_userspace_candidate_uses_the_full_builder_for_two_ts_and_same_plan_bytes() {
    let (dir, runtime, snapshot) = userspace_fixture_with(|_| {});
    fs::write(dir.join("cache.db"), b"original mutable cache").unwrap();
    fs::write(dir.join("singbox-runtime.json"), b"old runtime").unwrap();
    let generation = runtime.gate.generation();
    let sidecar = runtime.dns_race.config_projection();
    let candidate =
        generate_userspace_mesh_candidate(&runtime, &snapshot, "two-ts-source").unwrap();
    let profile = CandidateProfile::DesktopNonTunUserspaceMultiTsV1;
    assert_eq!(
        candidate.validate_same_candidate(&snapshot, candidate.plan(), profile),
        Ok(())
    );
    let same_builder = generate_sing_box_config_with_report_and_runtime_bindings(
        &candidate.facts.effective_user_config,
        &BTreeMap::new(),
        &candidate.facts.deps,
        &candidate.facts.runtime_bind_interfaces,
    )
    .unwrap();
    assert_eq!(same_builder.config, candidate.facts.source_config);
    assert_eq!(
        serde_json::to_vec_pretty(&same_builder.config).unwrap(),
        candidate.facts.source_config_bytes
    );
    assert_eq!(
        route_evidence_hash(&same_builder.mesh_route_candidates).unwrap(),
        candidate.facts.mesh_route_evidence_sha256
    );
    assert_eq!(candidate.facts.plan_input.candidates.len(), 2);
    assert_eq!(candidate.plan().owner_routes.len(), 4);
    for (id, epoch, prefix) in [
        ("ts-a", "epoch-a", "10.11.0.0/24"),
        ("ts-b", "epoch-b", "10.12.0.0/24"),
    ] {
        let input = candidate
            .facts
            .plan_input
            .candidates
            .iter()
            .find(|input| input.owner_ref.server_id == id)
            .unwrap();
        assert_eq!(input.owner_ref.identity_epoch, epoch);
        assert!(candidate
            .plan()
            .owner_routes
            .iter()
            .any(|route| route.cidr == prefix
                && route.owner_ref == input.owner_ref
                && Some(&route.endpoint_tag) == input.endpoint_tag.as_ref()));
    }
    let endpoints = candidate
        .materialized
        .emission
        .config
        .endpoints
        .as_ref()
        .unwrap();
    assert_eq!(endpoints.len(), 2);
    assert_ne!(endpoints[0].state_directory, endpoints[1].state_directory);
    assert_eq!(
        endpoints,
        candidate.facts.source_config.endpoints.as_ref().unwrap()
    );
    assert!(endpoints
        .iter()
        .all(|endpoint| endpoint.system_interface != Some(true)
            && Path::new(endpoint.state_directory.as_ref().unwrap()).is_absolute()
            && Path::new(endpoint.taildrop_directory.as_ref().unwrap()).is_absolute()));
    assert_eq!(
        candidate.config_bytes(),
        serde_json::to_vec_pretty(&candidate.materialized.emission.config).unwrap()
    );
    assert_eq!(
        candidate.metadata().check_support,
        CheckSupport::UnsupportedProfile
    );
    assert!(
        candidate
            .metadata()
            .tailscale_state_and_taildrop_handoff_required
    );
    assert!(candidate.metadata().managed_launch_unsupported);
    assert_eq!(
        candidate.metadata().ports,
        PortsProvenance::ProbedNumbersNotReserved
    );
    assert_eq!(
        candidate.facts.cache.ownership,
        CacheOwnership::PersistentMutablePreserve
    );
    assert_eq!(runtime.gate.generation(), generation);
    assert_eq!(runtime.dns_race.config_projection(), sidecar);
    assert_eq!(
        fs::read(dir.join("cache.db")).unwrap(),
        b"original mutable cache"
    );
    assert_eq!(
        fs::read(dir.join("singbox-runtime.json")).unwrap(),
        b"old runtime"
    );
    assert!(!dir.join("mesh-routes").exists());
}

#[test]
fn userspace_overlap_targets_and_dns_use_original_q_and_d1_algorithms() {
    let (_dir, runtime, snapshot) = userspace_fixture_with(|raw| {
        raw["servers"][1]["tailscaleSettings"]["routes"] = json!(["10.11.0.0/24", "fd00:11::/64"]);
        raw[POLICY_KEY]["assignments"] = json!([
            {"cidr":"10.11.0.0/25", "target":{"kind":"owner", "serverId":"ts-b", "identityEpoch":"epoch-b"}},
            {"cidr":"10.11.0.128/26", "target":{"kind":"reject"}},
            {"cidr":"10.11.0.192/26", "target":{"kind":"unmanaged"}}
        ]);
        raw[POLICY_KEY]["dnsPolicy"] = json!({
            "schemaVersion":1, "suffixAssignments":[{"suffix":"alpha.test", "target":{"kind":"owner", "serverId":"ts-a", "identityEpoch":"epoch-a"}}],
            "shortNamePolicy":{"kind":"reject"}, "serviceOwner":{"kind":"owner", "serverId":"ts-b", "identityEpoch":"epoch-b"}
        });
    });
    let candidate =
        generate_userspace_mesh_candidate(&runtime, &snapshot, "two-ts-overlap").unwrap();
    assert!(candidate
        .plan()
        .owner_routes
        .iter()
        .any(|route| route.cidr == "10.11.0.0/25" && route.owner_ref.server_id == "ts-b"));
    assert!(candidate
        .plan()
        .unassigned_cidrs
        .iter()
        .any(|cidr| cidr == "10.11.0.128/26"));
    assert_eq!(candidate.plan().released_cidrs, vec!["10.11.0.192/26"]);
    assert!(candidate.plan().dns_managed);
    assert!(!candidate
        .materialized
        .emission
        .ordinary_port53_hijack_cidrs
        .is_empty());
    assert!(!candidate
        .materialized
        .emission
        .internal_inbound_exceptions
        .is_empty());
    assert_eq!(
        candidate.validate_same_candidate(
            &snapshot,
            candidate.plan(),
            CandidateProfile::DesktopNonTunUserspaceMultiTsV1
        ),
        Ok(())
    );
}

#[test]
fn userspace_unknown_epochs_resources_and_acl_never_strip_or_stop_old_config() {
    type RawMutation = fn(&mut Value);
    let cases: &[(&str, RawMutation)] = &[
        ("missing identity", |raw| {
            raw[STATE_KEY]["identities"] = json!([])
        }),
        ("foreign control", |raw| {
            raw[STATE_KEY]["identities"][0]["controlAuthority"] =
                json!("https://foreign.example.test")
        }),
        ("system TS", |raw| {
            raw["servers"][0]["tailscaleSettings"]["reverseMesh"] = json!(true)
        }),
        ("explicit interface", |raw| {
            raw["servers"][0]["bindInterface"] = json!("unverified0")
        }),
        ("TUN", |raw| raw["proxyModeType"] = json!("tun")),
        ("unowned requested sidecar", |raw| {
            raw["dnsConfig"]["resolveNodeDomainsAhead"] = json!(true)
        }),
        ("unknown TS input", |raw| {
            raw["servers"][0]["tailscaleSettings"]["futureFilePath"] = json!("/unknown/file")
        }),
        ("managed override without actual matcher", |raw| {
            raw[POLICY_KEY]["overrides"] = json!([{"ruleId":"unknown", "scopeCidrs":["10.11.0.0/24"], "target":{"kind":"reject"}}])
        }),
        ("explicit ACL inbound pin", |raw| {
            raw["servers"][0]["meshInboundPolicy"] = json!({"mode":"block"})
        }),
    ];
    for (name, mutate) in cases {
        let (dir, runtime, snapshot) = userspace_fixture_with(*mutate);
        let raw_before = fs::read(dir.join("config.json")).unwrap();
        let generation = runtime.gate.generation();
        let result = generate_userspace_mesh_candidate(&runtime, &snapshot, "unsupported-two-ts");
        assert!(result.is_err(), "{name}");
        if *name == "explicit ACL inbound pin" {
            assert!(matches!(result, Err(CandidateError::Unsupported)));
        }
        assert_eq!(
            fs::read(dir.join("config.json")).unwrap(),
            raw_before,
            "{name}"
        );
        assert_eq!(runtime.gate.generation(), generation, "{name}");
        assert!(!dir.join("mesh-routes").exists(), "{name}");
        assert!(!dir.join("singbox-runtime.json").exists(), "{name}");
    }
    let (_dir, runtime, snapshot) = userspace_fixture_with(|_| {});
    runtime
        .observed_tailnet
        .write()
        .unwrap()
        .insert("ts-a".into(), vec!["100.80.1.2".into()]);
    assert!(matches!(
        generate_userspace_mesh_candidate(&runtime, &snapshot, "unbound-observation"),
        Err(CandidateError::SnapshotMismatch)
    ));
    runtime.observed_tailnet.write().unwrap().clear();
    runtime.set_race_server(39991, vec!["1.1.1.1".into()], vec![53]);
    assert!(matches!(
        generate_userspace_mesh_candidate(&runtime, &snapshot, "unleased-sidecar"),
        Err(CandidateError::Unsupported)
    ));
    runtime.set_race_server(0, Vec::new(), Vec::new());
    runtime
        .tailnet_file_write_epoch
        .store(1, std::sync::atomic::Ordering::SeqCst);
    assert!(matches!(
        generate_userspace_mesh_candidate(&runtime, &snapshot, "writing-evidence"),
        Err(CandidateError::SnapshotChanged)
    ));
}

#[test]
fn userspace_external_rule_evidence_and_relocation_share_one_owned_file_snapshot() {
    let (dir, runtime, snapshot) = userspace_fixture_with(|_| {});
    let rules = runtime.tailnet_rules_dir();
    fs::create_dir(&rules).unwrap();
    let config: UserConfig = serde_json::from_value(snapshot.raw().clone()).unwrap();
    let mut originals = Vec::new();
    for server in &config.servers {
        let cidrs = polaris_config_engine::builder::endpoint_routes::endpoint_forced_route_cidrs(
            server,
            &BTreeMap::new(),
        );
        let bytes =
            serde_json::to_vec_pretty(&json!({"version":1, "rules":[{"ip_cidr":cidrs}]})).unwrap();
        let path = rules.join(format!(
            "{}.json",
            polaris_config_engine::builder::endpoint_routes::tailnet_rule_file_base(&server.id)
        ));
        fs::write(&path, &bytes).unwrap();
        originals.push((path, bytes));
    }
    let candidate = generate_userspace_mesh_candidate(&runtime, &snapshot, "two-ts-files").unwrap();
    assert_eq!(candidate.materialized.closure.rule_files.len(), 2);
    assert!(candidate
        .facts
        .mesh_route_candidates
        .iter()
        .all(|audit| audit.candidate.leg == ForceRouteLeg::ExternalRuleSet));
    for audit in &candidate.facts.mesh_route_candidates {
        let path = audit.external_path.as_ref().unwrap();
        let original = originals
            .iter()
            .find(|(file, _)| file == Path::new(path))
            .unwrap();
        assert_eq!(
            candidate.facts.owned_rules.bytes_at(path).unwrap(),
            original.1
        );
        assert!(candidate
            .materialized
            .closure
            .rule_files
            .iter()
            .any(|(_, bytes)| bytes == &original.1));
    }
    assert_eq!(
        candidate.validate_source_inputs(&runtime, &snapshot),
        Ok(())
    );
    let frozen = candidate.config_bytes().to_vec();
    let original = &originals[0];
    let replacement = original.0.with_extension("replacement");
    fs::write(&replacement, &original.1).unwrap();
    fs::rename(&replacement, &original.0).unwrap();
    assert_eq!(
        candidate.validate_source_inputs(&runtime, &snapshot),
        Err(CandidateError::SnapshotChanged)
    );
    assert_eq!(
        candidate.validate_same_candidate(
            &snapshot,
            candidate.plan(),
            CandidateProfile::DesktopNonTunUserspaceMultiTsV1
        ),
        Ok(())
    );
    assert_eq!(candidate.config_bytes(), frozen);
    assert!(!dir.join("mesh-routes").exists());
}

#[test]
fn userspace_malformed_foreign_tailnet_file_cannot_supply_a_different_plan() {
    for content in [
        json!({"version":1,"rules":[{"ip_cidr":["10.255.0.0/16"]}]}),
        json!({"version":1,"rules":[{"domain":["foreign.example.test"]}]}),
        json!({"version":2,"rules":[{"ip_cidr":[]}]}),
    ] {
        let (dir, runtime, snapshot) = userspace_fixture_with(|_| {});
        let root = runtime.tailnet_rules_dir();
        fs::create_dir(&root).unwrap();
        let source = root.join(format!(
            "{}.json",
            polaris_config_engine::builder::endpoint_routes::tailnet_rule_file_base("ts-a")
        ));
        let bytes = serde_json::to_vec(&content).unwrap();
        fs::write(&source, &bytes).unwrap();
        assert!(generate_userspace_mesh_candidate(&runtime, &snapshot, "foreign-rule").is_err());
        assert_eq!(fs::read(&source).unwrap(), bytes);
        assert!(!dir.join("mesh-routes").exists());
    }
}

#[test]
fn userspace_bound_observation_can_plan_but_memory_changes_are_not_live_owner_proof() {
    let (_dir, runtime, snapshot) = userspace_fixture_with(|raw| {
        raw[STATE_KEY]["observations"] = json!([{"ownerRef":{"serverId":"ts-a","identityEpoch":"epoch-a"},
            "rawHosts":["100.80.1.2/32"],"advertisedRoutes":[],"source":"status","lastValidEvidence":"saved-bound-frame"}]);
    });
    runtime
        .observed_tailnet
        .write()
        .unwrap()
        .insert("ts-a".into(), vec!["100.80.1.2".into()]);
    let mut candidate =
        generate_userspace_mesh_candidate(&runtime, &snapshot, "bound-source").unwrap();
    assert!(candidate
        .plan()
        .owner_routes
        .iter()
        .any(|route| route.cidr == "100.80.1.2/32" && route.owner_ref.server_id == "ts-a"));
    assert_eq!(
        candidate.validate_source_inputs(&runtime, &snapshot),
        Ok(())
    );
    runtime
        .observed_tailnet
        .write()
        .unwrap()
        .insert("ts-a".into(), vec!["100.80.1.3".into()]);
    assert_eq!(
        candidate.validate_source_inputs(&runtime, &snapshot),
        Err(CandidateError::SnapshotChanged)
    );
    let profile = CandidateProfile::DesktopNonTunUserspaceMultiTsV1;
    assert_eq!(
        candidate.validate_same_candidate(&snapshot, candidate.plan(), profile),
        Ok(())
    );
    let plan = candidate.plan().clone();
    candidate.facts.deps.loopback_auth = Some(polaris_config_engine::singbox::InboundUser {
        username: "foreign".into(),
        password: "other-generation".into(),
    });
    assert_eq!(
        candidate.validate_same_candidate(&snapshot, &plan, profile),
        Err(CandidateIntegrityError::EffectiveFactsMismatch)
    );
    candidate.facts.deps.loopback_auth = None;
    candidate.facts.mesh_route_candidates[0].candidate.tag = "foreign-endpoint".into();
    assert_eq!(
        candidate.validate_same_candidate(&snapshot, &plan, profile),
        Err(CandidateIntegrityError::EffectiveFactsMismatch)
    );
}

#[test]
fn userspace_rejects_incomplete_duplicate_or_missing_full_builder_endpoint_evidence() {
    let (_dir, runtime, snapshot) = userspace_fixture_with(|_| {});
    let candidate =
        generate_userspace_mesh_candidate(&runtime, &snapshot, "evidence-guards").unwrap();
    let f = &candidate.facts;
    let derive = |config: &SingBoxConfig, audits: &[MeshRouteEmissionCandidate], total, limited| {
        managed_input_from_builder(
            &snapshot,
            &f.effective_user_config,
            &f.deps,
            config,
            audits,
            total,
            limited,
            &f.owned_rules,
            &f.plan_input.plan_id,
        )
    };
    assert!(derive(&f.source_config, &f.mesh_route_candidates, 2, true).is_err());
    assert!(derive(&f.source_config, &f.mesh_route_candidates[..1], 2, false).is_err());
    let mut missing = f.source_config.clone();
    missing.endpoints.as_mut().unwrap().pop();
    assert!(derive(&missing, &f.mesh_route_candidates, 2, false).is_err());
    let mut duplicate = f.source_config.clone();
    duplicate.endpoints.as_mut().unwrap()[1] = duplicate.endpoints.as_ref().unwrap()[0].clone();
    assert!(derive(&duplicate, &f.mesh_route_candidates, 2, false).is_err());
    let mut unknown = f.mesh_route_candidates.clone();
    unknown[0].candidate.match_cidrs = None;
    assert!(derive(&f.source_config, &unknown, 2, false).is_err());
}
