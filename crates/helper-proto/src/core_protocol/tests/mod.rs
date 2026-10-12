use super::*;
#[test]
fn pinned_start_admits_only_exact_frames() {
    let valid = vec![
        "a".repeat(64),
        "/user/runtime.json".into(),
        "/user/start.log".into(),
        "1".into(),
        "123".into(),
    ];
    assert!(PinnedStartParams::parse(&valid).is_some());
    for (index, value) in [
        (0, "../core"),
        (3, "true"),
        (4, "0123"),
        (1, "bad\ncommand"),
    ] {
        let mut invalid = valid.clone();
        invalid[index] = value.into();
        assert!(PinnedStartParams::parse(&invalid).is_none());
    }
    let mut extra = valid.clone();
    extra.push("123".into());
    assert!(PinnedStartParams::parse(&extra).is_none());
    assert!(PinnedStartParams::parse(&valid[..3]).is_none());
}
