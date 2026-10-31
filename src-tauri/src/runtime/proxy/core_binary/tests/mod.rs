use super::*;
use crate::runtime::core_identity::{user_store, RecoveryFixture};
#[test]
fn default_selection_reopens_exact_recovery_receipt_and_never_hides_damaged_slot_with_bundle() {
    let f = RecoveryFixture::new("linux");
    let bundle = f.root.join("original-bundle-core");
    std::fs::write(&bundle, b"bundle").unwrap();
    let data = || {
        user_store::selected_at(&f.config, &f.selected)
            .map(|v| v.map(|v| v.directory.join("sing-box")))
    };
    assert_eq!(
        candidate_with_recovery(vec![bundle.clone()], data())
            .unwrap()
            .path,
        bundle
    );
    let stage = f.stage().unwrap();
    user_store::install(&f.config, &stage).unwrap();
    let restored = candidate_with_recovery(vec![bundle.clone()], data())
        .unwrap()
        .path;
    assert_ne!(restored, bundle);
    assert_eq!(
        candidate_with_recovery(vec![bundle.clone()], data())
            .unwrap()
            .path,
        restored
    );
    let receipt = f
        .config
        .join(user_store::USER_STORE)
        .join("manifests")
        .join(format!("{}.json", stage.manifest.id));
    std::fs::remove_file(receipt).unwrap();
    assert!(candidate_with_recovery(vec![bundle.clone()], data()).is_err());
    assert!(bundle.exists());
}
