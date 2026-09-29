use super::super::*;

#[test]
fn pure_builtin_manual_update_has_every_builtin_target() {
    let (external, builtins) = manual_update_targets(Vec::new());
    assert!(external.is_empty());
    let expected = builtin_geo_rulesets();
    assert!(!expected.is_empty());
    assert_eq!(
        builtins.iter().map(|b| &b.tag).collect::<Vec<_>>(),
        expected.iter().map(|b| &b.tag).collect::<Vec<_>>()
    );
}

#[test]
fn manual_update_deduplicates_registered_ids_and_builtin_overlap() {
    let first_builtin = builtin_geo_rulesets().remove(0);
    let (external, builtins) = manual_update_targets(vec![
        json!({ "id": builtin_id_for(&first_builtin.tag) }),
        json!({ "id": "external-1" }),
        json!({ "id": "external-1" }),
        json!({ "name": "malformed without id" }),
    ]);
    assert_eq!(external.len(), 2);
    assert_eq!(external[0]["id"], "external-1");
    assert!(external[1].get("id").is_none());
    assert_eq!(builtins.len(), builtin_geo_rulesets().len());
}
