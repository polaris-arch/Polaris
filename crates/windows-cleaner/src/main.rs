#![cfg_attr(windows, windows_subsystem = "windows")]

fn main() {
    let Some(role) = polaris_windows_cleaner::role(std::env::args_os().skip(1)) else {
        std::process::exit(polaris_windows_cleaner::Outcome::Refused.code());
    };
    #[cfg(windows)]
    let outcome = match role {
        polaris_windows_cleaner::Role::Launcher => polaris_windows_cleaner::launch_current(),
        polaris_windows_cleaner::Role::Worker => polaris_windows_cleaner::run_worker(),
    };
    #[cfg(not(windows))]
    let outcome = {
        let _ = role;
        polaris_windows_cleaner::Outcome::Refused
    };
    std::process::exit(outcome.code());
}
