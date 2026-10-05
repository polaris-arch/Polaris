//! Explicit manual check: one saved-config prerequisite, then the existing IP/unlock detectors.
//! The normal connection is owned by proxy lifecycle, never by this action's cancellation/deadline.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use async_trait::async_trait;
use polaris_config_engine::user_config::app_config::UserConfig;
use polaris_config_engine::user_config::dns_constants::is_sentinel_selection;
use polaris_net_stack::safe_redirect::{FetchInit, GuardedTarget, HttpClient, MinimalResponse};
use polaris_unlock::http::{UnlockHttp, UnlockRequest, UnlockResponse};
use polaris_unlock::UnlockSnapshot;
use serde::Serialize;
use serde_json::Value;
use tauri::{AppHandle, Manager};
use tokio::sync::Notify;

use crate::response::ApiResponse;
use crate::runtime::http::HttpRuntime;
use crate::runtime::proxy::{
    ActionBinding, LocalHttpProxy, MainPrerequisiteError, NormalMainAction, ProxyRuntime,
    ReadyMainTicket,
};
use crate::runtime::unlock::UnlockRuntime;
use crate::runtime::AppRuntime;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ManualCheckFailure {
    pub code: String,
}

impl ManualCheckFailure {
    pub(super) fn new(code: &str) -> Self {
        Self { code: code.into() }
    }
}

impl From<MainPrerequisiteError> for ManualCheckFailure {
    fn from(error: MainPrerequisiteError) -> Self {
        // Private producer diagnostics stay in logs; IPC only carries the cause classification.
        log::debug!("manual network prerequisite: {error}");
        Self::new(error.code())
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ManualCheckContext {
    request_id: String,
    main_generation: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    start_time: Option<u64>,
}

#[derive(Debug, Serialize)]
pub struct ManualCheckLeg<T> {
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<T>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<ManualCheckFailure>,
}

impl<T> From<Result<T, ManualCheckFailure>> for ManualCheckLeg<T> {
    fn from(result: Result<T, ManualCheckFailure>) -> Self {
        match result {
            Ok(data) => Self {
                data: Some(data),
                error: None,
            },
            Err(error) => Self {
                data: None,
                error: Some(error),
            },
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ManualCheckResponse {
    context: ManualCheckContext,
    ip_info: ManualCheckLeg<Value>,
    unlock: ManualCheckLeg<UnlockSnapshot>,
}

struct ManualRequest {
    request_id: String,
    commit_lock: Mutex<()>,
    cancelled: AtomicBool,
    changed: Notify,
}

impl ManualRequest {
    fn check(&self) -> Result<(), ManualCheckFailure> {
        if self.cancelled.load(Ordering::SeqCst) {
            Err(ManualCheckFailure::new("cancelled"))
        } else {
            Ok(())
        }
    }

    async fn cancelled(&self) {
        loop {
            let notified = self.changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.check().is_err() {
                return;
            }
            notified.await;
        }
    }
}

// One bounded, action-specific slot. It contains no normal-connection ownership or ready authority.
#[derive(Default)]
struct ManualSlot(Mutex<Option<Arc<ManualRequest>>>);

impl ManualSlot {
    fn admit(&self, request_id: String) -> Result<ManualLease<'_>, ManualCheckFailure> {
        if request_id.is_empty()
            || request_id.len() > 128
            || request_id.chars().any(char::is_control)
        {
            return Err(ManualCheckFailure::new("invalidRequest"));
        }
        let mut active = self
            .0
            .lock()
            .map_err(|_| ManualCheckFailure::new("readyUnknown"))?;
        if active.is_some() {
            return Err(ManualCheckFailure::new("manualCheckBusy"));
        }
        let request = Arc::new(ManualRequest {
            request_id,
            commit_lock: Mutex::new(()),
            cancelled: AtomicBool::new(false),
            changed: Notify::new(),
        });
        *active = Some(Arc::clone(&request));
        Ok(ManualLease {
            slot: self,
            request,
        })
    }

    fn cancel(&self, request_id: &str) {
        if let Ok(active) = self.0.lock() {
            if let Some(request) = active
                .as_ref()
                .filter(|request| request.request_id == request_id)
            {
                if let Ok(_commit) = request.commit_lock.lock() {
                    request.cancelled.store(true, Ordering::SeqCst);
                }
                request.changed.notify_waiters();
            }
        }
    }
}

struct ManualLease<'a> {
    slot: &'a ManualSlot,
    request: Arc<ManualRequest>,
}

impl Drop for ManualLease<'_> {
    fn drop(&mut self) {
        if let Ok(mut active) = self.slot.0.lock() {
            if active
                .as_ref()
                .is_some_and(|request| Arc::ptr_eq(request, &self.request))
            {
                *active = None;
            }
        }
    }
}

fn manual_slot() -> &'static ManualSlot {
    static SLOT: OnceLock<ManualSlot> = OnceLock::new();
    SLOT.get_or_init(ManualSlot::default)
}

/// Detectors inject this narrow check at their actual I/O and commit boundaries.
/// Production authority comes exclusively from the producer's private ReadyMainTicket.
#[async_trait]
pub(super) trait ManualBindingGuard: Send + Sync {
    fn check(&self) -> Result<(), ManualCheckFailure>;
    /// Local detector predicates only; never reenter producer/config while committing.
    fn check_local(&self) -> Result<(), ManualCheckFailure> {
        Ok(())
    }
    fn commit<T>(
        &self,
        commit: impl FnOnce() -> Result<T, ManualCheckFailure>,
    ) -> Result<T, ManualCheckFailure> {
        self.check()?;
        commit()
    }
    async fn validate(&self) -> Result<(), ManualCheckFailure> {
        self.check()
    }
}

#[derive(Clone)]
struct BoundMain {
    proxy: Arc<ProxyRuntime>,
    ticket: ReadyMainTicket,
    request: Arc<ManualRequest>,
    failure: Arc<Mutex<Option<ManualCheckFailure>>>,
}

#[async_trait]
impl ManualBindingGuard for BoundMain {
    fn commit<T>(
        &self,
        commit: impl FnOnce() -> Result<T, ManualCheckFailure>,
    ) -> Result<T, ManualCheckFailure> {
        let result = self
            .proxy
            .with_ready_main_commit(&self.ticket, || {
                let _cancel = self
                    .request
                    .commit_lock
                    .lock()
                    .map_err(|_| ManualCheckFailure::new("readyUnknown"))?;
                self.request.check()?;
                let failure = self
                    .failure
                    .lock()
                    .map_err(|_| ManualCheckFailure::new("readyUnknown"))?;
                if let Some(error) = failure.as_ref() {
                    return Err(error.clone());
                }
                let result = commit();
                drop(failure);
                // Detector-local rejection must not poison the independent action leg.
                Ok(result)
            })
            .map_err(ManualCheckFailure::from)
            .and_then(|result| result);
        if let Err(error) = &result {
            if let Ok(mut failure) = self.failure.lock() {
                failure.get_or_insert_with(|| error.clone());
            }
        }
        // Only producer/config/action errors above are shared and sticky.
        result.and_then(|result| result)
    }
    fn check(&self) -> Result<(), ManualCheckFailure> {
        self.request.check()?;
        if let Some(error) = self
            .failure
            .lock()
            .map_err(|_| ManualCheckFailure::new("readyUnknown"))?
            .clone()
        {
            return Err(error);
        }
        self.proxy.check_ready_main(&self.ticket).map_err(|error| {
            let error = ManualCheckFailure::from(error);
            if let Ok(mut failure) = self.failure.lock() {
                failure.get_or_insert_with(|| error.clone());
            }
            error
        })
    }
    async fn validate(&self) -> Result<(), ManualCheckFailure> {
        self.check()?;
        if let Err(error) = self.proxy.validate_ready_main(&self.ticket).await {
            let error = ManualCheckFailure::from(error);
            if let Ok(mut failure) = self.failure.lock() {
                failure.get_or_insert_with(|| error.clone());
            }
            return Err(error);
        }
        self.check()
    }
}

pub(super) struct GuardedHttp<'a, H, G> {
    pub(super) http: &'a H,
    pub(super) guard: &'a G,
}

impl<H: HttpClient, G: ManualBindingGuard> HttpClient for GuardedHttp<'_, H, G> {
    async fn fetch(&self, url: &str, init: &FetchInit) -> Result<MinimalResponse, String> {
        self.guard.validate().await.map_err(|error| error.code)?;
        let result = self.http.fetch(url, init).await;
        self.guard.validate().await.map_err(|error| error.code)?;
        result
    }

    async fn fetch_guarded(
        &self,
        url: &str,
        init: &FetchInit,
        target: &GuardedTarget,
    ) -> Result<MinimalResponse, String> {
        self.guard.validate().await.map_err(|error| error.code)?;
        let result = self.http.fetch_guarded(url, init, target).await;
        self.guard.validate().await.map_err(|error| error.code)?;
        result
    }
}

pub(super) struct GuardedUnlock<'a, H, G> {
    pub(super) http: &'a H,
    pub(super) guard: &'a G,
}

#[async_trait]
impl<H: UnlockHttp, G: ManualBindingGuard> UnlockHttp for GuardedUnlock<'_, H, G> {
    async fn request(&self, request: &UnlockRequest) -> UnlockResponse {
        if let Err(error) = self.guard.validate().await {
            return UnlockResponse::err(error.code);
        }
        let result = self.http.request(request).await;
        if let Err(error) = self.guard.validate().await {
            return UnlockResponse::err(error.code);
        }
        result
    }
}

#[async_trait]
trait ManualOperation: Sync {
    type Ready: Send + Sync;
    async fn prepare(&self) -> Result<Self::Ready, ManualCheckFailure>;
    fn context(&self, ready: &Self::Ready) -> ManualCheckContext;
    async fn ip_info(&self, ready: &Self::Ready) -> Result<Value, ManualCheckFailure>;
    async fn unlock(&self, ready: &Self::Ready) -> Result<UnlockSnapshot, ManualCheckFailure>;
}

async fn perform_manual_check<O: ManualOperation>(
    operation: &O,
    request: &ManualRequest,
) -> Result<ManualCheckResponse, ManualCheckFailure> {
    request.check()?;
    let ready = tokio::select! {
        biased;
        () = request.cancelled() => return Err(ManualCheckFailure::new("cancelled")),
        ready = operation.prepare() => ready?,
    };
    request.check()?;
    let context = operation.context(&ready);
    // Both detectors settle independently, including cancellation/deadline after one has completed.
    let (ip_info, unlock) = tokio::join!(
        settle_manual_leg(request, operation.ip_info(&ready)),
        settle_manual_leg(request, operation.unlock(&ready)),
    );
    Ok(ManualCheckResponse {
        context,
        ip_info: ip_info.into(),
        unlock: unlock.into(),
    })
}

async fn settle_manual_leg<T>(
    request: &ManualRequest,
    leg: impl std::future::Future<Output = Result<T, ManualCheckFailure>>,
) -> Result<T, ManualCheckFailure> {
    tokio::select! {
        biased;
        () = request.cancelled() => Err(ManualCheckFailure::new("cancelled")),
        result = tokio::time::timeout(Duration::from_secs(45), leg) =>
            result.unwrap_or_else(|_| Err(ManualCheckFailure::new("targetTimedOut"))),
    }
}

struct PreparedManual {
    main: BoundMain,
    local_proxy: LocalHttpProxy,
    exit_blocked: bool,
}

struct ProductionOperation {
    app: AppHandle,
    proxy: Arc<ProxyRuntime>,
    http: Arc<HttpRuntime>,
    unlock: Arc<UnlockRuntime>,
    binding: ActionBinding,
    request: Arc<ManualRequest>,
    force: bool,
}

#[async_trait]
impl ManualOperation for ProductionOperation {
    type Ready = PreparedManual;

    async fn prepare(&self) -> Result<PreparedManual, ManualCheckFailure> {
        let ticket = self.proxy.await_normal_main(self.binding.clone()).await?;
        let main = BoundMain {
            proxy: Arc::clone(&self.proxy),
            ticket,
            request: Arc::clone(&self.request),
            failure: Arc::new(Mutex::new(None)),
        };
        main.check()?;
        let local_proxy = main
            .ticket
            .local_http_proxy()
            .filter(|proxy| proxy.port != 0)
            .ok_or_else(|| ManualCheckFailure::new("readyUnknown"))?;
        let exit_blocked = self
            .app
            .try_state::<AppRuntime>()
            .map(|state| super::unlock::compute_selected_exit_blocked(&state, true))
            .ok_or_else(|| ManualCheckFailure::new("readyUnknown"))?;
        main.check()?;
        Ok(PreparedManual {
            main,
            local_proxy,
            exit_blocked,
        })
    }

    fn context(&self, ready: &PreparedManual) -> ManualCheckContext {
        ManualCheckContext {
            request_id: ready.main.ticket.request_id().into(),
            main_generation: ready.main.ticket.generation(),
            start_time: ready.main.ticket.start_time(),
        }
    }

    async fn ip_info(&self, ready: &PreparedManual) -> Result<Value, ManualCheckFailure> {
        ready.main.validate().await?;
        super::misc::run_bound_ipinfo(
            &self.app,
            self.http.as_ref(),
            &ready.local_proxy,
            &ready.main,
        )
        .await
    }

    async fn unlock(&self, ready: &PreparedManual) -> Result<UnlockSnapshot, ManualCheckFailure> {
        ready.main.validate().await?;
        super::unlock::run_bound_unlock_cycle(
            &self.app,
            &self.unlock,
            &ready.local_proxy,
            ready.exit_blocked,
            self.force,
            &ready.main,
        )
        .await
    }
}

fn selected_targets(saved: &Value) -> Result<Vec<String>, ManualCheckFailure> {
    let config: UserConfig = serde_json::from_value(saved.clone())
        .map_err(|_| ManualCheckFailure::new("invalidConfiguration"))?;
    let Some(selected) = config
        .selected_server_id
        .filter(|id| !is_sentinel_selection(Some(id.as_str())))
    else {
        return Ok(Vec::new());
    };
    if !config.servers.iter().any(|server| server.id == selected) {
        return Err(ManualCheckFailure::new("targetNotInMain"));
    }
    Ok(vec![selected])
}

#[tauri::command]
pub async fn manual_network_check(
    app: AppHandle,
    request_id: String,
    force: Option<bool>,
) -> ApiResponse<ManualCheckResponse> {
    let result = async {
        let (saved, proxy, http, unlock) = {
            let state = app
                .try_state::<AppRuntime>()
                .ok_or_else(|| ManualCheckFailure::new("readyUnknown"))?;
            let saved = state
                .config
                .current()
                .map_err(|_| ManualCheckFailure::new("invalidConfiguration"))?;
            (
                saved,
                Arc::clone(&state.proxy),
                Arc::clone(&state.http),
                Arc::clone(&state.unlock),
            )
        };
        let targets = selected_targets(&saved)?;
        let lease = manual_slot().admit(request_id)?;
        let binding = ActionBinding::new(
            NormalMainAction::ManualNetworkCheck,
            lease.request.request_id.clone(),
            &saved,
            targets,
            None,
        )?;
        let operation = ProductionOperation {
            app,
            proxy,
            http,
            unlock,
            binding,
            request: Arc::clone(&lease.request),
            force: force.unwrap_or(false),
        };
        perform_manual_check(&operation, &lease.request).await
    }
    .await;
    match result {
        Ok(response) => ApiResponse::ok(response),
        Err(error) => ApiResponse::err_with_code(error.code.clone(), error.code),
    }
}

#[tauri::command]
pub fn manual_network_check_cancel(request_id: String) -> ApiResponse<()> {
    manual_slot().cancel(&request_id);
    ApiResponse::ok(())
}

#[cfg(test)]
mod tests;
