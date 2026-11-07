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
        PINNED_EXECUTION_DISCRIMINATOR.into(),
        "a".repeat(64),
        "/user/runtime.json".into(),
        "/user/start.log".into(),
        "1".into(),
        "123".into(),
    ];
    assert!(PinnedStartParams::parse(&valid).is_some());
    for (index, value) in [
        (0, "wrong-marker"),
        (1, "../core"),
        (4, "true"),
        (5, "0123"),
        (2, "bad\ncommand"),
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

// Frozen v2 parser from b123f5e5; method name and Self binding made explicit.
// The CoreId parser is unchanged. This is an old-consumer regression fixture,
// not an alternate production decoder or an execution factory.
fn frozen_v2_parse(lines: &[String]) -> Option<PinnedStartParams> {
    if !(4..=5).contains(&lines.len()) {
        return None;
    }
    let id = CoreId::parse(&lines[0]).ok()?;
    let cfg = &lines[1];
    let log = &lines[2];
    if cfg.is_empty()
        || [cfg, log]
            .into_iter()
            .any(|value| value.contains(['\n', '\r', '\0']))
    {
        return None;
    }
    let fwd = match lines[3].as_str() {
        "0" => false,
        "1" => true,
        _ => return None,
    };
    let parent_pid = match lines.get(4).map(String::as_str) {
        None | Some("") => None,
        Some(value) => {
            let pid = value.parse::<std::num::NonZeroU32>().ok()?;
            if pid.to_string() != value {
                return None;
            }
            Some(pid.get())
        }
    };
    Some(PinnedStartParams {
        runtime_directory: None,
        id,
        common: StartParams {
            cfg: cfg.clone(),
            log: log.clone(),
            fwd,
            parent_pid,
        },
    })
}
#[test]
fn v3_pinned_frames_are_rejected_by_frozen_v2_before_any_birth() {
    use crate::{Platform, Request, StartParams};
    let id = CoreId::parse(&"a".repeat(64)).unwrap();
    assert!(CoreId::parse(PINNED_EXECUTION_DISCRIMINATOR).is_err());
    for parent_pid in [None, Some(42)] {
        let params = PinnedStartParams {
            runtime_directory: None,
            id: id.clone(),
            common: StartParams {
                cfg: "/caller/cfg".into(),
                log: "".into(),
                fwd: false,
                parent_pid,
            },
        };
        for (platform, request) in [
            (Platform::Linux, Request::LinuxStartPinned(params.clone())),
            (Platform::Mac, Request::NativeStartPinned(params.clone())),
            (Platform::Win, Request::NativeStartPinned(params.clone())),
        ] {
            let args = request.args_lines();
            assert_eq!(args[0], PINNED_EXECUTION_DISCRIMINATOR);
            assert_eq!(PinnedStartParams::parse(&args), Some(params.clone()));
            assert!(
                frozen_v2_parse(&args).is_none(),
                "old {platform:?} must reject"
            );
            let legacy = args[1..].to_vec();
            assert!(frozen_v2_parse(&legacy).is_some());
            assert!(PinnedStartParams::parse(&legacy).is_none());
            let mut duplicate = args.clone();
            duplicate.insert(1, PINNED_EXECUTION_DISCRIMINATOR.into());
            assert!(PinnedStartParams::parse(&duplicate).is_none());
        }
    }
}

fn frozen_v3_parse(lines: &[String]) -> Option<PinnedStartParams> {
    let (marker, rest) = lines.split_first()?;
    if marker != PINNED_EXECUTION_DISCRIMINATOR || !(4..=5).contains(&rest.len()) {
        return None;
    }
    frozen_v2_parse(rest)
}
#[test]
fn explicit_runtime_directory_is_optional_framed_data_and_old_v3_rejects_it() {
    use crate::{Request, StartParams};
    for parent_pid in [None, Some(42)] {
        let mut params = PinnedStartParams {
            id: CoreId::parse(&"a".repeat(64)).unwrap(),
            common: StartParams {
                cfg: "/state/profiles/current.json".into(),
                log: String::new(),
                fwd: false,
                parent_pid,
            },
            runtime_directory: None,
        };
        let legacy = Request::LinuxStartPinned(params.clone()).args_lines();
        assert_eq!(frozen_v3_parse(&legacy), Some(params.clone()));
        params.runtime_directory = Some("/state with spaces/测试".into());
        let frame = Request::LinuxStartPinned(params.clone()).args_lines();
        assert_eq!(frame.len(), 7);
        assert_eq!(
            frame[5],
            parent_pid.map(|pid| pid.to_string()).unwrap_or_default()
        );
        assert_eq!(frame[6], "runtime-root-v1=/state with spaces/测试");
        assert_eq!(PinnedStartParams::parse(&frame), Some(params));
        assert!(frozen_v3_parse(&frame).is_none());
        for value in [
            "",
            "runtime-root-v1=",
            "runtime-root-v1=/bad\ncommand",
            "runtime-root-v1=/bad\rcommand",
            "runtime-root-v1=/bad\0command",
            "runtime-root-v2=/state",
        ] {
            let mut invalid = frame.clone();
            invalid[6] = value.into();
            assert!(PinnedStartParams::parse(&invalid).is_none());
        }
        let mut extra = frame.clone();
        extra.push("runtime-root-v1=/other".into());
        assert!(PinnedStartParams::parse(&extra).is_none());
    }
}
