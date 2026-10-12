use super::*;
use std::time::Duration;
struct Observed {
    lsof: Vec<u8>,
    fail: bool,
    image: String,
}
impl CommandRunner for Observed {
    fn run(&self, _: Duration, _: &str, _: &[&str]) -> Result<(), RunError> {
        panic!("port observations never signal processes")
    }
    fn output(&self, _: Duration, program: &str, _: &[&str]) -> Result<Vec<u8>, RunError> {
        if self.fail {
            return Err(RunError::Timeout);
        }
        Ok(if program == LSOF_BIN {
            self.lsof.clone()
        } else {
            self.image.as_bytes().to_vec()
        })
    }
    fn combined(&self, _: Duration, _: &str, _: &[&str]) -> Result<Vec<u8>, RunError> {
        panic!("no mutation")
    }
}
#[test]
fn observed_singbox_and_foreign_images_are_both_reported_without_signals() {
    for image in [
        "/old/app/sing-box",
        "/usr/bin/httpd",
        "",
        "wrong\nframe|fake",
    ] {
        let runner = Observed {
            lsof: b"123\n".to_vec(),
            fail: false,
            image: image.into(),
        };
        let reply = run_freeport(&runner, "9090");
        assert!(matches!(
            reply,
            Response::Ok(ResponseKind::FreePort(FreePort::Foreign { .. }))
        ));
        assert!(!reply.to_wire_line().contains('\n'));
    }
}
#[test]
fn only_an_observed_empty_scan_reports_free() {
    let runner = Observed {
        lsof: Vec::new(),
        fail: false,
        image: String::new(),
    };
    assert!(matches!(
        run_freeport(&runner, "9090"),
        Response::Ok(ResponseKind::FreePort(FreePort::Free))
    ));
    let failed = Observed {
        fail: true,
        ..runner
    };
    assert!(matches!(run_freeport(&failed, "9090"), Response::Err(_)));
}
#[test]
fn malformed_ports_and_pid_output_are_rejected() {
    let runner = Observed {
        lsof: b"123 garbage\n".to_vec(),
        fail: false,
        image: String::new(),
    };
    for port in ["", "-1", "90 90", "9090;kill"] {
        assert!(matches!(run_freeport(&runner, port), Response::Err(_)));
    }
    assert!(matches!(run_freeport(&runner, "9090"), Response::Err(_)));
}
