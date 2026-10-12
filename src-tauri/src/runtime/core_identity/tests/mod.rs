use super::*;
#[test]
fn all_selected_desktop_rows_have_complete_content_identities() {
    for platform in ["linux", "win", "mac-x64", "mac-arm64"] {
        let manifest = selected_manifest(platform).expect("actual pinned row");
        assert_eq!(
            manifest.files.len(),
            if platform.starts_with("mac-") { 1 } else { 2 }
        );
        assert_eq!(
            selected_id(platform)
                .expect("framed identity")
                .as_str()
                .len(),
            64
        );
    }
    assert!(selected_manifest("android").is_err());
}
