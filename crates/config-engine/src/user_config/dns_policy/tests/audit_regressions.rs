use super::*;
#[test]
fn user_resolver_ids_do_not_alias_after_tag_encoding() {
    let ids = ["a b", "a/b", "a-b", "a~20b", "é", "~c3~a9", "safe_ID-1"];
    let tags: std::collections::BTreeSet<_> = ids.iter().map(|id| dns_server_tag(id)).collect();
    assert_eq!(tags.len(), ids.len());
    assert_eq!(dns_server_tag("safe_ID-1"), "dns-user-safe_ID-1");
}
