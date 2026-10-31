use super::*;
use std::os::windows::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::time::Instant;

#[test]
fn native_predicate_matches_all_55_shared_samples() {
    let samples: Vec<(String, String, String, String)> = serde_json::from_str(
        &polaris_source_probe::repo_file!("scripts/fixtures/windows-core-sweep-samples.json"),
    )
    .unwrap();
    assert_eq!(samples.len(), 55);
    for (category, root, image, expected) in samples {
        let actual = match sweep_root(OsStr::new(&root)).unwrap() {
            None => "no-sweep",
            Some(root) => {
                if installed(&root, OsStr::new(&image)) {
                    "kill"
                } else {
                    "keep"
                }
            }
        };
        assert_eq!(actual, expected, "{category}: {root} <- {image}");
    }
    println!("Passed 55 native sweep predicate cases.");
}

#[test]
fn owned_process_waiter() {
    // This test-binary-only child target is not a production role. The parent
    // owns its Child and always reaps it, including negative identity cases.
    if std::env::var_os("POLARIS_INERT_CHILD_WAIT").is_some() {
        if let Some(path) = std::env::var_os("POLARIS_INERT_CHILD_READY") {
            std::fs::write(path, b"C05_SWEEP_READY").unwrap();
        }
        std::thread::sleep(Duration::from_secs(60));
    }
}
struct OwnedChild(Child);
impl Drop for OwnedChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn actual_owned_handles_reject_foreign_image_and_wrong_birth() {
    let dir = tempfile::tempdir().unwrap();
    let install = dir.path().join("installed");
    let eligible = install.join("resources/win/sing-box.exe");
    let foreign = dir.path().join("foreign/sing-box.exe");
    for path in [&eligible, &foreign] {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::copy(std::env::current_exe().unwrap(), path).unwrap();
    }
    let root = sweep_root(install.as_os_str()).unwrap().unwrap();
    for kind in ["installed", "foreign-image", "wrong-birth"] {
        let image = if kind == "foreign-image" {
            &foreign
        } else {
            &eligible
        };
        let ready = dir.path().join(format!("ready-{kind}"));
        let mut child = OwnedChild(
            Command::new(image)
                .args([
                    "--exact",
                    "windows::sweep::tests::owned_process_waiter",
                    "--nocapture",
                ])
                .env("POLARIS_INERT_CHILD_WAIT", "1")
                .env("POLARIS_INERT_CHILD_READY", &ready)
                .creation_flags(CREATE_NO_WINDOW)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        );
        let until = Instant::now() + Duration::from_secs(5);
        loop {
            assert!(
                child.0.try_wait().unwrap().is_none(),
                "owned child exited before its fixture body"
            );
            if std::fs::read(&ready).is_ok_and(|bytes| bytes == b"C05_SWEEP_READY") {
                break;
            }
            assert!(Instant::now() < until, "owned fixture did not enter");
            std::thread::sleep(Duration::from_millis(10));
        }
        let raw = unsafe {
            OpenProcess(
                PROCESS_QUERY_LIMITED_INFORMATION | SYNCHRONIZE,
                0,
                child.0.id(),
            )
        };
        assert!(!raw.is_null());
        let observed = OwnedHandle(raw);
        let born = birth(observed.0).unwrap();
        let observation = Observation {
            pid: child.0.id(),
            image: eligible.as_os_str().into(),
            birth: born + if kind == "wrong-birth" { 10000 } else { 0 },
        };
        let began = Instant::now();
        assert_eq!(sweep(&root, vec![observation]), 0, "{kind}");
        assert!(
            began.elapsed() < Duration::from_secs(8),
            "sweep exceeded its production wait bound: {kind}"
        );
        if kind == "installed" {
            // Bound the original owned handle, then require the production
            // TerminateProcess exit value. Natural exit0 after60s cannot pass.
            let process = child.0.as_raw_handle().cast();
            // SAFETY: original Child still owns this exact process handle.
            assert_eq!(unsafe { WaitForSingleObject(process, 6000) }, WAIT_OBJECT_0);
            let mut code = 0;
            assert_ne!(unsafe { GetExitCodeProcess(process, &mut code) }, 0);
            assert_eq!(code, 1, "eligible child was not terminated by sweep");
            assert_eq!(child.0.wait().unwrap().code(), Some(1));
        } else {
            assert!(
                child.0.try_wait().unwrap().is_none(),
                "foreign/wrong birth child was terminated"
            );
        }
    }
    println!("Passed 3 native owned-handle sweep cases.");
}
