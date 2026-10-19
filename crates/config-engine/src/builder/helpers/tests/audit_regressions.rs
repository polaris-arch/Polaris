use super::super::*;
struct Node(&'static str, &'static str);
impl ServerLike for Node {
    fn id(&self) -> &str {
        self.0
    }
    fn name(&self) -> &str {
        self.1
    }
}
#[test]
fn node_and_app_names_cannot_capture_custom_selectors() {
    let map = build_id_to_tag_map(&[
        Node("a", "rule-sel-r1"),
        Node("b", "node-rule-sel-r1"),
        Node("c", "rule-sel-r1"),
    ]);
    assert!(map.values().all(|tag| !tag.starts_with("rule-sel-")));
    assert_eq!(
        map.values()
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        3
    );
    assert_eq!(custom_rule_selector_tag("r1"), "rule-sel-r1");
    assert_ne!(custom_rule_selector_tag("app-foo"), "rule-sel-app-foo");
    assert_ne!(
        custom_rule_selector_tag("~61pp-foo"),
        custom_rule_selector_tag("app-foo")
    );
}

#[test]
fn generated_helper_names_are_reserved_before_display_names() {
    let map = build_id_to_tag_map(&[
        Node("a", "stls-out-X"),
        Node("b", "probe-selector-0"),
        Node("c", "node-stls-out-X"),
        Node("d", "node-probe-selector-0"),
    ]);
    assert!(map
        .values()
        .all(|tag| !tag.starts_with("stls-out-") && !tag.starts_with("probe-selector-")));
    assert_eq!(map.values().collect::<BTreeSet<_>>().len(), 4);
}
