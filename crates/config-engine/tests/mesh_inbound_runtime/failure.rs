//! Failed WG forward probes keep their original error and collect owned-child evidence.
use super::*;
#[path = "bounded_inspector.rs"]
mod bounded_inspector;

fn request_owned_stack(running: &mut Running) -> Value {
    let started = Instant::now();
    let before = running.log();
    let pid = running.child.id();
    let state = running.child.try_wait();
    let mut signal_receipt = json!({"requested":false});
    if matches!(state, Ok(None)) {
        // The child has not been reaped, so this owned PID cannot have been reused.
        // SIGQUIT is diagnostic and terminal; it is requested only after the probe failed.
        #[cfg(unix)]
        {
            signal_receipt = match std::process::Command::new("kill")
                .args(["-QUIT", &pid.to_string()])
                .output()
            {
                Ok(output) => json!({
                    "requested":true,"signal":"SIGQUIT","commandSucceeded":output.status.success(),
                    "stderr":String::from_utf8_lossy(&output.stderr)
                }),
                Err(error) => json!({"requested":false,"error":error.to_string()}),
            };
        }
        #[cfg(not(unix))]
        {
            signal_receipt = json!({"requested":false,"unsupported":"SIGQUIT on this platform"});
        }
    }
    json!({
        "pid":pid,"initialChildState":format!("{state:?}"),"stackRequest":signal_receipt,
        "beforeStackRequest":before,"requestElapsedMicros":started.elapsed().as_micros()
    })
}

fn finish_owned_snapshot(running: &mut Running, mut receipt: Value) -> Value {
    if receipt["stackRequest"]["commandSucceeded"] == true {
        let deadline = Instant::now() + Duration::from_secs(1);
        while Instant::now() < deadline {
            if !matches!(running.child.try_wait(), Ok(None)) {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
    }
    receipt["afterStackRequest"] = json!(running.log());
    receipt["finalChildState"] = json!(format!("{:?}", running.child.try_wait()));
    receipt
}

pub(super) struct Context<'a> {
    pub phase: &'a str,
    pub family: &'a str,
    pub policy: &'a str,
    pub error: &'a str,
    pub core: &'a Path,
    pub a: &'a Value,
    pub b: &'a Value,
    pub listener_observations: &'a Value,
}

fn source_core_identity_from_repo(repo: &Path, core: &Path) -> Option<Value> {
    let runtime_input = repo.join("native-payload-evidence").join(if cfg!(windows) {
        "runtime-input-win.json"
    } else {
        "runtime-input-linux.json"
    });
    if runtime_input.exists() {
        return std::fs::read(runtime_input)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            .filter(|input| {
                matches!(
                    input["schema"].as_str(),
                    Some(
                        "polaris-native-release-payload-input-v1"
                            | "polaris-native-payload-input-v1"
                    )
                )
            })
            .and_then(|input| input.get("core").cloned())
            .filter(|identity| identity["path"].as_str() == core.to_str());
    }
    let source_proof = repo.join("native-payload-evidence").join(if cfg!(windows) {
        "source-win.json"
    } else {
        "source-linux.json"
    });
    std::fs::read(source_proof)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|proof| proof.pointer("/input/core").cloned())
        .filter(|identity| identity["path"].as_str() == core.to_str())
}

fn actual_core_input(repo: &Path, core: &Path) -> Value {
    // Reinspect the precise path used by this failed probe. The existing
    // inspector checks actual SHA/BuildID against the frozen consumption
    // receipt and source contract; it does not run a TUN or traffic probe.
    if !cfg!(any(target_os = "linux", windows)) {
        return json!({"verified":false,"error":"dynamic metadata inspector is Linux/Windows only"});
    }
    let key = if cfg!(windows) { "win" } else { "linux" };
    let library = core.with_file_name(if cfg!(windows) {
        "libcronet.dll"
    } else {
        "libcronet.so"
    });
    let mut command = std::process::Command::new("node");
    command
        .arg(repo.join("scripts/native-payload-input.mjs"))
        .arg(key)
        .arg(core)
        .arg(library)
        .current_dir(repo);
    let output =
        bounded_inspector::capture(&mut command, Duration::from_secs(20), 8 * 1024 * 1024, 8192);
    match output {
        Ok(output)
            if output.failure.is_none()
                && output.cleanup_complete
                && output
                    .status
                    .as_ref()
                    .is_some_and(|status| status.success()) =>
        {
            match serde_json::from_slice::<Value>(&output.stdout) {
                Ok(input)
                    if matches!(
                        input["schema"].as_str(),
                        Some(
                            "polaris-native-release-payload-input-v1"
                                | "polaris-native-payload-input-v1"
                        )
                    ) =>
                {
                    json!({"verified":true,"input":input,
                        "stdoutBytes":output.stdout_observed,"stderrBytes":output.stderr_observed,
                        "deadlineSeconds":20,"stdoutMaximumBytes":8*1024*1024,"stderrMaximumBytes":8192})
                }
                _ => json!({"verified":false,"error":"actual metadata inspector response differs"}),
            }
        }
        Ok(output) => {
            json!({"verified":false,"status":output.status.as_ref().and_then(|s|s.code()),
            "error":output.failure,"cleanupComplete":output.cleanup_complete,
            "stdoutBytesObservedAtLeast":output.stdout_observed,"stderrBytesObservedAtLeast":output.stderr_observed,
            "deadlineSeconds":20,"stdoutMaximumBytes":8*1024*1024,"stderrMaximumBytes":8192,
            "stdoutPrefix":String::from_utf8_lossy(&output.stdout[..output.stdout.len().min(8192)]),
            "stderr":String::from_utf8_lossy(&output.stderr)})
        }
        Err(error) => json!({"verified":false,"error":error.to_string()}),
    }
}

fn matched_core_identity(recorded: Option<Value>, actual: &Value, core: &Path) -> Option<Value> {
    recorded.filter(|identity| {
        actual["verified"] == true
            && actual.pointer("/input/core") == Some(identity)
            && identity["path"].as_str() == core.to_str()
    })
}

pub(super) fn capture(context: Context<'_>, a_core: &mut Running, b_core: &mut Running) {
    let Context {
        phase,
        family,
        policy,
        error,
        core,
        a,
        b,
        listener_observations,
    } = context;
    let repo = support::core_locator::repo_root();
    let recorded = source_core_identity_from_repo(&repo, core);
    // Request both owned stacks before waiting: the first peer's exit must not consume
    // a full wait window before the other peer receives its diagnostic signal.
    let a_request = request_owned_stack(a_core);
    let b_request = request_owned_stack(b_core);
    let a_snapshot = finish_owned_snapshot(a_core, a_request);
    let b_snapshot = finish_owned_snapshot(b_core, b_request);
    let snapshot = json!({
        "schema":"polaris-wg-failed-probe-diagnostics-v1",
        "phase":phase,"family":family,"policy":policy,"originalProbeError":error,
        "corePath":core,"matchingNativeSourceCoreIdentity":null,
        "recordedCoreInputIdentity":recorded,"listenerObservationsBeforeSIGQUIT":listener_observations,
        "aConfig":a,"bConfig":b,
        "aChild":a_snapshot,"bChild":b_snapshot,
        "scope":"original failed probe and pre-SIGQUIT listener observations; immediate owned dual stack requests; no timeout/routing/skip changes"
    });
    // Full run logs already preserve stderr in CI. No extra files or inferred success pins.
    eprintln!("WG_FAILURE_DIAGNOSTICS={snapshot}");
    // Freeze the original error, configuration, listener windows and both
    // stacks before the independently bounded metadata collector can fail.
    let actual = actual_core_input(&repo, core);
    let identity = matched_core_identity(recorded, &actual, core);
    eprintln!(
        "WG_CORE_IDENTITY_DIAGNOSTICS={}",
        json!({
            "phase":phase,"family":family,"policy":policy,"corePath":core,
            "aPID":a_core.child.id(),"bPID":b_core.child.id(),
            "matchingNativeSourceCoreIdentity":identity,"actualCoreInputObservation":actual,
            "scope":"metadata reinspection after frozen original failure; unmatched/timeout/overflow remains unverified"
        })
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn owned_sigquit_capture_keeps_original_and_signal_response() {
        let temp = tempfile::tempdir().unwrap();
        let log = temp.path().join("owned.stderr");
        let child = std::process::Command::new("sh")
            .args(["-c", "trap 'echo owned-stack-response >&2; exit 0' QUIT; echo owned-ready >&2; while :; do sleep 0.01; done"])
            .stdout(Stdio::null())
            .stderr(std::fs::File::create(&log).unwrap())
            .spawn().unwrap();
        let mut running = Running { child, log };
        assert!(eventually(|| running.log().contains("owned-ready")));
        let request = request_owned_stack(&mut running);
        let receipt = finish_owned_snapshot(&mut running, request);
        assert!(receipt["beforeStackRequest"]
            .as_str()
            .unwrap()
            .contains("owned-ready"));
        assert_eq!(receipt["stackRequest"]["commandSucceeded"], true);
        assert!(receipt["afterStackRequest"]
            .as_str()
            .unwrap()
            .contains("owned-stack-response"));
    }

    #[cfg(unix)]
    #[test]
    fn already_reaped_child_never_receives_a_stack_signal() {
        let temp = tempfile::tempdir().unwrap();
        let log = temp.path().join("exited.stderr");
        let mut child = std::process::Command::new("sh")
            .args(["-c", "echo original-failure >&2; exit 7"])
            .stderr(std::fs::File::create(&log).unwrap())
            .spawn()
            .unwrap();
        assert_eq!(child.wait().unwrap().code(), Some(7));
        let mut running = Running { child, log };
        let request = request_owned_stack(&mut running);
        let receipt = finish_owned_snapshot(&mut running, request);
        assert_eq!(receipt["stackRequest"]["requested"], false);
        assert!(receipt["beforeStackRequest"]
            .as_str()
            .unwrap()
            .contains("original-failure"));
        assert_eq!(receipt["beforeStackRequest"], receipt["afterStackRequest"]);
    }

    fn fixture_proof(repo: &Path, core: &Path) -> Value {
        let folder = repo.join("native-payload-evidence");
        std::fs::create_dir_all(&folder).unwrap();
        let identity = json!({"path":core,"sha256":"test-fixture-only-not-a-production-pin"});
        let path = folder.join(if cfg!(windows) {
            "source-win.json"
        } else {
            "source-linux.json"
        });
        std::fs::write(
            path,
            serde_json::to_vec(&json!({"input":{"core":identity}})).unwrap(),
        )
        .unwrap();
        identity
    }

    #[test]
    fn crate_cwd_reads_proof_from_the_explicit_repo_root() {
        // Cargo starts integration tests at the crate, while the workflow writes at repo root.
        assert_eq!(
            std::env::current_dir().unwrap().canonicalize().unwrap(),
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .canonicalize()
                .unwrap()
        );
        let repo = tempfile::tempdir().unwrap();
        let core = repo.path().join("resources/linux/sing-box");
        let identity = fixture_proof(repo.path(), &core);
        assert_eq!(
            source_core_identity_from_repo(repo.path(), &core),
            Some(identity)
        );
    }

    #[test]
    fn wrong_core_path_never_supplies_an_identity() {
        let repo = tempfile::tempdir().unwrap();
        let core = repo.path().join("resources/linux/sing-box");
        fixture_proof(repo.path(), &core);
        assert_eq!(
            source_core_identity_from_repo(repo.path(), &repo.path().join("other-core")),
            None
        );
    }

    #[test]
    fn missing_repo_root_proof_stays_unbound() {
        let repo = tempfile::tempdir().unwrap();
        assert_eq!(
            source_core_identity_from_repo(repo.path(), &repo.path().join("core")),
            None
        );
    }

    #[test]
    fn changed_bytes_buildid_or_path_and_failed_inspector_never_bind_the_failed_probe() {
        let core = Path::new("/controlled/resources/linux/sing-box");
        let recorded = json!({"path":core,"sha256":"a".repeat(64),"buildID":"controlled-id"});
        let actual = json!({"verified":true,"input":{"core":recorded}});
        assert_eq!(
            matched_core_identity(Some(recorded.clone()), &actual, core),
            Some(recorded.clone())
        );
        for field in ["sha256", "buildID", "path"] {
            let mut changed = actual.clone();
            changed["input"]["core"][field] = json!("changed");
            assert!(matched_core_identity(Some(recorded.clone()), &changed, core).is_none());
        }
        let mut failed = actual.clone();
        failed["verified"] = json!(false);
        assert!(matched_core_identity(Some(recorded.clone()), &failed, core).is_none());
        assert!(matched_core_identity(None, &actual, core).is_none());
    }

    #[test]
    fn runtime_input_is_used_without_relabelling_it_as_a_native_capi_proof() {
        let repo = tempfile::tempdir().unwrap();
        let core = repo.path().join("resources/linux/sing-box");
        fixture_proof(repo.path(), &core);
        let identity =
            json!({"path":core,"sha256":"a".repeat(64),"buildID":"actual-metadata-control"});
        let path = repo
            .path()
            .join("native-payload-evidence")
            .join(if cfg!(windows) {
                "runtime-input-win.json"
            } else {
                "runtime-input-linux.json"
            });
        std::fs::write(
            &path,
            serde_json::to_vec(&json!({
                "schema":"polaris-native-release-payload-input-v1","core":identity
            }))
            .unwrap(),
        )
        .unwrap();
        assert_eq!(
            source_core_identity_from_repo(repo.path(), &core),
            Some(identity)
        );
        std::fs::write(&path, b"not JSON").unwrap();
        assert!(source_core_identity_from_repo(repo.path(), &core).is_none());
    }
}
