use super::*;
#[test]
fn native_sid_has_one_canonical_namespace_key() {
    let sid = CallerIdentity::windows("S-1-5-21-1-2-3-1000").unwrap();
    assert_eq!(sid.key(), "sS-1-5-21-1-2-3-1000");
    for invalid in [
        "S-1-05-21-1",
        "S-1-5-21-01",
        "S-2-5-21-1",
        "S-1-5-4294967296",
        "S-1-281474976710656-1",
        "S-1-5-21\n--root",
    ] {
        assert!(
            CallerIdentity::windows(invalid).is_none(),
            "accepted {invalid}"
        );
    }
}
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
