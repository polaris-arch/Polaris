use super::*;
use std::io::Write;

fn fixture(label: &str, bytes: &[u8]) -> (FixtureDirectory, UntrustedPayload, SelectedPolicy) {
    let directory = std::env::temp_dir().join(format!(
        "polaris-direct-fixture-{}-{label}",
        std::process::id()
    ));
    std::fs::create_dir(&directory).unwrap();
    let binary = directory.join("sing-box");
    std::fs::write(&binary, bytes).unwrap();
    let policy = SelectedPolicy {
        manifest: CoreManifest {
            platform: "fixture".into(),
            version: "fixture".into(),
            provider_fingerprint: "a".repeat(64),
            helper_sha256: None,
            cleaner_sha256: None,
            files: vec![polaris_helper_proto::core_manifest::CoreFile {
                name: "sing-box".into(),
                bytes: bytes.len() as u64,
                sha256: sha256_hex(bytes),
            }],
        },
        id: CoreId::parse(&"d".repeat(64)).unwrap(),
        source: "b".repeat(40),
        tree: "c".repeat(40),
    };
    (
        FixtureDirectory(directory),
        UntrustedPayload::bundled(binary),
        policy,
    )
}

#[test]
fn matching_members_pass_and_one_changed_byte_is_refused() {
    let (_directory, input, policy) = fixture("match", b"selected core bytes");
    if platform::privileged() {
        assert_eq!(
            protect_members(&input, &policy).unwrap_err().kind,
            AdmissionErrorKind::UnsupportedExactObject
        );
        return;
    }
    // An ordinary process selects the by-path loader.
    assert!(matches!(
        platform::protect_members(&input, &policy),
        Ok(platform::ProtectedMembers::Direct(_))
    ));
    assert!(protect_members(&input, &policy).unwrap().is_current());
    std::fs::write(&input.binary, b"selected core bytez").unwrap();
    let error = protect_members(&input, &policy).unwrap_err();
    assert_eq!(error.kind, AdmissionErrorKind::Mismatch);
    assert_eq!(error.member.as_deref(), Some("sing-box"));
}

#[test]
fn a_member_changed_after_hashing_is_no_longer_current() {
    let (_directory, input, policy) = fixture("changed", b"selected core bytes");
    if platform::privileged() {
        return;
    }
    let members = protect_members(&input, &policy).unwrap();
    std::fs::OpenOptions::new()
        .append(true)
        .open(&input.binary)
        .unwrap()
        .write_all(b"!")
        .unwrap();
    assert!(!members.is_current());
}

#[cfg(windows)]
#[allow(unsafe_code, reason = "thread impersonation is Win32 FFI")]
#[test]
fn an_impersonating_thread_never_falls_back_to_the_by_path_loader() {
    use windows_sys::Win32::Security::{ImpersonateSelf, RevertToSelf, SecurityImpersonation};
    let (_directory, input, policy) = fixture("impersonating", b"selected core bytes");
    // SAFETY: no pointer arguments; this test thread reverts before asserting.
    assert_ne!(unsafe { ImpersonateSelf(SecurityImpersonation) }, 0);
    let privileged = platform::privileged();
    let selected = platform::protect_members(&input, &policy);
    let direct = protect_members(&input, &policy);
    // SAFETY: no arguments; ends the impersonation started above.
    assert_ne!(unsafe { RevertToSelf() }, 0);
    assert!(privileged);
    assert!(selected.is_err());
    assert_eq!(
        direct.unwrap_err().kind,
        AdmissionErrorKind::UnsupportedExactObject
    );
}
