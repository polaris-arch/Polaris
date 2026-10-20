use super::super::*;
#[tokio::test]
async fn new_oversized_pattern_rejects_before_platform_or_binary_access() {
    assert_eq!(
        check_rule_patterns(std::path::Path::new("unused"), &["a".repeat(1025)]).await,
        Some(false)
    );
}

#[test]
fn only_inert_route_predicates_are_constructed_and_strings_are_preserved() {
    let patterns = vec![
        r"\p{ASCII}".to_owned(),
        "(a{1001}){0}".into(),
        "[[:x]".into(),
    ];
    let config = rule_pattern_check_config(&patterns);
    assert_eq!(config.as_object().unwrap().len(), 2);
    assert_eq!(config["route"]["rules"][0]["domain_regex"], json!(patterns));
    assert_eq!(
        config["outbounds"],
        json!([{"type":"direct","tag":"rule-check-direct"}])
    );
    for key in ["inbounds", "endpoints", "dns", "experimental", "rule_set"] {
        assert!(config.get(key).is_none());
    }
}
