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

#[tokio::test]
async fn full_builder_candidate_seals_direct_vless_and_preserves_old_resources() {
    let (dir, runtime, snapshot, input, plan) = fixture();
    let parsed: UserConfig = serde_json::from_value(snapshot.raw().clone()).unwrap();
    assert!(conservative_user_input(&parsed));
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
