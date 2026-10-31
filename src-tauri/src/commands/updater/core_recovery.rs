//! Current-app offline recovery commands. Neither accepts a source or payload.

use crate::runtime::helper::recovery_consumer::{user_recovery_requirement, CoreRecoveryState};
use crate::runtime::proxy::{ExpectedCoreStopRequest, ExpectedCoreStopResult, ProxyStatus};
use crate::{response::ApiResponse, runtime::AppRuntime};
use tauri::{AppHandle, Manager};

fn require_idle(status: &ProxyStatus) -> Result<(), CoreRecoveryState> {
    if status.running || status.starting || status.pid != 0 {
        Err(CoreRecoveryState::StopRequired)
    } else {
        Ok(())
    }
}

fn preflight(state: &AppRuntime) -> Result<(), CoreRecoveryState> {
    require_idle(&state.proxy().status())?;
    let config = state
        .config()
        .current()
        .map_err(|_| CoreRecoveryState::Unavailable)?;
    let config: polaris_config_engine::user_config::UserConfig =
        serde_json::from_value(config).map_err(|_| CoreRecoveryState::Unavailable)?;
    if config
        .proxy_mode_type
        .effective_on(state.helper.platform())
        .is_tun()
    {
        // A user-data slot cannot repair the privileged Helper's private store.
        return Err(CoreRecoveryState::HelperRequired);
    }
    Ok(())
}

/// This is an explicit cold operation, never part of launch or tray polling.
#[tauri::command]
pub async fn core_check_offline_recovery(
    app: AppHandle,
) -> Result<ApiResponse<CoreRecoveryState>, ()> {
    recovery_command(app, false).await
}

#[tauri::command]
pub async fn core_restore_bundled_data(
    app: AppHandle,
) -> Result<ApiResponse<CoreRecoveryState>, ()> {
    recovery_command(app, true).await
}

async fn recovery_command(
    app: AppHandle,
    apply: bool,
) -> Result<ApiResponse<CoreRecoveryState>, ()> {
    // Do not stop anything for unavailable/healthy data or a privileged-only mode.
    let probe_app = app.clone();
    let before = tokio::task::spawn_blocking(move || {
        let state = probe_app.state::<AppRuntime>();
        user_recovery_requirement().and_then(|_| preflight(&state))
    })
    .await
    .unwrap_or(Err(CoreRecoveryState::Unavailable));
    if let Err(state) = before {
        return Ok(ApiResponse::ok(state));
    }
    let mut stopped_generation = None;
    if apply {
        let runtime = app.state::<AppRuntime>();
        // The internal consumer compares even a cold generation atomically.
        // A successor Start is never stopped using an unconditional bootstrap.
        let status = match runtime.proxy.status_with_owned_stop_context() {
            Ok(status) => status,
            Err(_) => return Ok(ApiResponse::ok(CoreRecoveryState::Unavailable)),
        };
        if let Err(state) = require_idle(&status) {
            return Ok(ApiResponse::ok(state));
        }
        let Some(context) = status.owned_stop_context else {
            return Ok(ApiResponse::ok(CoreRecoveryState::Unavailable));
        };
        let result = runtime
            .proxy
            .stop_for_user_data_recovery(ExpectedCoreStopRequest {
                expected_main_generation: status.main_generation,
                expected_selected_server_id: context.selected_server_id,
                expected_selection_intent_generation: context.selection_intent_generation,
            })
            .await;
        match result {
            Ok(ExpectedCoreStopResult::Stopped {
                stop_main_generation,
                ..
            }) => {
                stopped_generation = Some(stop_main_generation);
            }
            _ => return Ok(ApiResponse::ok(CoreRecoveryState::StopRequired)),
        }
    }
    // spawn_blocking owns the complete transaction and its original fences:
    // cancellation of the renderer request does not drop its running work.
    let result = tokio::task::spawn_blocking(move || {
        let state = app.state::<AppRuntime>();
        state.helper.recover_current_user_core(apply, || {
            preflight(&state)?;
            if stopped_generation
                .is_some_and(|generation| state.proxy().status().main_generation != generation)
            {
                return Err(CoreRecoveryState::StopRequired);
            }
            Ok(())
        })
    })
    .await;
    Ok(ApiResponse::ok(
        result.unwrap_or(CoreRecoveryState::Unavailable),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recovery_refuses_running_starting_and_unconfirmed_pid() {
        assert_eq!(require_idle(&ProxyStatus::default()), Ok(()));
        for status in [
            ProxyStatus {
                running: true,
                ..Default::default()
            },
            ProxyStatus {
                starting: true,
                ..Default::default()
            },
            ProxyStatus {
                pid: 123,
                ..Default::default()
            },
        ] {
            assert_eq!(require_idle(&status), Err(CoreRecoveryState::StopRequired));
        }
    }
}
