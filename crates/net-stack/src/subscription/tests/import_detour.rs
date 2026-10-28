use super::*;
fn node(id: &str, address: &str, detour: Option<&str>) -> ServerConfig {
    ServerConfig {
        id: id.into(),
        name: id.into(),
        protocol: Protocol::Socks,
        address: address.into(),
        port: 1080,
        detour: detour.map(str::to_owned),
        ..Default::default()
    }
}
#[test]
fn dedupe_remaps_equivalent_dependency_graphs_without_dangling_ids() {
    let out = dedupe_by_fingerprint(vec![
        node("exit-a", "e", Some("front-b")),
        node("exit-b", "e", Some("front-a")),
        node("front-a", "f", None),
        node("front-b", "f", None),
    ]);
    assert_eq!(out.len(), 2);
    assert_eq!(out[0].id, "exit-a");
    assert_eq!(out[0].detour.as_deref(), Some("front-a"));
    assert_eq!(out[1].id, "front-a");
}
#[test]
fn distinct_or_invalid_dependency_graphs_are_never_coalesced() {
    let out = dedupe_by_fingerprint(vec![
        node("exit-a", "e", Some("front-a")),
        node("exit-b", "e", Some("front-b")),
        node("front-a", "a", None),
        node("front-b", "b", None),
    ]);
    assert_eq!(out.len(), 4);
    let cycle = dedupe_by_fingerprint(vec![node("a", "e", Some("b")), node("b", "e", Some("a"))]);
    assert_eq!(cycle.len(), 2);
    assert_eq!(cycle[0].detour.as_deref(), Some("b"));
    assert_eq!(cycle[1].detour.as_deref(), Some("a"));
}
#[test]
fn graph_identity_ignores_id_churn_but_keeps_namespaces_and_edges() {
    let old = vec![
        ("old-exit".into(), "exit".into(), Some("old-front".into())),
        ("old-front".into(), "front".into(), None),
    ];
    let new = vec![
        ("new-front".into(), "front".into(), None),
        ("new-exit".into(), "exit".into(), Some("new-front".into())),
    ];
    let classes = dependency_identity_classes(&[old, new]);
    assert_eq!(classes[0][0], classes[1][1]);
    assert_eq!(classes[0][1], classes[1][0]);
    let invalid =
        dependency_identity_classes(&[vec![("a".into(), "a".into(), Some("missing".into()))]]);
    assert_eq!(invalid, vec![vec![None]]);
}
