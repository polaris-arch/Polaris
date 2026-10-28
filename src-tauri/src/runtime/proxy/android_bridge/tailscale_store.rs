use polaris_mesh::tailscale_state::retirement::{ObservedTailscaleStoreRun, TailscaleStoreExport};

const UNKNOWN: &str = "nativeRetirementUnknown";
const MAX_BYTES: usize = 1_048_576;

#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct StoreData {
    pub contract_version: String,
    pub producer_kind: String,
    pub process_nonce: String,
    pub native_ticket_id: String,
    pub logical_instance_id: String,
    pub main_birth_nonce: String,
    pub actual_config_digest: String,
    pub original_observed_runs: Vec<ObservedTailscaleStoreRun>,
    pub history_unknown: bool,
    pub terminal: bool,
    pub store_retirement: String,
}

#[derive(Clone, Debug)]
pub(crate) struct AndroidStoreCustody {
    original: StoreData,
}

fn hex64(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn native_id(value: &str, max: usize) -> bool {
    !value.is_empty()
        && value.len() <= max
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._:-".contains(&byte))
}

impl StoreData {
    fn export(&self, terminal: bool) -> Result<TailscaleStoreExport, String> {
        if self.contract_version != "polaris-android-ts-store-custody-v1"
            || !matches!(self.producer_kind.as_str(), "Main" | "Login" | "Speedtest")
            || !native_id(&self.process_nonce, 128)
            || !native_id(&self.native_ticket_id, 128)
            || !native_id(&self.logical_instance_id, 256)
            || (self.producer_kind == "Main") != !self.main_birth_nonce.is_empty()
            || (!self.main_birth_nonce.is_empty() && !native_id(&self.main_birth_nonce, 128))
            || !hex64(&self.actual_config_digest)
            || self.history_unknown
            || self.store_retirement.len() > MAX_BYTES
            || (terminal && !self.terminal)
            || self
                .original_observed_runs
                .last()
                .is_none_or(|run| run.config_digest != self.actual_config_digest)
        {
            return Err(UNKNOWN.into());
        }
        let export: TailscaleStoreExport =
            serde_json::from_str(&self.store_retirement).map_err(|_| UNKNOWN.to_owned())?;
        export
            .validate_observed_runs(&self.original_observed_runs, terminal)
            .map_err(|_| UNKNOWN.to_owned())?;
        Ok(export)
    }

    fn related(&self, file: &str) -> Result<bool, String> {
        let export = self.export(false)?;
        if !self.terminal || export.instances().iter().any(|run| !run.census_complete) {
            return Err(UNKNOWN.into());
        }
        let related = export
            .instances()
            .iter()
            .any(|run| run.nodes.iter().any(|node| node.state_file == file));
        if related {
            export
                .validate_observed_runs(&self.original_observed_runs, true)
                .map_err(|_| UNKNOWN.to_owned())?;
        }
        Ok(related)
    }

    fn same_original(&self, original: &Self) -> bool {
        self.contract_version == original.contract_version
            && self.producer_kind == original.producer_kind
            && self.process_nonce == original.process_nonce
            && self.native_ticket_id == original.native_ticket_id
            && self.logical_instance_id == original.logical_instance_id
            && self.main_birth_nonce == original.main_birth_nonce
            && self.actual_config_digest == original.actual_config_digest
            && self.original_observed_runs == original.original_observed_runs
    }
}

impl AndroidStoreCustody {
    pub(crate) fn original(&self) -> &StoreData {
        &self.original
    }
    pub(crate) fn retired_export(&self) -> Result<TailscaleStoreExport, String> {
        self.original.export(true)
    }
    pub(crate) fn binding_json(&self) -> Result<String, String> {
        serde_json::to_string(&self.original).map_err(|_| UNKNOWN.to_owned())
    }
}

pub(super) fn decode_store(
    raw: &str,
    original: Option<&AndroidStoreCustody>,
) -> Result<AndroidStoreCustody, String> {
    if raw.len() > MAX_BYTES {
        return Err(UNKNOWN.into());
    }
    let data: StoreData = serde_json::from_str(raw).map_err(|_| UNKNOWN.to_owned())?;
    data.export(false)?;
    if original.is_some_and(|previous| !data.same_original(&previous.original)) {
        return Err(UNKNOWN.into());
    }
    Ok(AndroidStoreCustody { original: data })
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Membership {
    contract_version: String,
    #[serde(rename = "requestID")]
    request_id: String,
    config_digest: String,
    run_nonce: String,
    membership_state: String,
    targets: Vec<polaris_mesh::tailscale_state::retirement::ObservedTailscaleStoreScope>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Validation {
    native_contract_version: String,
    validation: String,
    cleanup: String,
    #[serde(rename = "requestID")]
    request_id: String,
    config_digest: String,
    membership: String,
    store_retirement: String,
}

impl Validation {
    fn related(&self, ticket: &str, file: &str) -> Result<bool, String> {
        if self.native_contract_version != "polaris-validation-v1"
            || !matches!(
                self.validation.as_str(),
                "Accepted" | "Rejected" | "InternalFailure"
            )
            || !matches!(
                self.cleanup.as_str(),
                "NoConstruction" | "CleanupUnknown" | "DisposedExact"
            )
            || self.request_id != ticket
            || !hex64(&self.config_digest)
            || self.membership.len() > MAX_BYTES
            || self.store_retirement.len() > MAX_BYTES
        {
            return Err(UNKNOWN.into());
        }
        let member: Membership =
            serde_json::from_str(&self.membership).map_err(|_| UNKNOWN.to_owned())?;
        if member.contract_version != "polaris-ts-store-target-membership-v1"
            || member.request_id != ticket
            || member.config_digest != self.config_digest
            || !matches!(member.membership_state.as_str(), "Complete" | "Unknown")
        {
            return Err(UNKNOWN.into());
        }
        let export: TailscaleStoreExport =
            serde_json::from_str(&self.store_retirement).map_err(|_| UNKNOWN.to_owned())?;
        let header: serde_json::Value =
            serde_json::from_str(&self.store_retirement).map_err(|_| UNKNOWN.to_owned())?;
        if header["contractVersion"] != "polaris-ts-auth-writer-retirement-v1"
            || header["globalCleanupEvidence"] != "CleanupUnknown"
        {
            return Err(UNKNOWN.into());
        }
        if self.validation == "Rejected" && self.cleanup == "NoConstruction" {
            if member.membership_state != "Unknown"
                || !member.targets.is_empty()
                || (!member.run_nonce.is_empty() && !hex64(&member.run_nonce))
            {
                return Err(UNKNOWN.into());
            }
            return Ok(false);
        }
        if self.validation != "Accepted"
            || member.membership_state != "Complete"
            || !hex64(&member.run_nonce)
        {
            return Err(UNKNOWN.into());
        }
        let observed = [ObservedTailscaleStoreRun {
            run_nonce: member.run_nonce,
            config_digest: member.config_digest,
            scopes: member.targets,
        }];
        let related = observed[0]
            .scopes
            .iter()
            .any(|scope| scope.state_file == file);
        export
            .validate_observed_runs(&observed, related)
            .map_err(|_| UNKNOWN.to_owned())?;
        Ok(related)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ValidationOwner {
    contract_version: String,
    process_nonce: String,
    native_ticket_id: String,
    validation: Validation,
}
#[derive(Clone, Debug)]
pub(crate) struct AndroidValidationCustody {
    original: ValidationOwner,
}
pub(crate) struct AndroidValidationCheck {
    verdict: super::ConfigCheckVerdict,
    custody: Option<AndroidValidationCustody>,
}
impl AndroidValidationCheck {
    pub(crate) fn verdict(&self) -> &super::ConfigCheckVerdict {
        &self.verdict
    }
    pub(crate) fn custody(&self) -> Option<&AndroidValidationCustody> {
        self.custody.as_ref()
    }
}
pub(super) fn decode_validation(
    raw: &str,
    digest: &str,
) -> Result<AndroidValidationCustody, String> {
    if raw.len() > MAX_BYTES {
        return Err(UNKNOWN.into());
    }
    let original: ValidationOwner = serde_json::from_str(raw).map_err(|_| UNKNOWN.to_owned())?;
    if original.contract_version != "polaris-android-validation-custody-v1"
        || !native_id(&original.process_nonce, 128)
        || !native_id(&original.native_ticket_id, 128)
        || original.validation.config_digest != digest
    {
        return Err(UNKNOWN.into());
    }
    // For an Accepted locator, this also checks exact nonempty membership and scoped writers.
    original.validation.related(
        &original.native_ticket_id,
        "/__polaris_unselected__/tailscaled.state",
    )?;
    Ok(AndroidValidationCustody { original })
}
#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WarmTupleData {
    contract_version: String,
    validation: ValidationOwner,
    state_file: String,
    action_request_id: String,
    logical_instance_id: String,
}
/// Intent recorded before a native begin; never a birth, filesystem or held-action proof.
#[derive(Clone, Debug)]
pub(crate) struct AndroidWarmTuple {
    original: WarmTupleData,
}
impl AndroidWarmTuple {
    pub(crate) fn logical_instance_id(&self) -> &str {
        &self.original.logical_instance_id
    }
    pub(crate) fn state_file(&self) -> &str {
        &self.original.state_file
    }
    pub(crate) fn action_request_id(&self) -> &str {
        &self.original.action_request_id
    }
    pub(crate) fn config_digest(&self) -> &str {
        &self.original.validation.validation.config_digest
    }
    pub(super) fn binding_json(&self) -> Result<String, String> {
        serde_json::to_string(&self.original).map_err(|_| UNKNOWN.to_owned())
    }
}
pub(crate) fn make_warm_tuple(
    validation: &AndroidValidationCustody,
    file: &str,
    action: &str,
    warm_id: &str,
) -> Result<AndroidWarmTuple, String> {
    if !native_id(action, 128)
        || !native_id(warm_id, 256)
        || validation.original.validation.validation != "Accepted"
        || !validation
            .original
            .validation
            .related(&validation.original.native_ticket_id, file)?
    {
        return Err(UNKNOWN.into());
    }
    Ok(AndroidWarmTuple {
        original: WarmTupleData {
            contract_version: "polaris-android-ts-cold-warm-v1".into(),
            validation: validation.original.clone(),
            state_file: file.into(),
            action_request_id: action.into(),
            logical_instance_id: warm_id.into(),
        },
    })
}
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WarmResponse {
    tuple: WarmTupleData,
    native_ticket_id: String,
    global_state: String,
    held: bool,
    claimed: bool,
    child_registration_open: bool,
    child_native_ticket_id: String,
    child_global_state: String,
    runtime: Option<StoreData>,
    family: Option<NativeTargetSnapshot>,
}
/// Original Entry is still manageable when engine/store construction produced no runtime payload.
pub(crate) struct AndroidWarmReservation {
    tuple: AndroidWarmTuple,
    runtime: Option<AndroidStoreCustody>,
    family: Option<TargetSnapshot>,
}
impl AndroidWarmReservation {
    pub(crate) fn runtime_custody(&self) -> Option<&AndroidStoreCustody> {
        self.runtime.as_ref()
    }
    pub(crate) fn held_retirement(&self) -> Result<(AndroidStoreCustody, TargetSnapshot), String> {
        let runtime = self.runtime.as_ref().ok_or(UNKNOWN)?;
        runtime.retired_export()?;
        let family = self.family.as_ref().ok_or(UNKNOWN)?;
        if family._action_request_id != self.tuple.action_request_id()
            || family._state_file != self.tuple.state_file()
        {
            return Err(UNKNOWN.into());
        }
        Ok((
            runtime.clone(),
            TargetSnapshot {
                _native_ticket_id: family._native_ticket_id.clone(),
                _action_request_id: family._action_request_id.clone(),
                _state_file: family._state_file.clone(),
            },
        ))
    }
}
pub(super) fn decode_warm(
    raw: &str,
    tuple: &AndroidWarmTuple,
) -> Result<AndroidWarmReservation, String> {
    if raw.len() > MAX_BYTES {
        return Err(UNKNOWN.into());
    }
    let response: WarmResponse = serde_json::from_str(raw).map_err(|_| UNKNOWN.to_owned())?;
    if response.tuple != tuple.original
        || !native_id(&response.native_ticket_id, 128)
        || !matches!(
            response.global_state.as_str(),
            "Reserved" | "BirthEntered" | "CancelledBeforeBirth" | "Unknown"
        )
        || (!response.claimed
            && (response.child_registration_open || !response.child_native_ticket_id.is_empty()))
        || (response.child_native_ticket_id.is_empty() != response.child_global_state.is_empty())
        || (!response.child_native_ticket_id.is_empty()
            && (!native_id(&response.child_native_ticket_id, 128)
                || !matches!(
                    response.child_global_state.as_str(),
                    "Reserved"
                        | "BirthEntered"
                        | "CancelledBeforeBirth"
                        | "ValidationCleanupUnknown"
                )))
    {
        return Err(UNKNOWN.into());
    }
    let runtime = if let Some(data) = response.runtime {
        if !response.claimed
            || response.child_registration_open
            || response.child_native_ticket_id.is_empty()
            || !matches!(response.global_state.as_str(), "BirthEntered" | "Unknown")
            || data.process_nonce != tuple.original.validation.process_nonce
            || data.native_ticket_id != response.native_ticket_id
            || data.producer_kind != "Login"
            || data.logical_instance_id != tuple.logical_instance_id()
            || !data.main_birth_nonce.is_empty()
            || (!data.actual_config_digest.is_empty()
                && data.actual_config_digest != tuple.config_digest())
        {
            return Err(UNKNOWN.into());
        }
        if data.export(false).is_ok() {
            Some(AndroidStoreCustody { original: data })
        } else {
            None
        }
    } else {
        None
    };
    let family = if response.held {
        match (response.family, runtime.as_ref()) {
            (Some(family), Some(original)) => Some(decode_target(
                &serde_json::to_string(&family).map_err(|_| UNKNOWN.to_owned())?,
                original,
                tuple.state_file(),
                tuple.action_request_id(),
            )?),
            (Some(_), None) => return Err(UNKNOWN.into()),
            _ => None,
        }
    } else {
        if response.family.is_some() {
            return Err(UNKNOWN.into());
        }
        None
    };
    Ok(AndroidWarmReservation {
        tuple: tuple.clone(),
        runtime,
        family,
    })
}

#[derive(serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct NativeEntry {
    native_ticket_id: String,
    producer_kind: String,
    logical_instance_id: String,
    global_state: String,
    related: bool,
    store: Option<StoreData>,
    validation: Option<Validation>,
    parent_native_ticket_id: Option<String>,
}

#[derive(serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct NativeTargetSnapshot {
    process_nonce: String,
    revision: u64,
    action_request_id: String,
    state_file: String,
    entries: Vec<NativeEntry>,
}

/// Only this module accepts the original native family's action response.
pub(crate) struct TargetSnapshot {
    _native_ticket_id: String,
    _action_request_id: String,
    _state_file: String,
}

pub(super) fn decode_target(
    raw: &str,
    original: &AndroidStoreCustody,
    file: &str,
    action: &str,
) -> Result<TargetSnapshot, String> {
    if raw.len() > MAX_BYTES {
        return Err(UNKNOWN.into());
    }
    let snapshot: NativeTargetSnapshot =
        serde_json::from_str(raw).map_err(|_| UNKNOWN.to_owned())?;
    if snapshot.process_nonce != original.original.process_nonce
        || snapshot.state_file != file
        || snapshot.action_request_id != action
        || !native_id(action, 128)
        || snapshot.revision == 0
        || snapshot.entries.is_empty()
        || !original
            .original
            .original_observed_runs
            .iter()
            .any(|run| run.scopes.iter().any(|scope| scope.state_file == file))
    {
        return Err(UNKNOWN.into());
    }
    let mut ids = std::collections::BTreeSet::new();
    let mut selected = false;
    for entry in &snapshot.entries {
        if !native_id(&entry.native_ticket_id, 128)
            || !native_id(&entry.logical_instance_id, 256)
            || !matches!(
                entry.producer_kind.as_str(),
                "Main"
                    | "Login"
                    | "Speedtest"
                    | "CheckConfig"
                    | "TargetlessStop"
                    | "TargetlessReload"
            )
            || !ids.insert(&entry.native_ticket_id)
        {
            return Err(UNKNOWN.into());
        }
        let related = if entry.global_state == "CancelledBeforeBirth" {
            if entry.validation.is_some()
                || entry.parent_native_ticket_id.is_some()
                || entry.store.as_ref().is_some_and(|data| {
                    data.contract_version != "polaris-android-ts-store-custody-v1"
                        || data.history_unknown
                        || data.terminal
                        || (data.producer_kind == "Main") == data.main_birth_nonce.is_empty()
                        || data.native_ticket_id != entry.native_ticket_id
                        || data.producer_kind != entry.producer_kind
                        || data.logical_instance_id != entry.logical_instance_id
                        || data.process_nonce != snapshot.process_nonce
                        || !data.original_observed_runs.is_empty()
                        || !data.actual_config_digest.is_empty()
                        || !data.store_retirement.is_empty()
                })
            {
                return Err(UNKNOWN.into());
            }
            false
        } else if entry.producer_kind == "CheckConfig" {
            if entry.global_state != "ValidationCleanupUnknown"
                || entry.store.is_some()
                || entry.parent_native_ticket_id.is_some()
            {
                return Err(UNKNOWN.into());
            }
            entry
                .validation
                .as_ref()
                .ok_or(UNKNOWN)?
                .related(&entry.native_ticket_id, file)?
        } else {
            if entry.validation.is_some()
                || matches!(entry.global_state.as_str(), "Reserved" | "BirthEntered")
            {
                return Err(UNKNOWN.into());
            }
            let data = if matches!(
                entry.producer_kind.as_str(),
                "TargetlessStop" | "TargetlessReload"
            ) {
                if !matches!(entry.global_state.as_str(), "Completed" | "Unknown")
                    || entry.store.is_some()
                {
                    return Err(UNKNOWN.into());
                }
                let parent = snapshot
                    .entries
                    .iter()
                    .find(|candidate| {
                        Some(&candidate.native_ticket_id) == entry.parent_native_ticket_id.as_ref()
                            && candidate.producer_kind == "Main"
                    })
                    .ok_or(UNKNOWN)?;
                parent.store.as_ref().ok_or(UNKNOWN)?
            } else {
                if !matches!(entry.global_state.as_str(), "ClosedExact" | "Unknown") {
                    return Err(UNKNOWN.into());
                }
                let data = entry.store.as_ref().ok_or(UNKNOWN)?;
                if data.producer_kind != entry.producer_kind
                    || data.native_ticket_id != entry.native_ticket_id
                    || data.logical_instance_id != entry.logical_instance_id
                    || data.process_nonce != snapshot.process_nonce
                    || entry.parent_native_ticket_id.is_some()
                {
                    return Err(UNKNOWN.into());
                }
                data
            };
            let related = data.related(file)?;
            if data.native_ticket_id == original.original.native_ticket_id {
                if !data.same_original(&original.original) {
                    return Err(UNKNOWN.into());
                }
                selected = true;
            }
            related
        };
        if related != entry.related {
            return Err(UNKNOWN.into());
        }
    }
    if !selected {
        return Err(UNKNOWN.into());
    }
    Ok(TargetSnapshot {
        _native_ticket_id: original.original.native_ticket_id.clone(),
        _action_request_id: snapshot.action_request_id,
        _state_file: snapshot.state_file,
    })
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct TailscaleStoreArgs {
    pub operation: String,
    pub binding: Option<String>,
    pub instance_id: Option<String>,
    pub expected_actual_config_digest: Option<String>,
    pub run_id: Option<String>,
    pub birth_nonce: Option<String>,
    pub state_file: Option<String>,
    pub action_request_id: Option<String>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ScopedStoreResponse {
    pub envelope: String,
}

#[cfg(target_os = "android")]
async fn call(args: TailscaleStoreArgs) -> Result<String, String> {
    let plugin = super::plugin_handle().map_err(|_| UNKNOWN.to_owned())?;
    super::call_with_budget::<ScopedStoreResponse, _>(
        plugin,
        "tailscaleStoreCustody",
        TailscaleStoreArgs {
            operation: args.operation,
            binding: args.binding,
            instance_id: args.instance_id,
            expected_actual_config_digest: args.expected_actual_config_digest,
            run_id: args.run_id,
            birth_nonce: args.birth_nonce,
            state_file: args.state_file,
            action_request_id: args.action_request_id,
        },
        super::STOP_TIMEOUT,
        None,
    )
    .await
    .map(|response| response.envelope)
    .map_err(|_| UNKNOWN.to_owned())
}

#[cfg(target_os = "android")]
fn args(operation: &str) -> TailscaleStoreArgs {
    TailscaleStoreArgs {
        operation: operation.into(),
        binding: None,
        instance_id: None,
        expected_actual_config_digest: None,
        run_id: None,
        birth_nonce: None,
        state_file: None,
        action_request_id: None,
    }
}

pub(super) fn decode_validation_check(
    response: super::CheckResponse,
    digest: &str,
) -> Result<AndroidValidationCheck, super::CapacityClosed> {
    let custody = response
        .tailscale_validation_envelope
        .as_deref()
        .and_then(|raw| decode_validation(raw, digest).ok());
    let verdict = super::check_response_verdict(response)?;
    Ok(AndroidValidationCheck { verdict, custody })
}
#[cfg(target_os = "android")]
pub(crate) async fn check_config_for_tailscale(
    config: &str,
) -> Result<AndroidValidationCheck, super::CapacityClosed> {
    match super::check_config_reply(config).await {
        Ok(response) => decode_validation_check(
            response,
            &polaris_updater::verify::sha256_hex(config.as_bytes()),
        ),
        Err(error) => Ok(AndroidValidationCheck {
            verdict: super::verdict_from_libbox_check(Err(error)),
            custody: None,
        }),
    }
}

#[cfg(target_os = "android")]
async fn warm_call(
    tuple: &AndroidWarmTuple,
    operation: &str,
    request_id: Option<&str>,
) -> Result<AndroidWarmReservation, String> {
    let mut request = args(operation);
    request.binding = Some(tuple.binding_json()?);
    request.action_request_id = request_id.map(str::to_owned);
    decode_warm(&call(request).await?, tuple)
}
#[cfg(target_os = "android")]
pub(crate) async fn begin_warm(tuple: &AndroidWarmTuple) -> Result<AndroidWarmReservation, String> {
    warm_call(tuple, "beginWarm", None).await
}
#[cfg(target_os = "android")]
pub(crate) async fn read_warm(tuple: &AndroidWarmTuple) -> Result<AndroidWarmReservation, String> {
    warm_call(tuple, "readWarm", None).await
}
#[cfg(target_os = "android")]
pub(crate) async fn close_warm(
    tuple: &AndroidWarmTuple,
    request: &str,
) -> Result<AndroidWarmReservation, String> {
    warm_call(tuple, "closeWarm", Some(request)).await
}
pub(super) fn decode_warm_release(raw: &str, tuple: &AndroidWarmTuple) -> Result<(), String> {
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    struct Released {
        released: bool,
        before_birth: bool,
        process_nonce: String,
        native_ticket_id: String,
        action_request_id: String,
        state_file: String,
    }
    if raw.len() > MAX_BYTES {
        return Err(UNKNOWN.into());
    }
    let reply: Released = serde_json::from_str(raw).map_err(|_| UNKNOWN.to_owned())?;
    if !reply.released
        || reply.process_nonce != tuple.original.validation.process_nonce
        || (if reply.before_birth {
            !reply.native_ticket_id.is_empty()
        } else {
            !native_id(&reply.native_ticket_id, 128)
        })
        || reply.action_request_id != tuple.action_request_id()
        || reply.state_file != tuple.state_file()
    {
        return Err(UNKNOWN.into());
    }
    Ok(())
}

#[cfg(target_os = "android")]
pub(crate) async fn finish_warm(tuple: &AndroidWarmTuple) -> Result<(), String> {
    let mut request = args("finishWarm");
    request.binding = Some(tuple.binding_json()?);
    decode_warm_release(&call(request).await?, tuple)
}

#[cfg(target_os = "android")]
pub(crate) async fn observe_login(id: &str, digest: &str) -> Result<AndroidStoreCustody, String> {
    let mut request = args("observeLogin");
    request.instance_id = Some(id.into());
    request.expected_actual_config_digest = Some(digest.into());
    let observed = decode_store(&call(request).await?, None)?;
    if observed.original.producer_kind != "Login"
        || observed.original.logical_instance_id != id
        || observed.original.actual_config_digest != digest
    {
        return Err(UNKNOWN.into());
    }
    Ok(observed)
}

#[cfg(target_os = "android")]
pub(crate) async fn observe_main(
    target: Option<&super::AndroidExactTarget>,
) -> Result<AndroidStoreCustody, String> {
    let mut request = args("observeMain");
    if let Some(target) = target {
        request.run_id = Some(target.run_id.clone());
        request.birth_nonce = Some(target.birth_nonce.clone());
    }
    let observed = decode_store(&call(request).await?, None)?;
    if observed.original.producer_kind != "Main"
        || target.is_some_and(|target| {
            observed.original.logical_instance_id != target.run_id
                || observed.original.main_birth_nonce != target.birth_nonce
        })
    {
        return Err(UNKNOWN.into());
    }
    Ok(observed)
}

#[cfg(target_os = "android")]
pub(crate) async fn read_main_retirement(
    original: &AndroidStoreCustody,
) -> Result<AndroidStoreCustody, String> {
    let mut request = args("readMain");
    request.binding = Some(original.binding_json()?);
    let retired = decode_store(&call(request).await?, Some(original))?;
    retired.retired_export()?;
    Ok(retired)
}

#[cfg(target_os = "android")]
pub(crate) async fn read_login_retirement(
    original: &AndroidStoreCustody,
) -> Result<AndroidStoreCustody, String> {
    if original.original.producer_kind != "Login" {
        return Err(UNKNOWN.into());
    }
    let mut request = args("readLogin");
    request.binding = Some(original.binding_json()?);
    let retired = decode_store(&call(request).await?, Some(original))?;
    retired.retired_export()?;
    Ok(retired)
}

#[cfg(target_os = "android")]
pub(crate) async fn close_login(
    original: &AndroidStoreCustody,
    request_id: &str,
) -> Result<AndroidStoreCustody, String> {
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    struct TailscaleStoreClosed {
        original_close_request_id: String,
        ordinary_close_state: String,
        store: StoreData,
    }
    let mut request = args("closeLogin");
    request.binding = Some(original.binding_json()?);
    request.action_request_id = Some(request_id.into());
    let raw = call(request).await?;
    if raw.len() > MAX_BYTES {
        return Err(UNKNOWN.into());
    }
    let closed: TailscaleStoreClosed =
        serde_json::from_str(&raw).map_err(|_| UNKNOWN.to_owned())?;
    if closed.original_close_request_id != request_id
        || closed.ordinary_close_state != "Closed"
        || !closed.store.same_original(&original.original)
    {
        return Err(UNKNOWN.into());
    }
    let retired = AndroidStoreCustody {
        original: closed.store,
    };
    retired.retired_export()?;
    Ok(retired)
}

#[cfg(target_os = "android")]
pub(crate) async fn target_action(
    original: &AndroidStoreCustody,
    file: &str,
    action: &str,
    begin: bool,
) -> Result<TargetSnapshot, String> {
    let mut request = args(if begin { "begin" } else { "query" });
    request.binding = Some(original.binding_json()?);
    request.state_file = Some(file.into());
    request.action_request_id = Some(action.into());
    decode_target(&call(request).await?, original, file, action)
}

#[cfg(target_os = "android")]
pub(crate) async fn finish_action(
    original: &AndroidStoreCustody,
    file: &str,
    action: &str,
) -> Result<(), String> {
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    struct Released {
        released: bool,
        process_nonce: String,
        native_ticket_id: String,
        action_request_id: String,
        state_file: String,
    }
    let mut request = args("finish");
    request.binding = Some(original.binding_json()?);
    request.state_file = Some(file.into());
    request.action_request_id = Some(action.into());
    let raw = call(request).await?;
    let receipt: Released = serde_json::from_str(&raw).map_err(|_| UNKNOWN.to_owned())?;
    if !receipt.released
        || receipt.process_nonce != original.original.process_nonce
        || receipt.native_ticket_id != original.original.native_ticket_id
        || receipt.action_request_id != action
        || receipt.state_file != file
    {
        return Err(UNKNOWN.into());
    }
    Ok(())
}
