#![cfg_attr(windows, windows_subsystem = "windows")]

fn main() {
    let Some(role) = polaris_windows_cleaner::role(std::env::args_os().skip(1)) else {
        std::process::exit(polaris_windows_cleaner::Outcome::Refused.code());
    };
    #[cfg(windows)]
    let outcome = match role {
        polaris_windows_cleaner::Role::Launcher => polaris_windows_cleaner::launch_current(),
        polaris_windows_cleaner::Role::Worker => polaris_windows_cleaner::run_worker(),
        polaris_windows_cleaner::Role::InstallWorker(path) => {
            // BIN ONLY: never read the process environment for installation
            // authority, and never compile this anchor into the Helper's LIB
            // dependency. Missing normal producer input fails closed (81).
            std::process::exit(polaris_windows_cleaner::run_install_worker(
                &path,
                option_env!("POLARIS_WINDOWS_INSTALL_ANCHOR"),
            ));
        }
        polaris_windows_cleaner::Role::SweepInstalledCores => {
            std::process::exit(polaris_windows_cleaner::sweep_installed_cores());
        }
    };
    #[cfg(not(windows))]
    let outcome = {
        let _ = role;
        polaris_windows_cleaner::Outcome::Refused
    };
    std::process::exit(outcome.code());
}
