//! Test-side probe for an already running, owned final-package core.
//! No product binary is built or substituted by this program. All inputs are
//! bounded stdin JSON; argv, config contents, credentials and RPC messages are
//! deliberately never echoed. A lost Apply reply is reconciled, not resubmitted.
use polaris_singbox_grpc::{
    daemon as d, ClientError, Endpoint, RuntimeConfigError, SingBoxApiClient,
};
use serde_json::{json, Value};
use std::{collections::BTreeSet, fs, io::Read, path::Path};

type ProbeResult<T> = Result<T, Value>;
const MAX_INPUT: u64 = 4 * 1024 * 1024 + 65536;
const METHODS: [&str; 6] = [
    "GetRuntimeCapabilities",
    "PlanRuntimeConfig",
    "ApplyRuntimeConfig",
    "GetRuntimeConfigStatus",
    "RetryRuntimePersistence",
    "AcknowledgeRuntimePersistence",
];

fn invalid() -> Value {
    json!({"kind":"invalid-input"})
}
fn check(ok: bool, kind: &str) -> ProbeResult<()> {
    if ok {
        Ok(())
    } else {
        Err(json!({"kind":kind}))
    }
}
fn string<'a>(value: &'a Value, key: &str) -> ProbeResult<&'a str> {
    value.get(key).and_then(Value::as_str).ok_or_else(invalid)
}
fn number(value: &Value, key: &str) -> ProbeResult<u64> {
    value.get(key).and_then(Value::as_u64).ok_or_else(invalid)
}
fn id(value: &Value) -> ProbeResult<Option<d::RuntimeIdentity>> {
    if value.is_null() {
        return Ok(None);
    }
    Ok(Some(d::RuntimeIdentity {
        incarnation: string(value, "incarnation")?.into(),
        applied_revision: number(value, "appliedRevision")?,
        effective_digest: string(value, "effectiveDigest")?.into(),
    }))
}
fn identity(value: Option<&d::RuntimeIdentity>) -> Value {
    value.map_or(Value::Null, |i| {
        json!({"incarnation":i.incarnation,"appliedRevision":i.applied_revision,
               "effectiveDigest":i.effective_digest})
    })
}
fn errors(value: &[d::RuntimePlanError]) -> Value {
    // Human error messages may contain config values; only fixed codes/paths.
    json!(value
        .iter()
        .map(|e| json!({"code":e.code,"effectivePath":e.effective_path}))
        .collect::<Vec<_>>())
}
fn capabilities(value: &d::RuntimeCapabilities) -> Value {
    json!({"contractVersion":value.contract_version,"connectedHotEnabled":value.connected_hot_enabled,
        "implementationManifestDigest":value.implementation_manifest_digest,
        "active":identity(value.active.as_ref()),"controlRevision":value.control_revision,
        "fullEffectiveSchemaSupported":value.full_effective_schema_supported,
        "receiptRetentionSeconds":value.receipt_retention_seconds,"maxReceipts":value.max_receipts})
}
fn plan(value: &d::RuntimeConfigPlan) -> Value {
    json!({"contractVersion":value.contract_version,"planId":value.plan_id,
        "expected":identity(value.expected.as_ref()),"candidateDigest":value.candidate_digest,
        "dependencyDigest":value.dependency_digest,"impactDigest":value.impact_digest,
        "expectedControlRevision":value.expected_control_revision,"transactionId":value.transaction_id,
        "applicable":value.applicable,"noOp":value.no_op,"errors":errors(&value.errors)})
}
fn operation(value: &d::RuntimeConfigOperation) -> Value {
    json!({"contractVersion":value.contract_version,"transactionId":value.transaction_id,
        "state":value.state,"durable":value.durable,"before":identity(value.before.as_ref()),
        "active":identity(value.active.as_ref()),"candidateDigest":value.candidate_digest,
        "impactDigest":value.impact_digest,"controlRevision":value.control_revision,
        "persistenceMode":value.persistence_mode,"ownerReloadAcknowledged":value.owner_reload_acknowledged,
        "retirementState":value.retirement_state,"desiredStateDigest":value.desired_state_digest,
        "desiredStateRevision":value.desired_state_revision,"errors":errors(&value.errors)})
}
fn status(value: &d::RuntimeConfigStatus) -> Value {
    json!({"contractVersion":value.contract_version,"active":identity(value.active.as_ref()),
        "operation":value.operation.as_ref().map(operation),"lookupState":value.lookup_state,
        "durableAppliedDigest":value.durable_applied_digest,"controlRevision":value.control_revision,
        "persistenceMode":value.persistence_mode,"durableDesiredStateDigest":value.durable_desired_state_digest,
        "durableDesiredStateRevision":value.durable_desired_state_revision,
        "durableOwnerReloadAcknowledged":value.durable_owner_reload_acknowledged})
}
fn error_info(error: RuntimeConfigError) -> Value {
    match error {
        RuntimeConfigError::Unsupported(_) => json!({"kind":"unsupported","code":12}),
        RuntimeConfigError::Call(ClientError::Status(status)) => {
            json!({"kind":"rpc","code":status.code() as i32})
        }
        RuntimeConfigError::Call(_) => json!({"kind":"transport"}),
    }
}
fn expect_code<T>(result: Result<T, RuntimeConfigError>, code: i32) -> ProbeResult<()> {
    match result {
        Err(RuntimeConfigError::Call(ClientError::Status(status)))
            if status.code() as i32 == code =>
        {
            Ok(())
        }
        _ => Err(json!({"kind":"expected-rpc-rejection-not-observed","expectedCode":code})),
    }
}
fn start_time(text: &str) -> Option<&str> {
    text.rsplit_once(')')?.1.split_whitespace().nth(19)
}
fn uid(text: &str) -> Option<&str> {
    text.lines()
        .find(|line| line.starts_with("Uid:"))?
        .split_whitespace()
        .nth(2)
}
fn read(path: impl AsRef<Path>) -> ProbeResult<String> {
    fs::read_to_string(path).map_err(|_| json!({"kind":"owned-process-evidence-unavailable"}))
}
fn authority(action: &str, own_uid: &str, gui_uid: Option<u64>) -> ProbeResult<bool> {
    let tun = matches!(action, "tun-capabilities" | "tun-status");
    if tun {
        check(
            own_uid == "0" && gui_uid.is_some_and(|uid| uid > 0 && uid <= u32::MAX as u64),
            "root-read-only-tun-probe-required",
        )?;
    } else {
        check(own_uid != "0", "non-root-probe-required")?;
    }
    Ok(tun)
}

#[cfg(target_os = "linux")]
fn verify_tun_owned(
    input: &Value,
    base: &str,
    exe: &Path,
    own_gui_uid: u64,
    port: u16,
) -> ProbeResult<()> {
    use std::os::unix::fs::MetadataExt;
    let owned = input.get("owned").ok_or_else(invalid)?;
    check(
        exe.starts_with("/usr/local/lib/polaris/core")
            && exe.file_name().is_some_and(|v| v == "sing-box"),
        "protected-installed-core-required",
    )?;
    for path in exe.ancestors() {
        let st = fs::symlink_metadata(path).map_err(|_| invalid())?;
        check(
            !st.file_type().is_symlink() && st.uid() == 0 && st.mode() & 0o022 == 0,
            "root-owned-protected-core-required",
        )?;
    }
    let st = fs::metadata(exe).map_err(|_| invalid())?;
    let running = fs::metadata(format!("{base}/exe")).map_err(|_| invalid())?;
    check(
        st.is_file()
            && st.dev() == running.dev()
            && st.ino() == running.ino()
            && st.dev() == number(owned, "exeDev")?
            && st.ino() == number(owned, "exeInode")?,
        "protected-core-inode-changed",
    )?;
    let status = read(format!("{base}/status"))?;
    for key in ["CapEff:", "CapAmb:"] {
        let value = status
            .lines()
            .find(|l| l.starts_with(key))
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|v| u64::from_str_radix(v, 16).ok());
        check(value == Some(0x3400), "production-core-capabilities-differ")?;
    }
    check(
        read(format!("{base}/cgroup"))?.trim() == "0::/system.slice/polaris-helper.service",
        "production-helper-cgroup-required",
    )?;
    let runtime_path = Path::new(string(owned, "runtimeConfig")?);
    check(
        runtime_path.is_absolute()
            && runtime_path
                .file_name()
                .is_some_and(|v| v == "singbox-runtime.json"),
        "actual-runtime-path-required",
    )?;
    let st = fs::symlink_metadata(runtime_path).map_err(|_| invalid())?;
    check(
        st.is_file()
            && !st.file_type().is_symlink()
            && st.uid() as u64 == own_gui_uid
            && st.size() <= 8 * 1024 * 1024,
        "private-gui-runtime-required",
    )?;
    let canonical = fs::canonicalize(runtime_path).map_err(|_| invalid())?;
    check(canonical == runtime_path, "linked-runtime-path")?;
    let cmdline = fs::read(format!("{base}/cmdline")).map_err(|_| invalid())?;
    let args: Vec<_> = cmdline
        .split(|b| *b == 0)
        .filter_map(|v| std::str::from_utf8(v).ok())
        .collect();
    let configs: Vec<_> = args
        .windows(2)
        .filter(|w| matches!(w[0], "-c" | "--config"))
        .map(|w| w[1])
        .collect();
    check(
        configs.len() == 1 && Path::new(configs[0]) == runtime_path,
        "runtime-path-not-in-core-command",
    )?;
    let raw = read(runtime_path)?;
    let config: Value = serde_json::from_str(&raw).map_err(|_| invalid())?;
    let apis: Vec<_> = config["services"]
        .as_array()
        .ok_or_else(invalid)?
        .iter()
        .filter(|s| s["type"] == "api")
        .collect();
    check(
        apis.len() == 1
            && apis[0]["listen"] == "127.0.0.1"
            && apis[0]["listen_port"] == port
            && apis[0]["secret"].as_str() == Some(string(input, "secret")?),
        "actual-core-api-differs",
    )?;
    check(
        config["inbounds"].as_array().is_some_and(|rows| {
            rows.iter()
                .any(|r| r["type"] == "tun" && r["interface_name"] == "polaris-tun0")
        }),
        "actual-production-tun-required",
    )?;
    Ok(())
}
#[cfg(not(target_os = "linux"))]
fn verify_tun_owned(_: &Value, _: &str, _: &Path, _: u64, _: u16) -> ProbeResult<()> {
    Err(invalid())
}

fn verify_owned(input: &Value, port: u16, verify_content: bool) -> ProbeResult<()> {
    check(cfg!(target_os = "linux"), "linux-only-probe")?;
    let own_status = read("/proc/self/status")?;
    let own_uid = uid(&own_status).ok_or_else(invalid)?;
    let owned = input.get("owned").ok_or_else(invalid)?;
    let tun = authority(
        string(input, "action")?,
        own_uid,
        owned.get("guiUid").and_then(Value::as_u64),
    )?;
    let expected_uid = if tun {
        number(owned, "guiUid")?.to_string()
    } else {
        own_uid.to_owned()
    };
    let pid = number(owned, "pid")?;
    check(pid > 1 && pid <= i32::MAX as u64, "invalid-owned-pid")?;
    let base = format!("/proc/{pid}");
    let stat = read(format!("{base}/stat"))?;
    let expected_start = number(owned, "startTimeTicks")?.to_string();
    check(
        start_time(&stat) == Some(expected_start.as_str()),
        "owned-process-changed",
    )?;
    check(
        uid(&read(format!("{base}/status"))?) == Some(expected_uid.as_str()),
        "owned-process-uid-differs",
    )?;
    let exe = fs::read_link(format!("{base}/exe")).map_err(|_| invalid())?;
    let expected_exe = fs::canonicalize(string(owned, "exe")?).map_err(|_| invalid())?;
    check(exe == expected_exe, "owned-executable-differs")?;
    if tun {
        verify_tun_owned(input, &base, &exe, number(owned, "guiUid")?, port)?;
    }
    let local = format!("0100007F:{port:04X}");
    let inodes: BTreeSet<String> = read(format!("{base}/net/tcp"))?
        .lines()
        .filter_map(|line| {
            let fields: Vec<_> = line.split_whitespace().collect();
            if fields.len() > 9 && fields[1] == local && fields[3] == "0A" {
                Some(format!("socket:[{}]", fields[9]))
            } else {
                None
            }
        })
        .collect();
    check(inodes.len() == 1, "owned-loopback-listener-not-unique")?;
    let found = fs::read_dir(format!("{base}/fd"))
        .map_err(|_| invalid())?
        .filter_map(Result::ok)
        .filter_map(|entry| fs::read_link(entry.path()).ok())
        .any(|path| inodes.contains(path.to_string_lossy().as_ref()));
    check(found, "listener-not-owned-by-core")?;
    check(
        start_time(&read(format!("{base}/stat"))?) == Some(expected_start.as_str()),
        "owned-process-changed",
    )?;
    if verify_content && string(input, "action")? == "suite" {
        let runtime_path =
            fs::canonicalize(string(owned, "runtimeConfig")?).map_err(|_| invalid())?;
        let cmdline = fs::read(format!("{base}/cmdline")).map_err(|_| invalid())?;
        let mut names = cmdline
            .split(|byte| *byte == 0)
            .filter_map(|part| std::str::from_utf8(part).ok());
        check(
            names.any(|arg| Path::new(arg) == runtime_path.as_path()),
            "runtime-path-not-in-core-command",
        )?;
        let request = input.get("request").ok_or_else(invalid)?;
        check(
            read(runtime_path)? == string(request, "configContent")?,
            "runtime-content-differs",
        )?;
    }
    Ok(())
}

fn plan_request(value: &Value) -> ProbeResult<d::PlanRuntimeConfigRequest> {
    let intents = value
        .get("selectorIntents")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| {
                    Ok(d::RuntimeSelectorIntent {
                        group: string(item, "group")?.into(),
                        selected: string(item, "selected")?.into(),
                        expected_selection_path: item
                            .get("expectedSelectionPath")
                            .and_then(Value::as_array)
                            .ok_or_else(invalid)?
                            .iter()
                            .map(|v| v.as_str().map(String::from).ok_or_else(invalid))
                            .collect::<ProbeResult<_>>()?,
                        pin_current_tcp: item
                            .get("pinCurrentTcp")
                            .and_then(Value::as_bool)
                            .ok_or_else(invalid)?,
                    })
                })
                .collect::<ProbeResult<Vec<_>>>()
        })
        .transpose()?
        .unwrap_or_default();
    Ok(d::PlanRuntimeConfigRequest {
        contract_version: 1,
        expected: id(&value["expected"])?,
        config_content: string(value, "configContent")?.into(),
        overrides: None,
        expected_control_revision: number(value, "expectedControlRevision")?,
        selector_intents: intents,
    })
}
fn apply_request(value: &Value) -> ProbeResult<d::ApplyRuntimeConfigRequest> {
    Ok(d::ApplyRuntimeConfigRequest {
        contract_version: 1,
        transaction_id: string(value, "transactionId")?.into(),
        plan_id: string(value, "planId")?.into(),
        expected: id(&value["expected"])?,
        candidate_digest: string(value, "candidateDigest")?.into(),
        accepted_impact_digest: string(value, "acceptedImpactDigest")?.into(),
        expected_control_revision: number(value, "expectedControlRevision")?,
    })
}
fn planned_apply(value: &d::RuntimeConfigPlan) -> d::ApplyRuntimeConfigRequest {
    d::ApplyRuntimeConfigRequest {
        contract_version: 1,
        transaction_id: value.transaction_id.clone(),
        plan_id: value.plan_id.clone(),
        expected: value.expected.clone(),
        candidate_digest: value.candidate_digest.clone(),
        accepted_impact_digest: value.impact_digest.clone(),
        expected_control_revision: value.expected_control_revision,
    }
}
fn status_request(transaction: &str, incarnation: &str) -> d::RuntimeConfigStatusRequest {
    d::RuntimeConfigStatusRequest {
        contract_version: 1,
        transaction_id: transaction.into(),
        runtime_incarnation: incarnation.into(),
    }
}
async fn apply_once(
    client: &SingBoxApiClient,
    request: d::ApplyRuntimeConfigRequest,
) -> ProbeResult<d::RuntimeConfigOperation> {
    let lookup = status_request(
        &request.transaction_id,
        request
            .expected
            .as_ref()
            .map_or("", |i| i.incarnation.as_str()),
    );
    match client.apply_runtime_config(request).await {
        Ok(op) => Ok(op),
        Err(error) => {
            let detail = error_info(error);
            // Any transport/timeout/unknown answer is not a rejection. Retain the
            // original transaction, query it once, and fail this probe either way.
            let unknown = detail["kind"] == "transport"
                || [1, 2, 4, 13, 14].contains(&(detail["code"].as_i64().unwrap_or(0) as i32));
            if unknown {
                let reconciliation = match client.get_runtime_config_status(lookup.clone()).await {
                    Ok(value) => json!({"status":status(&value)}),
                    Err(error) => json!({"error":error_info(error)}),
                };
                Err(
                    json!({"kind":"apply-outcome-unknown","transactionId":lookup.transaction_id,
                    "cause":detail,"reconciliation":reconciliation,"resubmitted":false}),
                )
            } else {
                Err(detail)
            }
        }
    }
}
async fn suite(client: &SingBoxApiClient, port: u16, request: &Value) -> ProbeResult<Value> {
    let before = client
        .get_runtime_capabilities()
        .await
        .map_err(error_info)?;
    check(
        before.contract_version == 1 && before.connected_hot_enabled && before.active.is_some(),
        "runtime-capabilities-not-enabled",
    )?;
    for secret in ["", "incorrect-package-loopback-credential"] {
        let unauth = SingBoxApiClient::connect(Endpoint::new("127.0.0.1", port), secret)
            .await
            .map_err(|_| json!({"kind":"negative-client-connect-failed"}))?;
        expect_code(unauth.get_runtime_capabilities().await, 16)?;
        expect_code(unauth.plan_runtime_config(Default::default()).await, 16)?;
        expect_code(unauth.apply_runtime_config(Default::default()).await, 16)?;
        expect_code(
            unauth.get_runtime_config_status(Default::default()).await,
            16,
        )?;
        expect_code(
            unauth.retry_runtime_persistence(Default::default()).await,
            16,
        )?;
        expect_code(
            unauth
                .acknowledge_runtime_persistence(Default::default())
                .await,
            16,
        )?;
    }
    let mut content: Value =
        serde_json::from_str(string(request, "configContent")?).map_err(|_| invalid())?;
    let original = d::PlanRuntimeConfigRequest {
        contract_version: 1,
        expected: before.active.clone(),
        config_content: serde_json::to_string(&content).map_err(|_| invalid())?,
        overrides: None,
        expected_control_revision: before.control_revision,
        selector_intents: vec![],
    };
    let noop_plan = client
        .plan_runtime_config(original.clone())
        .await
        .map_err(error_info)?;
    check(
        noop_plan.applicable && noop_plan.no_op && noop_plan.errors.is_empty(),
        "no-op-plan-not-observed",
    )?;
    let noop = apply_once(client, planned_apply(&noop_plan)).await?;
    check(
        noop.active == before.active
            && noop.before == noop.active
            && noop.control_revision == before.control_revision,
        "no-op-changed-runtime",
    )?;
    check(content["log"].is_object(), "actual-runtime-log-missing")?;
    let next = if content["log"]["level"] == "debug" {
        "warn"
    } else {
        "debug"
    };
    content["log"]["level"] = next.into();
    let changed = client
        .plan_runtime_config(d::PlanRuntimeConfigRequest {
            config_content: serde_json::to_string(&content).map_err(|_| invalid())?,
            ..original.clone()
        })
        .await
        .map_err(error_info)?;
    check(
        changed.applicable && !changed.no_op && changed.errors.is_empty(),
        "changed-plan-not-applicable",
    )?;
    let apply = planned_apply(&changed);
    let op = apply_once(client, apply.clone()).await?;
    check(
        op.durable && op.persistence_mode == "exact_configuration_file",
        "suite-apply-not-durable",
    )?;
    let expected = before.active.as_ref().ok_or_else(invalid)?;
    let active = op.active.as_ref().ok_or_else(invalid)?;
    check(
        active.incarnation == expected.incarnation
            && active.applied_revision == expected.applied_revision + 1,
        "suite-apply-restarted-or-revision-differs",
    )?;
    // An intentional replay is tested only AFTER an acknowledged durable Apply.
    let replay = apply_once(client, apply.clone()).await?;
    check(
        replay.active == op.active && replay.transaction_id == op.transaction_id,
        "same-transaction-replay-differs",
    )?;
    let lookup = client
        .get_runtime_config_status(status_request(&op.transaction_id, &active.incarnation))
        .await
        .map_err(error_info)?;
    check(
        lookup.lookup_state == d::RuntimeLookupState::Found as i32
            && lookup
                .operation
                .as_ref()
                .is_some_and(|value| value.active == op.active),
        "original-transaction-status-differs",
    )?;
    let retry = client
        .retry_runtime_persistence(d::RuntimePersistenceRetryRequest {
            contract_version: 1,
            transaction_id: op.transaction_id.clone(),
            active: op.active.clone(),
            control_revision: op.control_revision,
            persistence_mode: op.persistence_mode.clone(),
        })
        .await
        .map_err(error_info)?;
    check(
        retry.durable && retry.active == op.active,
        "terminal-retry-republished-runtime",
    )?;
    expect_code(
        client
            .acknowledge_runtime_persistence(d::RuntimePersistenceAcknowledgementRequest {
                contract_version: 1,
                transaction_id: op.transaction_id.clone(),
                active: op.active.clone(),
                control_revision: op.control_revision,
                persistence_mode: "app_authoritative_regeneration".into(),
                desired_state_digest: "0".repeat(64),
                desired_state_revision: "fixture-not-an-owner-receipt".into(),
            })
            .await,
        9,
    )?;
    let mut wrong = apply;
    wrong.candidate_digest = "0".repeat(64);
    expect_code(client.apply_runtime_config(wrong).await, 9)?;
    let wrong_lookup = client
        .get_runtime_config_status(status_request(
            &op.transaction_id,
            "different-package-instance",
        ))
        .await
        .map_err(error_info)?;
    check(
        wrong_lookup.lookup_state == d::RuntimeLookupState::WrongIncarnation as i32,
        "wrong-incarnation-not-rejected",
    )?;
    expect_code(client.plan_runtime_config(original).await, 9)?;
    let after = client
        .get_runtime_capabilities()
        .await
        .map_err(error_info)?;
    check(
        after.active == op.active && after.control_revision == op.control_revision,
        "negative-cases-mutated-runtime",
    )?;
    Ok(
        json!({"methods":METHODS,"unauthenticatedPerMethod":2,"before":capabilities(&before),
        "noOpOperation":operation(&noop),"operation":operation(&op),"terminalRetry":operation(&retry),
        "sameTransactionReplay":true,"cliRegenerationAckRejected":true,"wrongDigestReplayRejected":true,
        "stalePlanRejected":true,"wrongIncarnationLookup":wrong_lookup.lookup_state,
        "after":capabilities(&after),"logLevelAfter":next,
        "positiveRegenerationAck":"not covered; requires real libbox owner"}),
    )
}

async fn execute(input: &Value) -> ProbeResult<Value> {
    check(
        string(input, "host")? == "127.0.0.1",
        "literal-loopback-host-required",
    )?;
    let port = u16::try_from(number(input, "port")?).map_err(|_| invalid())?;
    check(port > 0, "nonzero-listener-port-required")?;
    let secret = string(input, "secret")?;
    check(
        (16..=512).contains(&secret.len()) && secret.bytes().all(|b| (0x21..=0x7e).contains(&b)),
        "private-secret-required",
    )?;
    verify_owned(input, port, true)?;
    let client = SingBoxApiClient::connect(Endpoint::new("127.0.0.1", port), secret)
        .await
        .map_err(|_| json!({"kind":"connect-failed"}))?;
    let empty_request = Value::Null;
    let request = input.get("request").unwrap_or(&empty_request);
    let result = match string(input, "action")? {
        "capabilities" | "tun-capabilities" => capabilities(
            &client
                .get_runtime_capabilities()
                .await
                .map_err(error_info)?,
        ),
        "plan" => plan(
            &client
                .plan_runtime_config(plan_request(request)?)
                .await
                .map_err(error_info)?,
        ),
        "apply" => operation(&apply_once(&client, apply_request(request)?).await?),
        "status" | "tun-status" => status(
            &client
                .get_runtime_config_status(status_request(
                    string(request, "transactionId")?,
                    string(request, "runtimeIncarnation")?,
                ))
                .await
                .map_err(error_info)?,
        ),
        "retry" => operation(
            &client
                .retry_runtime_persistence(d::RuntimePersistenceRetryRequest {
                    contract_version: 1,
                    transaction_id: string(request, "transactionId")?.into(),
                    active: id(&request["active"])?,
                    control_revision: number(request, "controlRevision")?,
                    persistence_mode: string(request, "persistenceMode")?.into(),
                })
                .await
                .map_err(error_info)?,
        ),
        "ack" => operation(
            &client
                .acknowledge_runtime_persistence(d::RuntimePersistenceAcknowledgementRequest {
                    contract_version: 1,
                    transaction_id: string(request, "transactionId")?.into(),
                    active: id(&request["active"])?,
                    control_revision: number(request, "controlRevision")?,
                    persistence_mode: string(request, "persistenceMode")?.into(),
                    desired_state_digest: string(request, "desiredStateDigest")?.into(),
                    desired_state_revision: string(request, "desiredStateRevision")?.into(),
                })
                .await
                .map_err(error_info)?,
        ),
        "suite" => suite(&client, port, request).await?,
        _ => return Err(invalid()),
    };
    verify_owned(input, port, false)?;
    Ok(result)
}

#[tokio::main]
async fn main() {
    let mut raw = Vec::new();
    let input = if std::env::args_os().len() == 1
        && std::io::stdin()
            .take(MAX_INPUT + 1)
            .read_to_end(&mut raw)
            .is_ok()
        && raw.len() as u64 <= MAX_INPUT
    {
        serde_json::from_slice::<Value>(&raw).ok()
    } else {
        None
    };
    let Some(input) = input else {
        println!(
            "{}",
            json!({"schema":"polaris-package-rpc-probe-v1","ok":false,"error":invalid()})
        );
        std::process::exit(1);
    };
    // Never echo an arbitrary action string from untrusted stdin.
    let action = input["action"]
        .as_str()
        .filter(|s| {
            [
                "capabilities",
                "plan",
                "apply",
                "status",
                "retry",
                "ack",
                "suite",
                "tun-capabilities",
                "tun-status",
            ]
            .contains(s)
        })
        .unwrap_or("invalid");
    let output = match tokio::time::timeout(std::time::Duration::from_secs(90), execute(&input))
        .await
    {
        Ok(Ok(result)) => {
            json!({"schema":"polaris-package-rpc-probe-v1","ok":true,"action":action,"result":result})
        }
        Ok(Err(error)) => {
            json!({"schema":"polaris-package-rpc-probe-v1","ok":false,"action":action,"error":error})
        }
        Err(_) => {
            json!({"schema":"polaris-package-rpc-probe-v1","ok":false,"action":action,"error":{"kind":"suite-deadline"}})
        }
    };
    let success = output["ok"] == true;
    println!("{output}");
    if !success {
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tun_authority_is_root_only_and_read_only() {
        for action in [
            "capabilities",
            "plan",
            "apply",
            "status",
            "retry",
            "ack",
            "suite",
        ] {
            assert!(authority(action, "0", Some(1001)).is_err());
            assert_eq!(authority(action, "1001", None).unwrap(), false);
        }
        for action in ["tun-capabilities", "tun-status"] {
            assert_eq!(authority(action, "0", Some(1001)).unwrap(), true);
            for uid in [None, Some(0), Some(u64::MAX)] {
                assert!(authority(action, "0", uid).is_err());
            }
            assert!(authority(action, "1001", Some(1001)).is_err());
        }
    }
    #[test]
    fn proc_identity_parser_handles_parenthesized_names() {
        let stat = format!(
            "42 (odd ) name) S {} 998",
            (1..=18)
                .map(|n| n.to_string())
                .collect::<Vec<_>>()
                .join(" ")
        );
        assert_eq!(start_time(&stat), Some("998"));
        assert_eq!(start_time("broken"), None);
        assert_eq!(
            uid("Name:\tprobe\nUid:\t1000\t1000\t1000\t1000\n"),
            Some("1000")
        );
    }
    #[test]
    fn errors_do_not_echo_rpc_messages_or_config() {
        let error = RuntimeConfigError::Call(ClientError::Status(
            polaris_singbox_grpc::tonic::Status::unauthenticated("SECRET-CONFIG"),
        ));
        assert_eq!(error_info(error), json!({"kind":"rpc","code":16}));
        assert!(expect_code::<()>(Ok(()), 16).is_err());
    }
    #[test]
    fn json_request_preserves_exact_transaction_identity() {
        let raw = json!({"transactionId":"transaction","planId":"plan","expected":{"incarnation":"instance","appliedRevision":7,"effectiveDigest":"digest"},
            "candidateDigest":"candidate","acceptedImpactDigest":"impact","expectedControlRevision":8});
        let parsed = apply_request(&raw).unwrap();
        assert_eq!(parsed.transaction_id, "transaction");
        assert_eq!(parsed.expected.unwrap().applied_revision, 7);
        assert_eq!(parsed.expected_control_revision, 8);
        assert!(apply_request(&json!({})).is_err());
    }
}
