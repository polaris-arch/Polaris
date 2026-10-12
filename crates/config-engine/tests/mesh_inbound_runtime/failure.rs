//! Failed WG forward probes keep their original error and collect owned-child evidence.
use super::*;

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
    } = context;
    let source_proof = Path::new("native-payload-evidence").join(if cfg!(windows) {
        "source-win.json"
    } else {
        "source-linux.json"
    });
    let identity = std::fs::read(&source_proof)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|proof| proof.pointer("/input/core").cloned())
        .filter(|identity| identity["path"].as_str() == core.to_str());
    // Request both owned stacks before waiting: the first peer's exit must not consume
    // a full wait window before the other peer receives its diagnostic signal.
    let a_request = request_owned_stack(a_core);
    let b_request = request_owned_stack(b_core);
    let a_snapshot = finish_owned_snapshot(a_core, a_request);
    let b_snapshot = finish_owned_snapshot(b_core, b_request);
    let snapshot = json!({
        "schema":"polaris-wg-failed-probe-diagnostics-v1",
        "phase":phase,"family":family,"policy":policy,"originalProbeError":error,
        "corePath":core,"matchingNativeSourceCoreIdentity":identity,
        "aConfig":a,"bConfig":b,
        "aChild":a_snapshot,"bChild":b_snapshot,
        "scope":"after failed original probe; no timeout/routing/skip changes; missing identity is null"
    });
    // Full run logs already preserve stderr in CI. No extra files or inferred success pins.
    eprintln!("WG_FAILURE_DIAGNOSTICS={snapshot}");
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

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
}
