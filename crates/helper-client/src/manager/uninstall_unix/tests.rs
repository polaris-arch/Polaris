use super::*;

fn utility() -> MaintenanceUtility {
    MaintenanceUtility {
        src_binary: "/private/staged/helper".into(),
        helper_sha256: "a".repeat(64),
    }
}

#[test]
fn maintenance_reuses_sealing_lease_and_existing_guard_binding() {
    for script in [
        build_mac_uninstall_script(&InstallPaths::mac(), &utility()),
        build_linux_uninstall_script(&InstallPaths::linux(), &utility()),
    ] {
        assert!(script.contains(LEASE_OPEN));
        assert!(script.contains("admin lease-acquire --lock \"$LOCK\" --fd 9"));
        assert!(script.contains("/dev/fd/8"));
        assert!(script.contains(&"a".repeat(64)));
        assert!(script.contains("polaris-helper-upgrade-v1"));
        assert!(script.contains("printf '%s' \"$DEST\" > \"$TXDIR/transaction-destination\""));
        assert!(!script.contains("rm -rf \"$SUPPORT\""));
        assert!(!script.contains("rm -f \"$LOCK\""));
        assert!(!script.contains("lease-release"));
        assert!(
            script.find("admin lease-acquire").unwrap() < script.find("TXDIR=$(mktemp").unwrap()
        );
        #[cfg(unix)]
        {
            use std::io::Write;
            use std::process::{Command, Stdio};
            let mut child = Command::new("/bin/sh")
                .arg("-n")
                .stdin(Stdio::piped())
                .spawn()
                .unwrap();
            child
                .stdin
                .take()
                .unwrap()
                .write_all(script.as_bytes())
                .unwrap();
            assert!(child.wait().unwrap().success());
        }
    }
    let mac = build_mac_uninstall_script(&InstallPaths::mac(), &utility());
    assert!(
        mac.find("admin migration-stop --journal").unwrap()
            < mac.find("admin migration-recheck --journal").unwrap()
    );
    assert!(
        mac.find("admin migration-recheck --journal").unwrap()
            < mac.find("retire_original \"$DEST\"").unwrap()
    );
    assert!(!mac.contains("launchctl"));
    assert!(!mac.contains("migration-rollback"));
    assert!(!mac.contains("core-install"));
    let linux = build_linux_uninstall_script(&InstallPaths::linux(), &utility());
    assert!(linux.contains(super::super::linux_service_stop::FUNCTIONS));
    assert!(
        linux.find("linux_service_closed ||").unwrap()
            < linux.find("rm -f \"$DESCRIPTOR\"").unwrap()
    );
}

#[cfg(unix)]
#[test]
fn mac_retirement_inert_shell_preserves_inodes_and_failure_evidence() {
    use std::os::unix::fs::MetadataExt;
    for failure in [
        "",
        "no-record",
        "lease-acquire",
        "migration-stop",
        "migration-recheck",
        "move-descriptor",
        "old-journal",
        "preflight",
        "preflight-unknown",
    ] {
        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path();
        let support = root.join("support");
        let boot = root.join("bootstrap");
        std::fs::create_dir(&support).unwrap();
        std::fs::create_dir(&boot).unwrap();
        std::fs::write(boot.join("helper"), "inert bundled utility").unwrap();
        let helper = root.join("installed-helper");
        let descriptor = root.join("installed.plist");
        let record = support.join("helper-installed.json");
        std::fs::write(&helper, "original helper").unwrap();
        std::fs::write(&descriptor, "original plist").unwrap();
        if failure != "no-record" {
            std::fs::write(&record, "original receipt").unwrap();
        }
        std::fs::write(support.join("user-data"), "preserved user state").unwrap();
        let helper_id = std::fs::metadata(&helper).unwrap().ino();
        let descriptor_id = std::fs::metadata(&descriptor).unwrap().ino();
        let lease = support.join("helper-install.lease");
        std::fs::write(&lease, "stable lease").unwrap();
        let lease_id = std::fs::metadata(&lease).unwrap().ino();
        if failure == "old-journal" {
            std::fs::create_dir(support.join("helper-upgrade.old")).unwrap();
            std::fs::write(
                support.join("helper-upgrade.old/recovery-required"),
                "evidence",
            )
            .unwrap();
        }
        // All paths are fixture-owned. Administrative/service verbs are inert
        // shell functions, and no installed or bundled helper is executed.
        let script = format!(
            r#"set -eu
umask 077
MAINTENANCE_MAC=1
SUPPORT={support}
BOOT={boot}
DEST={helper}
DESCRIPTOR={descriptor}
RECORD="$SUPPORT/helper-installed.json"
FAILURE={failure}
TRACE={trace}
close_bootstrap() {{ :; }}
admin() {{
  printf '%s\n' "$1" >> "$TRACE"
  if [ "$1" = migration-uninstall-preflight-unchanged ]; then
    [ "$FAILURE" = preflight ]; return
  fi
  if [ "$1" = migration-stop ]; then
    case "$FAILURE" in preflight|preflight-unknown) return 1;; esac
  fi
  [ "$1" != "$FAILURE" ] || return 1
  case "$1" in
    lease-acquire|sync-install-path|migration-stop|migration-recheck) ;;
    *) return 90;;
  esac
}}
file_identity() {{ /bin/ls -di "$1" | /usr/bin/awk '{{print $1}}'; }}
mv() {{
  if [ "$FAILURE" = move-descriptor ]; then
    case "$2" in */descriptor.original) return 1;; esac
  fi
  /bin/mv "$@"
}}
{lease_open}{set_phase}{common}{retirement}
"#,
            support = shell_quote(support.to_str().unwrap()),
            boot = shell_quote(boot.to_str().unwrap()),
            helper = shell_quote(helper.to_str().unwrap()),
            descriptor = shell_quote(descriptor.to_str().unwrap()),
            failure = shell_quote(failure),
            trace = shell_quote(root.join("trace").to_str().unwrap()),
            lease_open = LEASE_OPEN,
            set_phase = SET_PHASE,
            common = COMMON_TRANSACTION,
            retirement = MAC_RETIREMENT,
        );
        let output = std::process::Command::new("/bin/sh")
            .args(["-c", &script])
            .output()
            .unwrap();
        let succeeds = matches!(failure, "" | "no-record");
        assert_eq!(
            output.status.success(),
            succeeds,
            "{failure}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&output.stdout).contains("polaris-helper-uninstall-ok"),
            succeeds
        );
        assert_eq!(std::fs::metadata(&lease).unwrap().ino(), lease_id);
        assert_eq!(
            std::fs::read_to_string(support.join("user-data")).unwrap(),
            "preserved user state"
        );
        let journals: Vec<_> = std::fs::read_dir(&support)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.is_dir())
            .collect();
        if matches!(failure, "lease-acquire" | "preflight") {
            assert!(journals.is_empty());
            assert_eq!(std::fs::metadata(&helper).unwrap().ino(), helper_id);
            assert_eq!(std::fs::metadata(&descriptor).unwrap().ino(), descriptor_id);
        } else {
            assert_eq!(journals.len(), 1, "{failure}");
            let journal = &journals[0];
            if succeeds {
                assert_eq!(
                    std::fs::metadata(journal.join("helper.original"))
                        .unwrap()
                        .ino(),
                    helper_id
                );
                assert_eq!(
                    std::fs::metadata(journal.join("descriptor.original"))
                        .unwrap()
                        .ino(),
                    descriptor_id
                );
                assert_eq!(
                    journal.join("record.original").exists(),
                    failure != "no-record"
                );
                assert!(!helper.exists());
                assert!(!descriptor.exists());
            } else {
                assert!(journal.join("recovery-required").exists());
                assert_eq!(std::fs::metadata(&descriptor).unwrap().ino(), descriptor_id);
                let helper_path = if failure == "move-descriptor" {
                    journal.join("helper.original")
                } else {
                    helper.clone()
                };
                assert_eq!(std::fs::metadata(helper_path).unwrap().ino(), helper_id);
            }
        }
        if matches!(failure, "lease-acquire" | "old-journal") {
            assert!(!std::fs::read_to_string(root.join("trace"))
                .unwrap()
                .contains("migration-stop"));
        }
    }
}

#[cfg(unix)]
#[test]
fn linux_cleanup_keeps_lease_parent_and_unfinished_admission() {
    use std::os::unix::fs::MetadataExt;
    for failure in ["", "validate", "stop", "closure", "remove", "reload"] {
        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path();
        for name in ["support", "bootstrap", "install", "runtime"] {
            std::fs::create_dir(root.join(name)).unwrap();
        }
        let support = root.join("support");
        let lease = support.join("helper-install.lease");
        std::fs::write(&lease, "stable lease").unwrap();
        let lease_id = std::fs::metadata(&lease).unwrap().ino();
        std::fs::write(root.join("bootstrap/helper"), "inert utility").unwrap();
        std::fs::write(root.join("install/helper"), "installed helper").unwrap();
        std::fs::write(root.join("unit"), "unit").unwrap();
        std::fs::write(support.join("state"), "state").unwrap();
        std::fs::create_dir(support.join("helper-admission.unfinished")).unwrap();
        std::fs::write(
            support.join("helper-admission.unfinished/evidence"),
            "retain",
        )
        .unwrap();
        let script = format!(
            r#"set -eu
umask 077
MAINTENANCE_MAC=0
ROOT={root}
SUPPORT="$ROOT/support"
BOOT="$ROOT/bootstrap"
DEST="$ROOT/install/helper"
DESCRIPTOR="$ROOT/unit"
INSTALLDIR="$ROOT/install"
RUNTIMEDIR="$ROOT/runtime"
SERVICE=fixture
FAILURE={failure}
close_bootstrap() {{ :; }}
admin() {{ case "$1" in lease-acquire|sync-install-path) ;; *) return 90;; esac; }}
linux_validate_service() {{ LOAD=loaded; [ "$FAILURE" != validate ]; }}
linux_service_closed() {{ [ "$FAILURE" != closure ]; }}
systemctl() {{
  case "$*" in
    '--system disable --now fixture') [ "$FAILURE" != stop ];;
    daemon-reload) [ "$FAILURE" != reload ];;
    *) return 90;;
  esac
}}
rm() {{
  printf '%s\n' remove >> "$ROOT/trace"
  [ "$FAILURE" != remove ] || return 1
  /bin/rm "$@"
}}
{lease_open}{set_phase}{common}{removal}
"#,
            root = shell_quote(root.to_str().unwrap()),
            failure = shell_quote(failure),
            lease_open = LEASE_OPEN,
            set_phase = SET_PHASE,
            common = COMMON_TRANSACTION,
            removal = LINUX_REMOVAL,
        );
        let output = std::process::Command::new("/bin/sh")
            .args(["-c", &script])
            .output()
            .unwrap();
        assert_eq!(
            output.status.success(),
            failure.is_empty(),
            "{failure}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(support.is_dir());
        assert_eq!(std::fs::metadata(&lease).unwrap().ino(), lease_id);
        assert_eq!(
            std::fs::read_to_string(support.join("helper-admission.unfinished/evidence")).unwrap(),
            "retain"
        );
        if matches!(failure, "validate" | "stop" | "closure") {
            assert!(!root.join("trace").exists());
            assert!(root.join("install/helper").is_file());
            assert!(root.join("unit").is_file());
        }
        let journals: Vec<_> = std::fs::read_dir(&support)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| {
                path.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("helper-upgrade.")
            })
            .collect();
        if failure.is_empty() {
            assert!(journals.is_empty());
            assert!(!root.join("install").exists());
            assert!(!root.join("runtime").exists());
            assert!(!support.join("state").exists());
        } else {
            assert_eq!(journals.len(), 1);
            assert!(journals[0].join("recovery-required").is_file());
        }
    }
}
