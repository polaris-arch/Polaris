use super::*;

fn synthetic_published_subject() -> ExactStartMetadata {
    ExactStartMetadata {
        nonce: [1; 32],
        subject_uid: 1000,
        plan_id: "plan-a".into(),
        plan_digest: [2; 32],
        candidate_run_id: "direct-a".into(),
        config_digest: [3; 32],
        binary_digest: [4; 32],
        manifest_ref: "mesh-routes/plans/plan-a/manifest.json".into(),
        manifest_digest: [5; 32],
        closure_digest: [6; 32],
        profile: ExecutionProfile::LinuxNoNetworkFixtureV1,
        config_bytes: 512,
        binary_bytes: 1024,
        rule_count: 0,
        immutable_bytes: 1536,
    }
}

#[test]
fn fixed_order_roundtrip_and_every_truncation_rejected() {
    let subject = synthetic_published_subject();
    let capsule = ExactStartCapsule::new(subject.clone()).unwrap();
    let bytes = capsule.encode();
    assert_eq!(&bytes[..MAGIC.len()], MAGIC);
    assert_eq!(&bytes[MAGIC.len()..MAGIC.len() + 4], &[0, 1, 0, 1]);
    assert_eq!(&bytes[MAGIC.len() + 4..MAGIC.len() + 36], &subject.nonce);
    assert_eq!(
        &bytes[MAGIC.len() + 36..MAGIC.len() + 40],
        &subject.subject_uid.to_be_bytes()
    );
    assert_eq!(ExactStartCapsule::parse(&bytes).unwrap(), capsule);
    for end in 0..bytes.len() {
        assert!(
            ExactStartCapsule::parse(&bytes[..end]).is_err(),
            "truncated at {end}"
        );
    }
    let mut trailing = bytes.clone();
    trailing.push(0);
    assert_eq!(
        ExactStartCapsule::parse(&trailing),
        Err(CapsuleError::TrailingBytes)
    );
    assert_eq!(
        ExactStartCapsule::parse(&vec![0; MAX_CAPSULE_BYTES + 1]),
        Err(CapsuleError::ResourceBudget)
    );
}

#[test]
fn unknown_versions_profile_and_string_budget_rejected() {
    let capsule = ExactStartCapsule::new(synthetic_published_subject()).unwrap();
    for offset in [MAGIC.len(), MAGIC.len() + 2] {
        let mut bytes = capsule.encode();
        bytes[offset..offset + 2].copy_from_slice(&2u16.to_be_bytes());
        assert_eq!(
            ExactStartCapsule::parse(&bytes),
            Err(CapsuleError::UnsupportedVersion)
        );
    }
    let mut bytes = capsule.encode();
    let offset = bytes.len() - 20;
    bytes[offset..offset + 2].copy_from_slice(&u16::MAX.to_be_bytes());
    assert_eq!(
        ExactStartCapsule::parse(&bytes),
        Err(CapsuleError::UnsupportedProfile)
    );
    let mut bytes = capsule.encode();
    let offset = MAGIC.len() + 40;
    bytes[offset..offset + 2].copy_from_slice(&u16::MAX.to_be_bytes());
    assert_eq!(
        ExactStartCapsule::parse(&bytes),
        Err(CapsuleError::ResourceBudget)
    );
    let mut bytes = capsule.encode();
    bytes[0] ^= 1;
    assert_eq!(
        ExactStartCapsule::parse(&bytes),
        Err(CapsuleError::InvalidMagic)
    );
}

#[test]
fn whole_expected_subject_required_including_nonce_uid_and_each_digest() {
    let expected = synthetic_published_subject();
    let capsule = ExactStartCapsule::new(expected.clone()).unwrap();
    assert_eq!(capsule.validate_against(&expected), Ok(()));
    let mutations: [fn(&mut ExactStartMetadata); 11] = [
        |s| s.nonce[0] ^= 1,
        |s| s.subject_uid += 1,
        |s| s.plan_digest[0] ^= 1,
        |s| s.candidate_run_id.push('b'),
        |s| s.config_digest[0] ^= 1,
        |s| s.binary_digest[0] ^= 1,
        |s| s.manifest_digest[0] ^= 1,
        |s| s.closure_digest[0] ^= 1,
        |s| s.profile = ExecutionProfile::LinuxB609PlainTcpCheckV1,
        |s| s.config_bytes -= 1,
        |s| {
            s.plan_id = "plan-b".into();
            s.manifest_ref = "mesh-routes/plans/plan-b/manifest.json".into();
        },
    ];
    for mutate in mutations {
        let mut changed = expected.clone();
        mutate(&mut changed);
        assert_eq!(
            capsule.validate_against(&changed),
            Err(CapsuleError::SubjectMismatch)
        );
    }
}

#[test]
fn declared_resources_and_manifest_have_finite_bounds() {
    let mutations: [fn(&mut ExactStartMetadata); 9] = [
        |s| s.config_bytes = 0,
        |s| s.config_bytes = MAX_CONFIG_BYTES + 1,
        |s| s.binary_bytes = 0,
        |s| s.binary_bytes = MAX_BINARY_BYTES + 1,
        |s| s.immutable_bytes = u64::MAX,
        |s| s.immutable_bytes = 0,
        |s| s.rule_count = MAX_RULES + 1,
        |s| s.plan_id = "../outside".into(),
        |s| s.manifest_ref = "mesh-routes/plans/other/manifest.json".into(),
    ];
    for mutate in mutations {
        let mut subject = synthetic_published_subject();
        mutate(&mut subject);
        assert!(ExactStartCapsule::new(subject).is_err());
    }
}

#[test]
fn canonical_closure_has_order_lengths_duplicates_and_path_checks() {
    let a = ImmutableRuleMetadata {
        relative_path: "rules/a.json".into(),
        bytes: 10,
        digest: [7; 32],
    };
    let b = ImmutableRuleMetadata {
        relative_path: "rules/b.json".into(),
        bytes: 20,
        digest: [8; 32],
    };
    let bytes = closure_identity_bytes([3; 32], 512, &[a.clone(), b.clone()]).unwrap();
    assert_eq!(
        bytes,
        closure_identity_bytes([3; 32], 512, &[b, a.clone()]).unwrap()
    );
    assert_ne!(
        bytes,
        closure_identity_bytes([3; 32], 513, std::slice::from_ref(&a)).unwrap()
    );
    assert_eq!(
        closure_identity_bytes([3; 32], 512, &[a.clone(), a]),
        Err(CapsuleError::DuplicateRule)
    );
    for path in [
        "/outside",
        "../outside",
        "a/../b",
        "a//b",
        "a\\b",
        "C:/a",
        ".",
        "a/",
    ] {
        let rule = ImmutableRuleMetadata {
            relative_path: path.into(),
            bytes: 1,
            digest: [0; 32],
        };
        assert_eq!(
            closure_identity_bytes([0; 32], 1, &[rule]),
            Err(CapsuleError::InvalidRulePath)
        );
    }
    let huge = ImmutableRuleMetadata {
        relative_path: "a".into(),
        bytes: u64::MAX,
        digest: [0; 32],
    };
    assert_eq!(
        closure_identity_bytes([0; 32], 1, &[huge]),
        Err(CapsuleError::ResourceBudget)
    );
}

#[test]
fn prepare_metadata_never_upgrades_new_or_old_platform_capability() {
    for platform in [
        Platform::Linux,
        Platform::Mac,
        Platform::Win,
        Platform::Android,
        Platform::Ios,
        Platform::Other,
    ] {
        assert_eq!(
            exact_start_capability(platform),
            ExactStartCapability::Unsupported
        );
    }
}
