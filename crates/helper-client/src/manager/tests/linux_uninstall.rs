#[cfg(target_os = "linux")]
use super::*;

#[test]
#[cfg(target_os = "linux")]
pub(super) fn linux_uninstall_requires_effective_unit_and_hierarchical_closure() {
    // Exercise the actual shared proof with inert stop/removal commands. Sealed
    // utility bootstrap and the maintenance lease are covered by uninstall_unix.
    let script = format!(
        r#"set -eu
SERVICE=polaris-helper.service
DESCRIPTOR=/etc/systemd/system/polaris-helper.service
{}
linux_validate_service || exit 1
if [ "$LOAD" = loaded ]; then systemctl --system disable --now "$SERVICE"; fi
linux_service_closed || exit 1
rm -f "$DESCRIPTOR"
systemctl --system daemon-reload
echo polaris-helper-uninstall-ok
"#,
        linux_service_stop::FUNCTIONS
    );
    assert!(script.contains("set -eu"));
    assert!(!script.contains("|| true"));
    assert_shell_syntax("/bin/sh", &script);
    // All external commands are inert fixtures. No real systemctl, stat or rm.
    for (key, value, succeeds) in [
        ("", "", true),
        ("LOAD", "not-found", true),
        ("STATE", "failed", true),
        ("STATE", "active", false),
        ("PID", "12", false),
        ("STOP_RC", "1", false),
        ("FAIL_PROPERTY", "LoadState", false),
        ("FAIL_PROPERTY", "ActiveState", false),
        ("FAIL_PROPERTY", "MainPID", false),
        ("FAIL_PROPERTY", "ControlGroup", false),
        ("LOAD", "", false),
        ("LOAD", "error", false),
        ("STATE", "deactivating", false),
        ("KILL_MODE", "process", false),
        ("SEND_KILL", "no", false),
        ("FINAL_KILL", "15", false),
        ("DELEGATE", "yes", false),
        (
            "DROPINS",
            "/etc/systemd/system/polaris-helper.service.d/override.conf",
            false,
        ),
        ("UNIT_USER", "1000", false),
        ("FRAGMENT", "/inert/foreign.service", false),
        ("CGROUP", "/system.slice/foreign.service", false),
        ("CGTYPE", "tmpfs", false),
        ("EVENTS", "populated 1\nfrozen 0\n", false),
        ("EVENTS", "frozen 0\n", false),
        ("EVENTS", "populated 0\npopulated 0\n", false),
        ("RM_RC", "1", false),
        ("RELOAD_RC", "1", false),
        ("MISSING_SYSTEMCTL", "1", false),
        ("EVENT_READ_RC", "1", false),
        ("OWNER", "1000", false),
        ("MODE", "666", false),
        ("ABSENT_CGROUP", "1", true),
        ("MANAGER", "other-init", false),
        ("NAMESPACE", "cgroup", false),
        ("NAMESPACE", "mnt", false),
        ("NAMESPACE", "pid", false),
        ("NAMESPACE_READ_RC", "1", false),
        ("MANAGER_GROUP", "", true),
        ("MANAGER_FORM", "missing", false),
        ("MANAGER_FORM", "duplicate", false),
        ("MANAGER_FORM", "foreign", false),
        ("INIT_QUERY_RC", "1", false),
        ("PID1_READ_RC", "1", false),
        ("PID1_GROUP", "0::/", true),
        ("PID1_GROUP", "0::/subtree/init.scope", false),
        ("PID1_GROUP", "0::/init.scope\n0::/init.scope", false),
        ("PID1_GROUP", "", false),
        ("INIT_GROUP", "/foreign", false),
        ("INIT_GROUP", "", true),
        ("INIT_GROUP", "/", false),
        ("INIT_LOAD", "not-found", false),
        ("INIT_STATE", "inactive", false),
        ("INIT_FORM", "missing", false),
        ("INIT_FORM", "duplicate", false),
        ("PID1_MEMBERS", "2", false),
        ("PID1_MEMBERS", "PRIVATE", false),
        ("PID1_MEMBERS_RC", "1", false),
        ("PID1_SECOND_MISSING", "1", false),
        ("PID1_CHANGED", "1", false),
        ("MANAGER_GROUP", "/subtree", false),
        ("MANAGER_QUERY_RC", "1", false),
        ("MOUNT_ROOT", "/subtree", false),
        ("MOUNT_MISSING", "1", false),
        ("MOUNT_DUPLICATE", "1", false),
        ("MOUNT_CHILD", "1", false),
        ("MOUNT_READ_RC", "1", false),
        ("FIND_RC", "1", false),
        ("ABSENT_FIND_FAIL", "1", false),
        ("POST_DROPINS", "/inert/changed.conf", false),
        ("POST_KILL_MODE", "process", false),
        ("POST_QUERY_FAIL", "1", false),
        ("MISSING_PROPERTY", "DropInPaths", false),
        ("MISSING_PROPERTY", "User", false),
        ("DUPLICATE_PROPERTY", "User", false),
        ("FOREIGN_PROPERTY", "User", false),
    ] {
        run_linux_proof_fixture(&script, key, value, succeeds, &[]);
    }
    // Reproduce the N16 observed empty-manager/empty-unit pair. Namespace and
    // mount evidence here is an additional strong fixture, not ordinary authority.
    run_linux_proof_fixture(&script, "INIT_GROUP", "", true, &[("MANAGER_GROUP", "")]);
}

#[cfg(target_os = "linux")]
fn run_linux_proof_fixture(
    script: &str,
    key: &str,
    value: &str,
    succeeds: bool,
    overrides: &[(&str, &str)],
) {
    let fixture = tempfile::tempdir().unwrap();
    let descriptor = fixture.path().join("helper.service");
    std::fs::write(&descriptor, "fixture").unwrap();
    let cgroup = fixture
        .path()
        .join("cgroup/system.slice/polaris-helper.service");
    std::fs::create_dir_all(&cgroup).unwrap();
    std::fs::write(
        cgroup.join("cgroup.events"),
        if key == "EVENTS" {
            value
        } else {
            "populated 0\nfrozen 0\n"
        },
    )
    .unwrap();
    if matches!(key, "ABSENT_CGROUP" | "ABSENT_FIND_FAIL") {
        std::fs::remove_dir_all(&cgroup).unwrap();
    }
    std::fs::create_dir_all(fixture.path().join("cgroup/init.scope")).unwrap();
    let commands = [
        ("systemctl", r#"[ "$1" != --system ] || shift
case "$1" in
  show)
PROPERTY=${3#--property=}
if [ "$#" -eq 3 ]; then
  case "$MOCK_MANAGER_FORM" in missing) exit 0;; duplicate) echo "$PROPERTY=/";; foreign) echo Other=; exit 0;; esac
  echo "$PROPERTY=$MOCK_MANAGER_GROUP"; exit "$MOCK_MANAGER_QUERY_RC"
fi
if [ "$4" = init.scope ]; then
  case "$MOCK_INIT_FORM" in missing) exit 0;; duplicate) echo "$PROPERTY=";; esac
  case "$PROPERTY" in ControlGroup) VALUE=$MOCK_INIT_GROUP;; LoadState) VALUE=$MOCK_INIT_LOAD;; ActiveState) VALUE=$MOCK_INIT_STATE;; *) exit 90;; esac
  echo "$PROPERTY=$VALUE"; exit "$MOCK_INIT_QUERY_RC"
fi
[ "$PROPERTY" != "$MOCK_MISSING_PROPERTY" ] || exit 0
if [ "$PROPERTY" = "$MOCK_DUPLICATE_PROPERTY" ]; then echo "$PROPERTY=one"; echo "$PROPERTY=two"; exit 0; fi
if [ "$PROPERTY" = "$MOCK_FOREIGN_PROPERTY" ]; then echo Foreign=value; exit 0; fi
if [ -f "$MOCK_STOPPED" ]; then
  [ "$MOCK_POST_QUERY_FAIL" = 0 ] || exit 1
  if [ "$PROPERTY" = DropInPaths ] && [ -n "$MOCK_POST_DROPINS" ]; then echo "$PROPERTY=$MOCK_POST_DROPINS"; exit 0; fi
  if [ "$PROPERTY" = KillMode ] && [ -n "$MOCK_POST_KILL_MODE" ]; then echo "$PROPERTY=$MOCK_POST_KILL_MODE"; exit 0; fi
fi
[ "$PROPERTY" != "$MOCK_FAIL_PROPERTY" ] || exit 1
printf '%s=' "$PROPERTY"
case "$PROPERTY" in
  LoadState) echo "$MOCK_LOAD";;
  FragmentPath) echo "$MOCK_FRAGMENT";;
  DropInPaths) echo "$MOCK_DROPINS";;
  User) echo "$MOCK_UNIT_USER";;
  KillMode) echo "$MOCK_KILL_MODE";;
  SendSIGKILL) echo "$MOCK_SEND_KILL";;
  FinalKillSignal) echo "$MOCK_FINAL_KILL";;
  Delegate) echo "$MOCK_DELEGATE";;
  Slice) echo system.slice;;
  ControlGroup) echo "$MOCK_CGROUP";;
  ActiveState) echo "$MOCK_STATE";;
  MainPID) echo "$MOCK_PID";;
  *) exit 90;;
esac;;
  disable|stop) echo stop >> "$MOCK_TRACE"; : > "$MOCK_STOPPED"; exit "$MOCK_STOP_RC";;
  daemon-reload) echo reload >> "$MOCK_TRACE"; exit "$MOCK_RELOAD_RC";;
  *) exit 90;;
esac
"#),
        ("stat", "case \"$1:$2\" in -c:%u) echo \"$MOCK_OWNER\";; -c:%a) echo \"$MOCK_MODE\";; -f:-c) echo \"$MOCK_CGTYPE\";; *) exit 90;; esac\n"),
        ("cat", r#"case "$1" in
  /proc/1/comm) echo "$MOCK_MANAGER"; exit 0;;
  /proc/1/cgroup) [ "$MOCK_PID1_READ_RC" = 0 ] || exit 1
    if [ "$MOCK_PID1_CHANGED" = 1 ] && [ -f "$MOCK_MEMBERS_SEEN" ]; then echo '0::/'; exit 0; fi
    printf '%s\n' "$MOCK_PID1_GROUP"; exit 0;;
  "$MOCK_CGROOT/cgroup.procs"|"$MOCK_CGROOT/init.scope/cgroup.procs")
    [ "$MOCK_PID1_MEMBERS_RC" = 0 ] || exit 1
    if [ "$MOCK_PID1_SECOND_MISSING" = 1 ] && [ -f "$MOCK_MEMBERS_SEEN" ]; then echo 2; exit 0; fi
    : > "$MOCK_MEMBERS_SEEN"
    printf '%s\n' "$MOCK_PID1_MEMBERS"; exit 0;;
  /proc/self/mountinfo)
[ "$MOCK_MOUNT_READ_RC" = 0 ] || exit 1
[ "$MOCK_MOUNT_MISSING" = 0 ] || exit 0
echo "1 0 0:1 $MOCK_MOUNT_ROOT $MOCK_CGROOT rw - cgroup2 cgroup rw"
if [ "$MOCK_MOUNT_DUPLICATE" = 1 ]; then echo "2 0 0:1 / $MOCK_CGROOT rw - cgroup2 cgroup rw"; fi
if [ "$MOCK_MOUNT_CHILD" = 1 ]; then echo "2 1 0:1 /foreign $MOCK_CGROOT/system.slice rw - cgroup2 cgroup rw"; fi
exit 0;;
esac
[ "$MOCK_EVENT_READ_RC" = 0 ] || exit 1
exec /bin/cat "$@"
"#),
        ("readlink", r#"[ "$MOCK_NAMESPACE_READ_RC" = 0 ] || exit 1
case "$1" in
  /proc/self/ns/*) echo "namespace-${1##*/}";;
  /proc/1/ns/*) if [ "${1##*/}" = "$MOCK_NAMESPACE" ]; then echo different; else echo "namespace-${1##*/}"; fi;;
  *) exit 1;;
esac
"#),
        ("find", r#"[ "$MOCK_FIND_RC" = 0 ] && [ "$MOCK_ABSENT_FIND_FAIL" = 0 ] || exit 1
exec /usr/bin/find "$@"
"#),
        ("rm", "echo remove >> \"$MOCK_TRACE\"\nexit \"$MOCK_RM_RC\"\n"),
    ];
    for (name, content) in commands {
        if name == "systemctl" && key == "MISSING_SYSTEMCTL" {
            continue;
        }
        let command = fixture.path().join(name);
        write_executable_stand_in(&command, format!("#!/bin/sh\n{content}"));
    }
    let script = script
        .replace(
            "/etc/systemd/system/polaris-helper.service",
            descriptor.to_str().unwrap(),
        )
        .replace(
            "CGROOT=/sys/fs/cgroup",
            &format!(
                "CGROOT={}",
                shell_quote(fixture.path().join("cgroup").to_str().unwrap())
            ),
        );
    let mut command = std::process::Command::new("/bin/sh");
    command
        .args(["-c", &script])
        .env("PATH", fixture.path())
        .env("MOCK_MEMBERS_SEEN", fixture.path().join("members-seen"));
    for (name, value) in [
        ("LOAD", "loaded"),
        ("STATE", "inactive"),
        ("PID", "0"),
        ("EVENT_READ_RC", "0"),
        ("OWNER", "0"),
        ("MODE", "644"),
        ("MANAGER", "systemd"),
        ("NAMESPACE", ""),
        ("NAMESPACE_READ_RC", "0"),
        ("MANAGER_GROUP", "/"),
        ("PID1_GROUP", "0::/init.scope"),
        ("INIT_GROUP", "/init.scope"),
        ("INIT_LOAD", "loaded"),
        ("INIT_STATE", "active"),
        ("INIT_FORM", "normal"),
        ("PID1_MEMBERS", "1\n2"),
        ("PID1_MEMBERS_RC", "0"),
        ("PID1_SECOND_MISSING", "0"),
        ("PID1_CHANGED", "0"),
        ("MANAGER_QUERY_RC", "0"),
        ("MANAGER_FORM", "normal"),
        ("INIT_QUERY_RC", "0"),
        ("PID1_READ_RC", "0"),
        ("MOUNT_ROOT", "/"),
        ("MOUNT_READ_RC", "0"),
        ("MOUNT_MISSING", "0"),
        ("MOUNT_DUPLICATE", "0"),
        ("MOUNT_CHILD", "0"),
        ("FIND_RC", "0"),
        ("ABSENT_FIND_FAIL", "0"),
        ("POST_DROPINS", ""),
        ("POST_KILL_MODE", ""),
        ("POST_QUERY_FAIL", "0"),
        ("MISSING_PROPERTY", ""),
        ("DUPLICATE_PROPERTY", ""),
        ("FOREIGN_PROPERTY", ""),
        ("STOP_RC", "0"),
        ("RM_RC", "0"),
        ("RELOAD_RC", "0"),
        ("FAIL_PROPERTY", ""),
        ("DROPINS", ""),
        ("UNIT_USER", ""),
        ("KILL_MODE", "control-group"),
        ("SEND_KILL", "yes"),
        ("FINAL_KILL", "9"),
        ("DELEGATE", "no"),
        ("CGROUP", "/system.slice/polaris-helper.service"),
        ("CGTYPE", "cgroup2fs"),
    ] {
        command.env(format!("MOCK_{name}"), value);
    }
    command
        .env("MOCK_CGROOT", fixture.path().join("cgroup"))
        .env("MOCK_STOPPED", fixture.path().join("stopped"));
    command
        .env("MOCK_FRAGMENT", &descriptor)
        .env("MOCK_TRACE", fixture.path().join("trace"));
    if !key.is_empty() {
        command.env(format!("MOCK_{key}"), value);
    }
    for (name, value) in overrides {
        command.env(format!("MOCK_{name}"), value);
    }
    let output = command.output().unwrap();
    assert_eq!(
        output.status.success(),
        succeeds,
        "{key}={value}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).contains("polaris-helper-uninstall-ok"),
        succeeds
    );
    if !succeeds && !matches!(key, "RM_RC" | "RELOAD_RC") {
        let trace = std::fs::read_to_string(fixture.path().join("trace")).unwrap_or_default();
        assert!(!trace.contains("remove"), "{key} must preserve files");
        assert!(
            !trace.contains("register"),
            "{key} must not mutate registration"
        );
        if script.contains("INITIAL_STATE=")
            && ((key == "FAIL_PROPERTY" && value == "ActiveState")
                || (key == "STATE" && matches!(value, "unknown" | "deactivating")))
        {
            assert!(
                !trace.contains("stop"),
                "invalid initial state must not stop the service"
            );
        }
    }
}

#[test]
#[cfg(target_os = "linux")]
pub(super) fn linux_install_inactive_snapshot_still_requires_production_closure() {
    let params = install_params(PathBuf::from("/inert"), PathBuf::from("/inert/helper"));
    let generated = build_linux_install_script(&InstallPaths::linux(), &params, &"a".repeat(64));
    let stop_body = generated
        .split_once("stop_service() {\n")
        .unwrap()
        .1
        .split_once("\n}\nrestore_service()")
        .unwrap()
        .0;
    let admission = generated
        .rsplit_once("trap rollback EXIT\n")
        .unwrap()
        .1
        .split_once("\nregister_user\n")
        .unwrap()
        .0;
    let required = generated
        .lines()
        .find(|line| line.starts_with("REQUIRE_CLOSURE="))
        .unwrap();
    let snapshot = format!(
        "INITIAL_STATE={}",
        generated
            .split_once("INITIAL_STATE=")
            .unwrap()
            .1
            .split_once("\nif systemctl is-enabled --quiet")
            .unwrap()
            .0
    );
    // All lifecycle scaffolding is inert. The stop function, admission condition,
    // requirement value and service snapshot come from the production generator.
    let script = format!(
        r#"set -eu
SERVICE=polaris-helper.service
DESCRIPTOR=/etc/systemd/system/polaris-helper.service
WAS_ACTIVE=0
{required}
{proof}
stop_service() {{
{stop_body}
}}
rollback() {{ rc=$?; trap - EXIT; exit "$rc"; }}
set_phase() {{ :; }}
admin() {{ return 91; }}
register_user() {{ echo register >> "$MOCK_TRACE"; }}
{snapshot}
trap rollback EXIT
{admission}
register_user
echo polaris-helper-uninstall-ok
"#,
        proof = linux_service_stop::FUNCTIONS
    );
    assert_shell_syntax("/bin/sh", &script);
    for (key, value, succeeds, overrides) in [
        (
            "EVENTS",
            "populated 1\nfrozen 0\n",
            false,
            vec![("STATE", "inactive")],
        ),
        (
            "EVENTS",
            "populated 1\nfrozen 0\n",
            false,
            vec![("STATE", "failed")],
        ),
        ("LOAD", "unknown", false, vec![]),
        ("FAIL_PROPERTY", "LoadState", false, vec![]),
        ("FAIL_PROPERTY", "ActiveState", false, vec![]),
        ("FAIL_PROPERTY", "MainPID", false, vec![]),
        ("ABSENT_CGROUP", "1", true, vec![("LOAD", "not-found")]),
        ("STATE", "inactive", true, vec![]),
        ("STATE", "failed", true, vec![]),
        ("STATE", "unknown", false, vec![]),
        ("STATE", "deactivating", false, vec![]),
    ] {
        run_linux_proof_fixture(&script, key, value, succeeds, &overrides);
    }
}

#[test]
#[cfg(target_os = "linux")]
pub(super) fn linux_cleanup_failure_markers_do_not_change_success_or_recovery() {
    let params = install_params(PathBuf::from("/inert"), PathBuf::from("/inert/helper"));
    let generated = build_linux_install_script(&InstallPaths::linux(), &params, &"a".repeat(64));
    let functions = generated
        .split_once("\ncleanup_failure() {\n")
        .unwrap()
        .1
        .split_once("\ntrap cleanup EXIT")
        .unwrap()
        .0;
    assert!(!functions.contains("\ntrap "));
    assert!(!functions.contains("/bin/"));
    for (original, removal, expected) in [(0, 0, 0), (7, 0, 7), (0, 2, 2), (7, 2, 2)] {
        let script = format!(
            r#"set -eu
{diagnostics}
cleanup_failure() {{
{functions}
rm() {{ return {removal}; }}
touch() {{ :; }}
admin() {{ :; }}
close_bootstrap() {{ :; }}
ADMITTED=0
TXDIR=/inert-polaris-diagnostic-only
BOOT=''
INSTALL_STAGE=manager-property
trap cleanup EXIT
exit {original}
"#,
            diagnostics = linux_service_stop::DIAGNOSTICS
        );
        let output = std::process::Command::new("/bin/sh")
            .env_clear()
            .env("PATH", "/nonexistent-polaris-inert-test")
            .args(["-c", &script])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(expected));
        let stderr = String::from_utf8(output.stderr).unwrap();
        if expected == 0 {
            assert!(!stderr.contains("POLARIS_HELPER_FAILURE_V1:"));
        } else {
            let stage = if original == 0 {
                "cleanup"
            } else {
                "manager-property"
            };
            assert!(stderr.contains(&format!("POLARIS_HELPER_FAILURE_V1:{stage}:")));
            if removal != 0 {
                assert!(stderr.contains("recovery-unconfirmed"));
            }
        }
    }
}

#[test]
#[cfg(target_os = "linux")]
pub(super) fn linux_maintenance_traps_keep_success_silent_and_cleanup_unknown() {
    let utility = MaintenanceUtility {
        src_binary: PathBuf::from("/inert/helper"),
        helper_sha256: "a".repeat(64),
    };
    let generated = uninstall_unix::build_linux_uninstall_script(&InstallPaths::linux(), &utility);
    for name in ["bootstrap_cleanup", "cleanup"] {
        let definition = generated
            .split_once(&format!("\n{name}() {{\n"))
            .unwrap()
            .1
            .split_once(&format!("\ntrap {name} EXIT"))
            .unwrap()
            .0;
        assert!(!definition.contains("\ntrap "));
        assert!(!definition.contains("/bin/chmod"));
        assert!(!definition.contains("/usr/bin/mktemp"));
        assert!(!definition.contains("/bin/"));
        for (original, close_failed, expected) in [(0, 0, 0), (7, 0, 7), (0, 1, 1), (7, 1, 1)] {
            let expected = if name == "bootstrap_cleanup" && original == 7 {
                7
            } else {
                expected
            };
            let script = format!(
                r#"set -eu
{diagnostics}
{name}() {{
{definition}
close_bootstrap() {{ return {close_failed}; }}
rm() {{ return 91; }}
touch() {{ return 91; }}
admin() {{ return 91; }}
FINISHED=1
TXDIR=''
FINAL_STATE=stopped-removed-lease-preserved
INSTALL_STAGE=manager-property
trap {name} EXIT
exit {original}
"#,
                diagnostics = linux_service_stop::DIAGNOSTICS
            );
            let output = std::process::Command::new("/bin/sh")
                .env_clear()
                .env("PATH", "/nonexistent-polaris-inert-test")
                .args(["-c", &script])
                .output()
                .unwrap();
            assert_eq!(output.status.code(), Some(expected));
            let stderr = String::from_utf8(output.stderr).unwrap();
            if expected == 0 {
                assert!(!stderr.contains("POLARIS_HELPER_FAILURE_V1:"));
            } else {
                assert!(stderr.contains("POLARIS_HELPER_FAILURE_V1:"));
            }
            if close_failed != 0 {
                assert!(stderr.contains("recovery-unconfirmed"));
            }
        }
    }
}

#[test]
fn legacy_and_ordinary_producers_share_the_complete_cohort_body_without_relaxing_regular_units() {
    let proof = crate::manager::LINUX_SERVICE_CLOSURE_FUNCTIONS;
    assert!(proof.contains("linux_service_closed() {"));
    assert!(proof.contains("linux_validate_service || return 1\n  linux_cohort_closed"));
    let common = proof.split("linux_cohort_closed() {").nth(1).unwrap();
    for required in [
        "linux_cgroup_view || return 1",
        "ActiveState",
        "MainPID",
        "ControlGroup",
        "find \"$CGPARENT\" -mindepth 1 -maxdepth 1",
        "cgroup.events",
        "populated",
        "POPULATED=unknown",
    ] {
        assert!(common.contains(required), "{required}");
    }
    for strict in [
        "loaded)",
        "[ -f \"$DESCRIPTOR\" ] && [ ! -L \"$DESCRIPTOR\" ]",
        "DropInPaths",
        "KillMode",
        "Delegate",
    ] {
        assert!(
            proof.contains(strict),
            "ordinary regular-unit validator lost {strict}"
        );
    }
    let removal = polaris_source_probe::crate_source!("manager/uninstall_unix.rs");
    assert!(removal.contains("helper-retired.*"));
    assert!(removal.contains("helper-legacy-tombstone"));
    assert!(removal.contains("linux_legacy_wrapper("));
}
