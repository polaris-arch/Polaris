//! No installer, daemon, device, or elevation: only the production admission
//! fragment, synthetic files, and an inherited-fd flock fixture.
use super::*;
use std::fs;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let directory =
            std::env::temp_dir().join(format!("polaris-update-recovery-{}", uuid::Uuid::new_v4()));
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&directory)
            .unwrap();
        Self(directory)
    }
    fn destination(&self) -> PathBuf {
        self.0.join("Polaris.AppImage")
    }
    fn write_artifact(&self, path: &Path, bytes: &[u8]) {
        if cfg!(target_os = "macos") {
            fs::create_dir_all(path).unwrap();
            fs::write(path.join("content"), bytes).unwrap();
        } else {
            fs::write(path, bytes).unwrap();
        }
    }
    fn read_artifact(&self, path: &Path) -> Vec<u8> {
        fs::read(if cfg!(target_os = "macos") {
            path.join("content")
        } else {
            path.to_path_buf()
        })
        .unwrap()
    }
    fn journal(&self, phase: &str) -> PathBuf {
        let path = self
            .0
            .join(format!(".polaris-update.{}", uuid::Uuid::new_v4()));
        fs::DirBuilder::new().mode(0o700).create(&path).unwrap();
        fs::write(path.join("transaction-kind"), "polaris-update-v1\n").unwrap();
        fs::write(
            path.join("transaction-destination"),
            self.destination().to_str().unwrap(),
        )
        .unwrap();
        fs::write(path.join("transaction-phase"), format!("{phase}\n")).unwrap();
        path
    }
    fn command(&self, tail: &str) -> Command {
        let source = self.0.join(format!("source-{}", uuid::Uuid::new_v4()));
        // flock on the inherited open description persists in Bash after this
        // short-lived fixture exits, as in the actual process_lease utility.
        let helper = b"#!/usr/bin/env python3\nimport sys,fcntl\nfd=int(sys.argv[-1])\nfcntl.flock(fd,fcntl.LOCK_EX|fcntl.LOCK_NB if sys.argv[1]=='lease-acquire' else fcntl.LOCK_UN)\n";
        fs::write(&source, helper).unwrap();
        let frozen =
            freeze_lease_helper_in(&self.0, &source, &polaris_updater::sha256_hex(helper)).unwrap();
        let object = if cfg!(target_os = "macos") {
            "[ -d \"$1\" ] && [ \"$(hash_file \"$1/content\")\" = \"$(cat \"$2\")\" ]"
        } else {
            "[ -f \"$1\" ] && [ \"$(hash_file \"$1\")\" = \"$(cat \"$2\")\" ]"
        };
        let script = format!(
            "DEST={}\ncandidate_matches() {{ [ ! -L \"$1\" ] && [ -f \"$2\" ] && [ ! -L \"$2\" ] && {object}; }}\n{}\n{tail}",
            sh_quote(self.destination().to_str().unwrap()), unix_update_admission(cfg!(target_os = "macos"))
        );
        let mut cmd = Command::new("/bin/bash");
        cmd.arg("-c")
            .arg(script)
            .env("POLARIS_LOCK_HELPER", frozen.path())
            .env(
                "POLARIS_LOCK_HELPER_SHA256",
                polaris_updater::sha256_hex(helper),
            );
        // Only this isolated fixture transfers removal to the production shell.
        let mut frozen = frozen;
        frozen.directory = None;
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        cmd
    }
    fn run(&self) -> std::process::Output {
        self.command("exit 0").output().unwrap()
    }
    fn original_name(&self) -> &'static str {
        if cfg!(target_os = "macos") {
            "original.app"
        } else {
            "original.AppImage"
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn frozen_helper_survives_loss_of_original_mount_and_rejects_changed_or_linked_bytes() {
    let fixture = Fixture::new();
    let mount = fixture.0.join("appimage-mount");
    fs::create_dir(&mount).unwrap();
    let source = mount.join("polaris-helper");
    let bytes = b"actual immutable bundled helper fixture";
    fs::write(&source, bytes).unwrap();
    let expected = polaris_updater::sha256_hex(bytes);
    let frozen = freeze_lease_helper_in(&fixture.0, &source, &expected).unwrap();
    assert!(!frozen.path().starts_with(&mount));
    fs::remove_dir_all(&mount).unwrap();
    assert_eq!(fs::read(frozen.path()).unwrap(), bytes);
    assert_eq!(
        fs::metadata(frozen.path()).unwrap().permissions().mode() & 0o777,
        0o700
    );
    let altered = fixture.0.join("changed");
    fs::write(&altered, b"other version").unwrap();
    assert!(freeze_lease_helper_in(&fixture.0, &altered, &expected).is_err());
    let linked = fixture.0.join("linked");
    std::os::unix::fs::symlink(&altered, &linked).unwrap();
    assert!(freeze_lease_helper_in(&fixture.0, &linked, &expected).is_err());
}

#[test]
fn pre_mutation_and_exact_original_backup_phases_retire_under_lease() {
    for phase in ["staging", "backup"] {
        let fixture = Fixture::new();
        let old = fixture.journal(phase);
        if phase == "backup" {
            fixture.write_artifact(&fixture.destination(), b"original bytes");
            fs::write(
                old.join("original-identity"),
                polaris_updater::sha256_hex(b"original bytes"),
            )
            .unwrap();
        }
        let output = fixture.run();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!old.exists());
    }
}

#[test]
fn interrupted_backup_restores_exact_original_without_overwriting_another_destination() {
    let fixture = Fixture::new();
    let old = fixture.journal("backup");
    fixture.write_artifact(&old.join(fixture.original_name()), b"original bytes");
    assert!(fixture.run().status.success());
    assert_eq!(
        fixture.read_artifact(&fixture.destination()),
        b"original bytes"
    );
    assert!(!old.exists());

    let other = Fixture::new();
    let old = other.journal("backup");
    other.write_artifact(&old.join(other.original_name()), b"original bytes");
    other.write_artifact(&other.destination(), b"unconfirmed installed bytes");
    assert!(!other.run().status.success());
    assert_eq!(
        other.read_artifact(&other.destination()),
        b"unconfirmed installed bytes"
    );
    assert_eq!(
        other.read_artifact(&old.join(other.original_name())),
        b"original bytes"
    );
}

#[test]
fn committed_recovery_requires_actual_candidate_identity_unknown_or_linked_stages_are_retained() {
    let fixture = Fixture::new();
    let old = fixture.journal("committed");
    fs::write(
        old.join("candidate-identity"),
        polaris_updater::sha256_hex(b"candidate"),
    )
    .unwrap();
    fixture.write_artifact(&fixture.destination(), b"wrong candidate");
    assert!(!fixture.run().status.success());
    assert!(old.exists());
    fixture.write_artifact(&fixture.destination(), b"candidate");
    assert!(fixture.run().status.success());
    assert!(!old.exists());
    for phase in ["unknown", "staging"] {
        let fixture = Fixture::new();
        let old = fixture.journal(phase);
        if phase == "staging" {
            fs::remove_file(old.join("transaction-phase")).unwrap();
            std::os::unix::fs::symlink("/dev/null", old.join("transaction-phase")).unwrap();
        }
        assert!(!fixture.run().status.success());
        assert!(old.exists());
    }
}

fn wait_for_marker(child: &mut Child, marker: &Path) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !marker.exists() {
        assert!(
            child.try_wait().unwrap().is_none(),
            "lease owner exited before barrier"
        );
        if Instant::now() >= deadline {
            let _ = child.kill();
            panic!("lease barrier timed out");
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn competing_admission_is_busy_not_an_old_recovery_object_and_abandoned_owner_is_recovered() {
    let fixture = Fixture::new();
    let marker = fixture.0.join("admitted");
    let mut first = fixture
        .command(&format!(
            "printf '%s' \"$WORK\" > {}\nread -r RELEASE",
            sh_quote(marker.to_str().unwrap())
        ))
        .spawn()
        .unwrap();
    wait_for_marker(&mut first, &marker);
    let old = PathBuf::from(fs::read_to_string(&marker).unwrap());
    let second = fixture.run();
    assert!(!second.status.success());
    assert!(old.exists());
    assert!(!String::from_utf8_lossy(&second.stderr).contains("recovery required"));
    first.kill().unwrap();
    first.wait().unwrap();
    assert!(fixture.run().status.success());
    assert!(
        !old.exists(),
        "known staging journal must not permanently block the next update"
    );
}

#[test]
fn cleanup_removes_journal_before_releasing_transaction_lease() {
    for platform in [InstallPlatform::Macos, InstallPlatform::LinuxAppImage] {
        let script = String::from_utf8(script_of(&plan_of(platform)).bytes).unwrap();
        let body = script
            .split("\ncleanup() {\n")
            .nth(1)
            .unwrap()
            .split("\n}\ntrap cleanup EXIT")
            .next()
            .unwrap();
        let remove = body.find("rm -rf \"$WORK\"").unwrap();
        let release = body.find("lease-release --lock").unwrap();
        assert!(remove < release);
        assert!(!script.contains("cp \"${POLARIS_LOCK_HELPER"));
    }
}
