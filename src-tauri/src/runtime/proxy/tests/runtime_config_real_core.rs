//! Explicit GitHub-only, source-admitted noTUN consumer tests. There is no RPC mock.
//! Diagnostic source and formal Release inputs are distinct; neither proves TUN/mobile acceptance.
use super::*;
use polaris_singbox_grpc::{daemon, ClientError, Endpoint, RuntimeConfigError, SingBoxApiClient};
use serde_json::json;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const DOMAIN: &str = "runtime-loopback.test";
const FIXTURE_RULE_SET: &str = "custom-rule-loop-route";
const IO_BOUND: Duration = Duration::from_secs(3);
const METHODS: [&str; 6] = [
    "GetRuntimeCapabilities",
    "PlanRuntimeConfig",
    "ApplyRuntimeConfig",
    "GetRuntimeConfigStatus",
    "RetryRuntimePersistence",
    "AcknowledgeRuntimePersistence",
];

fn hex(value: &str, len: usize) -> bool {
    value.len() == len
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn regular(path: &Path, limit: u64) -> Vec<u8> {
    assert!(path.is_absolute(), "absolute admitted path required");
    let metadata = std::fs::symlink_metadata(path).expect("admitted file metadata");
    assert!(metadata.file_type().is_file(), "regular file required");
    assert!(metadata.len() > 0 && metadata.len() <= limit);
    assert_eq!(
        path.canonicalize().unwrap(),
        path,
        "canonical path required"
    );
    let bytes = std::fs::read(path).expect("read admitted file");
    assert_eq!(bytes.len() as u64, metadata.len());
    bytes
}

// Formal bindings retain the existing production selection and original receipt.
// No writable receipt may choose a different compiled contract/core/sidecar.
fn formal_bytes(path: &Path, limit: u64) -> (Vec<u8>, u32) {
    assert!(path.is_absolute());
    assert_eq!(path.canonicalize().unwrap(), path);
    let stamp = |m: &std::fs::Metadata| {
        (
            m.dev(),
            m.ino(),
            m.len(),
            m.mode(),
            m.uid(),
            m.nlink(),
            m.mtime(),
            m.mtime_nsec(),
            m.ctime(),
            m.ctime_nsec(),
        )
    };
    let initial = std::fs::symlink_metadata(path).unwrap();
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_NONBLOCK)
        .open(path)
        .unwrap();
    let before = file.metadata().unwrap();
    assert_eq!(stamp(&initial), stamp(&before));
    assert!(before.is_file() && before.len() > 0 && before.len() <= limit);
    assert_eq!(before.nlink(), 1);
    assert_eq!(before.uid(), std::fs::metadata("/proc/self").unwrap().uid());
    assert_eq!(before.mode() & 0o7022, 0);
    let mut bytes = Vec::new();
    (&file)
        .take(limit.checked_add(1).unwrap())
        .read_to_end(&mut bytes)
        .unwrap();
    assert!(bytes.len() as u64 <= limit);
    assert_eq!(bytes.len() as u64, before.len());
    assert_eq!(stamp(&before), stamp(&file.metadata().unwrap()));
    assert_eq!(
        stamp(&before),
        stamp(&std::fs::symlink_metadata(path).unwrap())
    );
    assert_eq!(path.canonicalize().unwrap(), path);
    (bytes, before.mode() & 0o7777)
}

fn formal_pin(value: &Value, expected: &Path, limit: u64) -> Vec<u8> {
    assert_eq!(value["path"].as_str(), expected.to_str());
    let (bytes, mode) = formal_bytes(expected, limit);
    assert_eq!(value["bytes"].as_u64(), Some(bytes.len() as u64));
    assert_eq!(value["sha256"], polaris_updater::sha256_hex(&bytes));
    assert_eq!(value["mode"].as_u64(), Some(u64::from(mode)));
    bytes
}

// Same sorted-key ASCII JSON as the existing release-consumer portableCanonical.
// Receipt numbers are bounded integer facts; do not silently reinterpret floats.
fn formal_canonical(value: &Value) -> String {
    fn string(value: &str) -> String {
        serde_json::to_string(value)
            .unwrap()
            .encode_utf16()
            .map(|unit| {
                if unit >= 0x7f {
                    format!("\\u{unit:04x}")
                } else {
                    char::from_u32(u32::from(unit)).unwrap().to_string()
                }
            })
            .collect()
    }
    match value {
        Value::Null => "null".to_owned(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => {
            let signed = value
                .as_i64()
                .is_some_and(|n| n.unsigned_abs() <= 9_007_199_254_740_991);
            let unsigned = value.as_u64().is_some_and(|n| n <= 9_007_199_254_740_991);
            assert!(signed || unsigned, "portable receipt integer required");
            value.to_string()
        }
        Value::String(value) => string(value),
        Value::Array(values) => format!(
            "[{}]",
            values
                .iter()
                .map(formal_canonical)
                .collect::<Vec<_>>()
                .join(",")
        ),
        Value::Object(values) => {
            let mut entries: Vec<_> = values.iter().collect();
            entries.sort_by(|(a, _), (b, _)| a.encode_utf16().cmp(b.encode_utf16()));
            format!(
                "{{{}}}",
                entries
                    .into_iter()
                    .map(|(key, value)| format!("{}:{}", string(key), formal_canonical(value)))
                    .collect::<Vec<_>>()
                    .join(",")
            )
        }
    }
}

fn formal_fingerprint(value: &Value) -> String {
    polaris_updater::sha256_hex(formal_canonical(value).as_bytes())
}

fn formal_public_snapshot(snapshot: &Value, contract: &Value) {
    for (key, field) in [
        ("repositoryId", "repositoryId"),
        ("releaseId", "releaseId"),
        ("tag", "tagObject"),
        ("sourceCommit", "sourceCommit"),
        ("sourceTree", "sourceTree"),
    ] {
        assert_eq!(
            snapshot[key], contract[field],
            "compiled public identity: {key}"
        );
    }
    assert!(
        snapshot.get("producerReadback").is_none(),
        "no fresh producer-success claim"
    );
    let links = snapshot["sourceGitlinks"].as_array().unwrap();
    assert!(links.len() <= 1024);
    let mut paths = std::collections::BTreeSet::new();
    for link in links {
        let path = link["path"].as_str().unwrap();
        assert!(path
            .split('/')
            .all(|part| !part.is_empty() && ![".", ".."].contains(&part) && !part.contains('\0')));
        assert!(paths.insert(path) && hex(link["sha"].as_str().unwrap(), 40));
    }
    let mut pins = Vec::new();
    if contract["materialTransport"] != "committed-index-github-source-v1" {
        pins.extend([&contract["assetIndex"], &contract["sourceArchive"]]);
    }
    pins.extend(
        contract["platforms"]
            .as_object()
            .unwrap()
            .values()
            .map(|row| &row["releaseAsset"]),
    );
    let assets = snapshot["assets"].as_array().unwrap();
    assert_eq!(assets.len(), pins.len());
    for pin in pins {
        let rows: Vec<_> = assets
            .iter()
            .filter(|row| row["name"] == pin["name"])
            .collect();
        assert_eq!(rows.len(), 1, "unique public asset required");
        let row = rows[0];
        for (key, field) in [
            ("name", "name"),
            ("id", "assetId"),
            ("bytes", "bytes"),
            ("url", "url"),
        ] {
            assert_eq!(row[key], pin[field], "compiled public asset: {key}");
        }
        assert!(row.get("digest").is_some());
        assert!(
            row["digest"].is_null()
                || row["digest"] == format!("sha256:{}", pin["sha256"].as_str().unwrap())
        );
    }
}

#[test]
fn formal_portable_receipt_encoding_matches_consumer_ascii_json() {
    let fixture = json!({"z": [true, null, 17], "a": "\u{7f}中😀\n"});
    assert_eq!(
        formal_canonical(&fixture),
        r#"{"a":"\u007f\u4e2d\ud83d\ude00\n","z":[true,null,17]}"#
    );
    assert!(std::panic::catch_unwind(|| formal_canonical(&json!(1.5))).is_err());
}

#[test]
fn formal_public_snapshot_rejects_self_consistent_foreign_identity() {
    let contract = json!({"repositoryId": 1, "releaseId": 2, "tagObject": "fixture-tag",
        "sourceCommit": "fixture-source", "sourceTree": "fixture-tree",
        "materialTransport": "committed-index-github-source-v1", "platforms": {"linux": {
            "releaseAsset": {"name": "fixture-asset", "assetId": 3, "bytes": 4, "url": "https://fixture.invalid/asset", "sha256": "fixture-hash"}}}});
    let snapshot = json!({"repositoryId": 1, "releaseId": 2, "tag": "fixture-tag", "sourceCommit": "fixture-source", "sourceTree": "fixture-tree",
        "sourceGitlinks": [], "assets": [{"name": "fixture-asset", "id": 3, "bytes": 4, "url": "https://fixture.invalid/asset", "digest": null}]});
    formal_public_snapshot(&snapshot, &contract);
    for key in [
        "repositoryId",
        "releaseId",
        "tag",
        "sourceCommit",
        "sourceTree",
        "assets",
    ] {
        let mut foreign = snapshot.clone();
        foreign[key] = "foreign-fixture".into();
        assert!(
            std::panic::catch_unwind(|| formal_public_snapshot(&foreign, &contract)).is_err(),
            "{key}"
        );
    }
}

fn formal_input(receipt: &Value, receipt_path: &Path) -> PathBuf {
    assert_eq!(
        receipt["schema"],
        "polaris-runtime-loopback-formal-input-v1"
    );
    assert_eq!(receipt["inputMode"], "formal-release");
    assert!(hex(receipt["appTree"].as_str().unwrap(), 40));
    assert!(receipt["implementationManifestDigest"].is_null());
    let binding_metadata = std::fs::symlink_metadata(receipt_path).unwrap();
    assert_eq!(binding_metadata.mode() & 0o7777, 0o600);
    assert_eq!(binding_metadata.nlink(), 1);
    assert_eq!(
        binding_metadata.uid(),
        std::fs::metadata("/proc/self").unwrap().uid()
    );
    assert_eq!(receipt["profile"], "linux-amd64-formal-release");
    assert!(std::env::var_os("POLARIS_SINGBOX_PATH").is_none());
    assert_eq!(option_env!("GITHUB_SHA"), receipt["appCommit"].as_str());
    for (key, field) in [
        ("GITHUB_RUN_ID", "runId"),
        ("GITHUB_RUN_ATTEMPT", "runAttempt"),
    ] {
        let actual = std::env::var(key).unwrap();
        assert!(actual.parse::<u64>().unwrap() > 0);
        assert_eq!(receipt[field].as_str(), Some(actual.as_str()));
    }
    let slot = PathBuf::from(std::env::var_os("RUNNER_TEMP").unwrap())
        .canonicalize()
        .unwrap()
        .join(format!(
            "polaris-runtime-loopback-{}-{}",
            receipt["runId"].as_str().unwrap(),
            receipt["runAttempt"].as_str().unwrap()
        ))
        .join("evidence");
    assert_eq!(receipt_path, slot.join("formal-input.json"));
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .canonicalize()
        .unwrap();
    let manifest_raw = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/core-manifest.json"));
    let contract_raw = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../scripts/desktop-core/release-contract.json"
    ));
    let manifest: Value = serde_json::from_slice(manifest_raw).unwrap();
    let contract: Value = serde_json::from_slice(contract_raw).unwrap();
    assert!(manifest.get("desktopTemporary").is_none());
    assert_eq!(
        manifest["desktopRelease"]["contractPath"],
        "scripts/desktop-core/release-contract.json"
    );
    assert_eq!(
        manifest["desktopRelease"]["contractSha256"],
        polaris_updater::sha256_hex(contract_raw)
    );
    assert_eq!(
        contract["schema"],
        "polaris-upstream-native-release-consumer-v1"
    );
    assert_eq!(contract["repository"], "polaris-arch/sing-box-polaris");
    assert_eq!(receipt["kernelCommit"], contract["sourceCommit"]);
    assert_eq!(receipt["kernelTree"], contract["sourceTree"]);
    for field in [
        "repository",
        "version",
        "tag",
        "tagObject",
        "sourceCommit",
        "sourceTree",
        "releaseId",
    ] {
        assert_eq!(receipt["release"][field], contract[field]);
        if field != "repository" {
            assert_eq!(manifest["desktopRelease"][field], contract[field]);
        }
    }
    let inputs = &receipt["inputs"];
    let index_name = contract["assetIndex"]["name"].as_str().unwrap();
    assert!(
        !index_name.is_empty()
            && !index_name.contains(['/', '\\'])
            && index_name != "."
            && index_name != ".."
    );
    let originals = [
        (
            "manifest",
            root.join("src-tauri/core-manifest.json"),
            "manifest.json",
        ),
        (
            "contract",
            root.join("scripts/desktop-core/release-contract.json"),
            "contract.json",
        ),
        (
            "assetIndex",
            root.join("resources/.release-receipts").join(index_name),
            "asset-index.json",
        ),
        (
            "consumption",
            root.join("resources/.release-receipts/linux.json"),
            "consumption.json",
        ),
        (
            "readback",
            root.join("resources/.release-receipts/public-readback.json"),
            "public-readback.json",
        ),
        (
            "installation",
            root.join("resources/linux/core-manifest.json"),
            "installation.json",
        ),
    ];
    let mut original = std::collections::BTreeMap::new();
    for (key, path, retained) in originals {
        let bytes = formal_pin(&inputs[key], &path, 8 * 1024 * 1024);
        let retained_bytes = formal_pin(
            &receipt["retainedInputs"][key],
            &slot.join("formal-inputs").join(retained),
            8 * 1024 * 1024,
        );
        assert_eq!(
            bytes, retained_bytes,
            "retained original bytes differ: {key}"
        );
        original.insert(key, bytes);
    }
    assert_eq!(original["manifest"].as_slice(), manifest_raw.as_slice());
    assert_eq!(original["contract"].as_slice(), contract_raw.as_slice());
    assert_eq!(
        inputs["assetIndex"]["sha256"],
        contract["assetIndex"]["sha256"]
    );
    assert_eq!(
        inputs["assetIndex"]["bytes"],
        contract["assetIndex"]["bytes"]
    );
    let consumption: Value = serde_json::from_slice(&original["consumption"]).unwrap();
    let readback: Value = serde_json::from_slice(&original["readback"]).unwrap();
    let installation: Value = serde_json::from_slice(&original["installation"]).unwrap();
    assert_eq!(consumption["schema"], "polaris-app-release-consumption-v1");
    assert_eq!(consumption["platform"], "linux");
    assert_eq!(
        consumption["signaturePolicy"],
        "preserve measured original bytes; ad-hoc/not-signed observations do not assert trust"
    );
    assert_eq!(consumption.get("macCodeSignature"), Some(&Value::Null));
    assert_eq!(
        consumption["actualAppAcceptance"],
        "pending native load, gvisor/mixed, feature and final carrier gates"
    );
    let mut facts = consumption.clone();
    facts.as_object_mut().unwrap().remove("schema");
    facts.as_object_mut().unwrap().remove("fingerprint");
    assert_eq!(consumption["fingerprint"], formal_fingerprint(&facts));
    assert_eq!(
        consumption["adapter"]["schema"],
        "polaris-app-release-adapter-v1"
    );
    assert_eq!(
        consumption["adapter"]["fingerprint"],
        formal_fingerprint(&consumption["adapter"]["facts"])
    );

    assert_eq!(
        consumption["fingerprint"],
        receipt["consumptionFingerprint"]
    );
    assert_eq!(
        consumption["adapter"]["facts"]["candidate"],
        receipt["appCommit"]
    );
    assert_eq!(consumption["contractSha256"], inputs["contract"]["sha256"]);
    assert_eq!(
        consumption["assetIndexSha256"],
        inputs["assetIndex"]["sha256"]
    );
    let row = &contract["platforms"]["linux"];
    assert_eq!(consumption["producer"], *row);
    assert_eq!(consumption["releaseAsset"], row["releaseAsset"]);
    assert_eq!(readback["schema"], "polaris-app-release-readback-v1");
    assert_eq!(readback["contractSha256"], inputs["contract"]["sha256"]);
    assert_eq!(readback["before"], readback["after"]);
    assert_eq!(readback["before"], consumption["publicReadback"]);
    formal_public_snapshot(&readback["before"], &contract);
    for field in ["goVersion", "buildID", "tags"] {
        assert_eq!(receipt["build"][field], row[field]);
    }
    assert_eq!(
        receipt["naiveIncluded"].as_bool(),
        Some(
            row["tags"]
                .as_str()
                .unwrap()
                .split(',')
                .any(|tag| tag == "with_naive_outbound")
        )
    );
    formal_pin(
        &receipt["observedBuildInfo"],
        &slot.join("formal-build-info.txt"),
        8 * 1024 * 1024,
    );
    let selected = crate::runtime::core_identity::selected_manifest("linux").unwrap();
    let (source, contract_sha) =
        crate::runtime::core_identity::selected_check_source("linux").unwrap();
    assert_eq!(Some(source.as_str()), receipt["kernelCommit"].as_str());
    assert_eq!(
        Some(contract_sha.as_str()),
        inputs["contract"]["sha256"].as_str()
    );
    assert_eq!(installation["id"], receipt["installedCoreId"]);
    assert_eq!(
        installation["id"].as_str(),
        Some(
            crate::runtime::core_identity::selected_id("linux")
                .unwrap()
                .as_str()
        )
    );
    formal_pin(
        &receipt["helper"],
        &root.join("resources/linux/polaris-helper"),
        256 * 1024 * 1024,
    );
    assert_eq!(
        selected.helper_sha256.as_deref(),
        receipt["helper"]["sha256"].as_str()
    );
    assert_eq!(
        installation["core"]["helperSha256"],
        receipt["helper"]["sha256"]
    );
    for (key, member) in [("binary", "sing-box"), ("sidecar", "libcronet.so")] {
        formal_pin(
            &receipt[key],
            &root.join("resources/linux").join(member),
            256 * 1024 * 1024,
        );
        let expected = &row[if key == "binary" { "core" } else { "sidecar" }];
        assert_eq!(receipt[key]["sha256"], expected["sha256"]);
        assert_eq!(receipt[key]["bytes"], expected["bytes"]);
    }
    let binary = resolve_core_binary()
        .expect("compiled formal core and sidecar must resolve")
        .canonicalize()
        .unwrap();
    assert_eq!(binary, root.join("resources/linux/sing-box"));
    binary
}

struct Admission {
    receipt: Value,
    receipt_path: PathBuf,
    binary: PathBuf,
    binding_sha256: Option<String>,
    formal_bundle: Option<crate::runtime::core_identity::BundleObservation>,
}

impl Admission {
    fn read() -> Self {
        for (key, value) in [
            ("GITHUB_ACTIONS", "true"),
            ("GITHUB_REPOSITORY", "polaris-arch/Polaris"),
            ("GITHUB_REPOSITORY_ID", "1347028220"),
            ("GITHUB_EVENT_NAME", "workflow_dispatch"),
            ("GITHUB_JOB", "runtime_config_loopback"),
            ("GITHUB_REF", "refs/heads/hot/app-runtime-consumer-20261020"),
            ("GITHUB_WORKFLOW_REF", "polaris-arch/Polaris/.github/workflows/ci.yml@refs/heads/hot/app-runtime-consumer-20261020"),
            ("RUNNER_OS", "Linux"),
            ("RUNNER_ARCH", "X64"),
        ] {
            assert_eq!(std::env::var(key).as_deref(), Ok(value), "CI context: {key}");
        }
        assert_eq!(std::env::consts::OS, "linux");
        assert_eq!(std::env::consts::ARCH, "x86_64");
        assert_ne!(
            std::fs::metadata("/proc/self").unwrap().uid(),
            0,
            "unprivileged runner required"
        );
        let process_status = std::fs::read_to_string("/proc/self/status").unwrap();
        let capabilities = process_status
            .lines()
            .find_map(|line| line.strip_prefix("CapEff:"))
            .unwrap()
            .trim();
        assert_eq!(
            u64::from_str_radix(capabilities, 16).unwrap(),
            0,
            "no effective Linux capabilities"
        );
        let receipt_path = PathBuf::from(
            std::env::var_os("POLARIS_RUNTIME_LOOPBACK_RECEIPT")
                .expect("exact runner receipt required; no local path fallback"),
        );
        // Read safely before trusting either the filename or the schema. Valid
        // diagnostic receipts are already owned regular files with mode 0600.
        let raw_receipt = formal_bytes(&receipt_path, 1024 * 1024).0;
        let receipt: Value = serde_json::from_slice(&raw_receipt).expect("runner receipt JSON");
        let formal = receipt["schema"] == "polaris-runtime-loopback-formal-input-v1";
        if !formal {
            assert_eq!(receipt["schema"], "polaris-runtime-loopback-candidate-v1");
            assert_eq!(receipt["profile"], "linux-amd64-purego-no-naive");
            assert_eq!(receipt["naiveIncluded"], false);
        }
        assert_eq!(receipt["sourceVerified"], true);
        assert_eq!(receipt["publicationEligible"], false);
        for field in ["appCommit", "kernelCommit", "kernelTree"] {
            assert!(hex(receipt[field].as_str().unwrap(), 40), "exact {field}");
        }
        let app = receipt["appCommit"].as_str().unwrap();
        for key in ["GITHUB_SHA", "GITHUB_WORKFLOW_SHA"] {
            assert_eq!(std::env::var(key).as_deref(), Ok(app), "exact {key}");
        }
        assert_eq!(
            receipt["protoSha256"],
            polaris_updater::sha256_hex(include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../crates/singbox-grpc/proto/runtime_config.proto"
            )))
        );
        if !receipt["implementationManifestDigest"].is_null() {
            assert!(hex(
                receipt["implementationManifestDigest"].as_str().unwrap(),
                64
            ));
        }
        let binary = if formal {
            formal_input(&receipt, &receipt_path)
        } else {
            let binary = PathBuf::from(receipt["binary"]["path"].as_str().unwrap());
            assert_eq!(
                std::env::var_os("POLARIS_SINGBOX_PATH").map(PathBuf::from),
                Some(binary.clone()),
                "POLARIS_SINGBOX_PATH only attests this receipt; it is not a production override"
            );
            binary
        };
        let binding_sha256 = formal.then(|| polaris_updater::sha256_hex(&raw_receipt));
        let formal_bundle =
            formal.then(|| crate::runtime::core_identity::verify_bundled(&binary).unwrap());
        let result = Self {
            receipt,
            receipt_path,
            binary,
            binding_sha256,
            formal_bundle,
        };
        result.verify_binary();
        result
    }

    fn verify_binary(&self) {
        if let Some(expected) = &self.binding_sha256 {
            assert_eq!(
                *expected,
                polaris_updater::sha256_hex(&formal_bytes(&self.receipt_path, 1024 * 1024).0)
            );
            assert!(
                self.formal_bundle.as_ref().unwrap().is_current(),
                "formal payload object changed"
            );
            assert_eq!(formal_input(&self.receipt, &self.receipt_path), self.binary);
            assert!(
                self.formal_bundle.as_ref().unwrap().is_current(),
                "formal payload object changed during verification"
            );
        }
        let bytes = if self.binding_sha256.is_some() {
            formal_bytes(&self.binary, 256 * 1024 * 1024).0
        } else {
            regular(&self.binary, 256 * 1024 * 1024)
        };
        assert_eq!(
            self.receipt["binary"]["bytes"].as_u64(),
            Some(bytes.len() as u64)
        );
        assert_eq!(
            self.receipt["binary"]["sha256"],
            polaris_updater::sha256_hex(&bytes)
        );
    }

    fn capabilities(&self, value: &daemon::RuntimeCapabilities) {
        super::super::runtime_config::observe_runtime_pairing(value)
            .expect("real protocol/instance pairing");
        assert!(
            value.connected_hot_enabled,
            "this exact generated graph must actually be supported"
        );
        let protocol = value
            .fields
            .iter()
            .find(|f| f.effective_path == "runtime.protocol")
            .unwrap();
        assert_eq!(protocol.conditions, METHODS.map(str::to_owned));
        if let Some(expected) = self.receipt["implementationManifestDigest"].as_str() {
            assert_eq!(value.implementation_manifest_digest, expected);
        }
    }
}

struct Evidence {
    path: PathBuf,
    value: Value,
}

impl Evidence {
    fn new(admission: &Admission, name: &str) -> Self {
        let path = admission
            .receipt_path
            .parent()
            .unwrap()
            .join(format!("{name}.json"));
        assert!(!path.exists(), "refuse stale per-test evidence");
        let mut value = json!({
            "schema":"polaris-runtime-loopback-test-v1", "test":name,
            "passed":false, "profile":admission.receipt["profile"],
            "publicationEligible":false, "appCommit":admission.receipt["appCommit"],
            "kernelCommit":admission.receipt["kernelCommit"],
            "kernelTree":admission.receipt["kernelTree"],
            "coreSha256":admission.receipt["binary"]["sha256"], "events":[]
        });
        if let Some(digest) = &admission.binding_sha256 {
            value["schema"] = "polaris-runtime-loopback-formal-test-v1".into();
            value["inputSha256"] = digest.clone().into();
            value["contractSha256"] = admission.receipt["inputs"]["contract"]["sha256"].clone();
            value["consumptionFingerprint"] = admission.receipt["consumptionFingerprint"].clone();
        }
        let result = Self { path, value };
        result.flush();
        result
    }

    fn record(&mut self, event: Value) {
        self.value["events"].as_array_mut().unwrap().push(event);
        self.flush();
    }

    fn flush(&self) {
        std::fs::write(&self.path, serde_json::to_vec_pretty(&self.value).unwrap())
            .expect("write per-test actual evidence");
    }

    fn passed(&mut self) {
        self.value["passed"] = true.into();
        self.flush();
    }
}

fn identity(value: Option<&daemon::RuntimeIdentity>) -> Value {
    value.map_or(Value::Null, |i| {
        json!({
            "incarnation":i.incarnation, "revision":i.applied_revision,
            "effectiveDigest":i.effective_digest
        })
    })
}

fn capabilities(value: &daemon::RuntimeCapabilities) -> Value {
    json!({
        "contractVersion":value.contract_version, "enabled":value.connected_hot_enabled,
        "implementationManifestDigest":value.implementation_manifest_digest,
        "active":identity(value.active.as_ref()), "controlRevision":value.control_revision,
        "fullEffectiveSchemaSupported":value.full_effective_schema_supported,
        "receiptRetentionSeconds":value.receipt_retention_seconds,"maxReceipts":value.max_receipts,
        "fields":value.fields.iter().map(|f| json!({
            "path":f.effective_path,"variant":f.variant,"platform":f.platform_build,
            "class":f.update_class,"adapter":f.adapter_version,"conditions":f.conditions
        })).collect::<Vec<_>>()
    })
}

fn operation(value: &daemon::RuntimeConfigOperation) -> Value {
    json!({
        "transaction":value.transaction_id,"state":value.state,"durable":value.durable,
        "before":identity(value.before.as_ref()),"active":identity(value.active.as_ref()),
        "candidateDigest":value.candidate_digest,"impactDigest":value.impact_digest,
        "controlRevision":value.control_revision,"persistenceMode":value.persistence_mode,
        "ownerReloadAcknowledged":value.owner_reload_acknowledged,"retirementState":value.retirement_state,
        "desiredStateDigest":value.desired_state_digest,"desiredStateRevision":value.desired_state_revision,
        "errors":value.errors.iter().map(|e| json!({"code":e.code,"path":e.effective_path})).collect::<Vec<_>>()
    })
}

// Owned threads only. Reads time out, and Drop joins all accepted sockets before
// releasing listeners. No backend resolves or dials the destination requested.
struct Worker {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let result = thread.join();
            if !std::thread::panicking() {
                assert!(result.is_ok(), "owned fixture thread failed");
            }
        }
    }
}

#[derive(Clone)]
struct Target {
    connection: usize,
    host: String,
    port: u16,
}

struct Backend {
    address: SocketAddr,
    targets: Arc<Mutex<Vec<Target>>>,
    _worker: Worker,
}

fn socks_target(stream: &mut TcpStream) -> std::io::Result<(String, u16)> {
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(IO_BOUND))?;
    stream.set_write_timeout(Some(IO_BOUND))?;
    let mut greeting = [0; 2];
    stream.read_exact(&mut greeting)?;
    if greeting[0] != 5 || greeting[1] == 0 {
        return Err(std::io::Error::other("invalid SOCKS greeting"));
    }
    let mut methods = vec![0; usize::from(greeting[1])];
    stream.read_exact(&mut methods)?;
    if !methods.contains(&0) {
        return Err(std::io::Error::other("fixture only accepts no-auth SOCKS"));
    }
    stream.write_all(&[5, 0])?;
    let mut request = [0; 4];
    stream.read_exact(&mut request)?;
    if request[..3] != [5, 1, 0] {
        return Err(std::io::Error::other("fixture only accepts CONNECT"));
    }
    let host = match request[3] {
        1 => {
            let mut ip = [0; 4];
            stream.read_exact(&mut ip)?;
            Ipv4Addr::from(ip).to_string()
        }
        3 => {
            let mut length = [0];
            stream.read_exact(&mut length)?;
            let mut name = vec![0; usize::from(length[0])];
            stream.read_exact(&mut name)?;
            String::from_utf8(name).map_err(std::io::Error::other)?
        }
        4 => {
            let mut ip = [0; 16];
            stream.read_exact(&mut ip)?;
            std::net::Ipv6Addr::from(ip).to_string()
        }
        _ => return Err(std::io::Error::other("invalid SOCKS address")),
    };
    let mut port = [0; 2];
    stream.read_exact(&mut port)?;
    Ok((host, u16::from_be_bytes(port)))
}

impl Backend {
    fn new(label: u8) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let targets = Arc::new(Mutex::new(Vec::new()));
        let observations = Arc::clone(&targets);
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = Arc::clone(&stop);
        let thread = std::thread::spawn(move || {
            let mut sockets = Vec::new();
            while !stopping.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((mut stream, peer)) => {
                        assert!(peer.ip().is_loopback());
                        assert!(sockets.len() < 16, "bounded owned connection count");
                        let stop = Arc::clone(&stopping);
                        let targets = Arc::clone(&observations);
                        sockets.push(std::thread::spawn(move || {
                            let (host, port) =
                                socks_target(&mut stream).expect("owned SOCKS handshake");
                            {
                                let mut targets = targets.lock().unwrap();
                                let connection = targets.len() + 1;
                                targets.push(Target {
                                    connection,
                                    host,
                                    port,
                                });
                            }
                            stream.write_all(&[5, 0, 0, 1, 127, 0, 0, 1, 0, 0]).unwrap();
                            stream
                                .set_read_timeout(Some(Duration::from_millis(100)))
                                .unwrap();
                            let mut input = [0; 256];
                            while !stop.load(Ordering::Acquire) {
                                match stream.read(&mut input) {
                                    Ok(0) => break,
                                    Ok(count) => {
                                        let reply: Vec<_> = input[..count]
                                            .iter()
                                            .flat_map(|b| [label, *b])
                                            .collect();
                                        if stream.write_all(&reply).is_err() {
                                            break;
                                        }
                                    }
                                    Err(e)
                                        if matches!(
                                            e.kind(),
                                            std::io::ErrorKind::WouldBlock
                                                | std::io::ErrorKind::TimedOut
                                        ) => {}
                                    Err(_) => break,
                                }
                            }
                        }));
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(10))
                    }
                    Err(e) => panic!("owned listener: {e}"),
                }
            }
            for socket in sockets {
                socket.join().unwrap();
            }
        });
        Self {
            address,
            targets,
            _worker: Worker {
                stop,
                thread: Some(thread),
            },
        }
    }

    fn observations(&self) -> Value {
        json!(self
            .targets
            .lock()
            .unwrap()
            .iter()
            .map(|t| json!({
                "connection":t.connection,"host":t.host,"port":t.port
            }))
            .collect::<Vec<_>>())
    }
}

struct Resolver {
    address: SocketAddr,
    queries: Arc<Mutex<Vec<(String, u16)>>>,
    _worker: Worker,
}

fn question(packet: &[u8]) -> Option<(String, u16, usize)> {
    if packet.len() < 12 || packet[4..6] != [0, 1] {
        return None;
    }
    let mut offset = 12;
    let mut labels = Vec::new();
    loop {
        let length = usize::from(*packet.get(offset)?);
        offset += 1;
        if length == 0 {
            break;
        }
        if length > 63 {
            return None;
        }
        labels.push(std::str::from_utf8(packet.get(offset..offset + length)?).ok()?);
        offset += length;
    }
    let tail = packet.get(offset..offset + 4)?;
    if tail[2..] != [0, 1] {
        return None;
    }
    Some((
        labels.join("."),
        u16::from_be_bytes([tail[0], tail[1]]),
        offset + 4,
    ))
}

impl Resolver {
    fn new(answer: Ipv4Addr) -> Self {
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        socket
            .set_read_timeout(Some(Duration::from_millis(100)))
            .unwrap();
        let address = socket.local_addr().unwrap();
        let queries = Arc::new(Mutex::new(Vec::new()));
        let observations = Arc::clone(&queries);
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = Arc::clone(&stop);
        let thread = std::thread::spawn(move || {
            let mut packet = [0; 2048];
            while !stopping.load(Ordering::Acquire) {
                let (length, peer) = match socket.recv_from(&mut packet) {
                    Ok(value) => value,
                    Err(e)
                        if matches!(
                            e.kind(),
                            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                        ) =>
                    {
                        continue
                    }
                    Err(e) => panic!("owned DNS fixture: {e}"),
                };
                assert!(peer.ip().is_loopback());
                let Some((name, kind, end)) = question(&packet[..length]) else {
                    continue;
                };
                observations.lock().unwrap().push((name.clone(), kind));
                let answers = u16::from(name == DOMAIN && kind == 1);
                let mut response = Vec::new();
                response.extend_from_slice(&packet[..2]);
                response.extend_from_slice(&0x8180u16.to_be_bytes());
                response.extend_from_slice(&1u16.to_be_bytes());
                response.extend_from_slice(&answers.to_be_bytes());
                response.extend_from_slice(&[0; 4]);
                response.extend_from_slice(&packet[12..end]);
                if answers == 1 {
                    response.extend_from_slice(&[0xc0, 0x0c, 0, 1, 0, 1]);
                    response.extend_from_slice(&0u32.to_be_bytes()); // TTL 0: no cache evidence.
                    response.extend_from_slice(&4u16.to_be_bytes());
                    response.extend_from_slice(&answer.octets());
                }
                socket.send_to(&response, peer).unwrap();
            }
        });
        Self {
            address,
            queries,
            _worker: Worker {
                stop,
                thread: Some(thread),
            },
        }
    }

    fn count(&self) -> usize {
        self.queries
            .lock()
            .unwrap()
            .iter()
            .filter(|(name, kind)| name == DOMAIN && *kind == 1)
            .count()
    }
}

struct Network {
    a: Backend,
    b: Backend,
    dns_a: Resolver,
    dns_b: Resolver,
}

impl Network {
    fn new() -> Self {
        Self {
            a: Backend::new(b'A'),
            b: Backend::new(b'B'),
            dns_a: Resolver::new(Ipv4Addr::new(127, 0, 0, 2)),
            dns_b: Resolver::new(Ipv4Addr::new(127, 0, 0, 3)),
        }
    }

    fn assert_only_fixture_queries(&self) {
        for resolver in [&self.dns_a, &self.dns_b] {
            assert!(
                resolver
                    .queries
                    .lock()
                    .unwrap()
                    .iter()
                    .all(|(name, kind)| name == DOMAIN && matches!(*kind, 1 | 28)),
                "unexpected owned-resolver query"
            );
        }
    }

    fn config(&self) -> Value {
        let mixed = free_port();
        let control = loop {
            let port = free_port();
            if port != mixed {
                break port;
            }
        };
        fixture_config(
            [self.a.address, self.b.address],
            [self.dns_a.address, self.dns_b.address],
            mixed,
            control,
        )
    }
}

// Also used by ordinary admission tests with fixed, unopened loopback ports.
fn fixture_config(
    nodes: [SocketAddr; 2],
    dns: [SocketAddr; 2],
    mixed_port: u16,
    control_port: u16,
) -> Value {
    let mut config = polaris_store::default_config();
    config["proxyModeType"] = "manual".into();
    config["proxyMode"] = "smart".into();
    config["mixedPort"] = mixed_port.into();
    config["controlPort"] = control_port.into();
    config["clashApiSecret"] = FIXTURE_MANAGEMENT_SECRET.into();
    config["selectedServerId"] = "node-a".into();
    for key in [
        "singboxDashboard",
        "allowLan",
        "bypassLAN",
        "appRoutingEnabled",
        "autoSelectEnabled",
        "autoSwitchNode",
        "autoCheckUpdate",
        "autoUpdateSubscriptionOnStart",
        "ruleResourceAutoUpdate",
        "interruptConnectionsOnSwitch",
        "enableIPv6",
        "blockQuic",
    ] {
        config[key] = false.into();
    }
    for key in [
        "ruleResources",
        "customRuleSets",
        "appRules",
        "networkProfiles",
        "subscriptions",
        "autoSelectPins",
        "dnsServerGroups",
    ] {
        config[key] = json!([]);
    }
    config["servers"] = json!([
        {"id":"node-a","name":"Loop A","protocol":"socks","address":"127.0.0.1","port":nodes[0].port()},
        {"id":"node-b","name":"Loop B","protocol":"socks","address":"127.0.0.1","port":nodes[1].port()}
    ]);
    config["trafficRules"] = json!([{
        "id":"loop-route","type":"domain","values":[DOMAIN],"action":"proxy",
        "enabled":true,"targetServerId":"node-a",
        "effects":{"route":{"enabled":true,"action":"proxy","targetServerId":"node-a"}}
    }]);
    // Match the production route writers: legacy fields are exact traffic-only
    // projections, not independent empty dependencies (commands/rules/crud.rs).
    for key in ["policyRules", "customRules"] {
        config[key] = config["trafficRules"].clone();
    }
    config["dnsRules"] = json!([{
        "id":"loop-dns","type":"domain","values":[DOMAIN],"action":"proxy","enabled":true,
        "effects":{"dns":{"enabled":true,"action":{"type":"server","serverId":"loop-a"}}}
    }]);
    config["routeRuleOrder"] = json!(["loop-route"]);
    config["dnsRuleOrder"] = json!(["loop-dns"]);
    config["dnsServers"] = json!([
        dns_resource("builtin-domestic", dns[0].port()),
        dns_resource("builtin-remote", dns[0].port()),
        dns_resource("builtin-bootstrap", dns[0].port()),
        dns_resource("loop-a", dns[0].port()),
        dns_resource("loop-b", dns[1].port())
    ]);
    config["dnsDefaults"] = json!({
        "directServerId":"builtin-domestic", "proxyServerId":"builtin-remote",
        "unmatchedAction":{"type":"reject","method":"drop"},
        "realQueryFallbackAction":{"type":"reject","method":"drop"},
        "connectionResolution":"dnsRules"
    });
    config["dnsConfig"] = json!({
        "domesticDns":format!("udp://{}",dns[0]),
        "foreignDns":format!("udp://{}",dns[0]),
        "enableFakeIp":false,"fakeIpToggleMigrated":true,"fakeIpTunAutoEnable":false,
        "optimisticCache":false,"takeoverSystemDns":false,"nodeResolverMigrated":true,
        "resolveNodeDomainsAhead":false,"nodeResolverSingle":"loop-node",
        "nodeResolverPool":["loop-node"],
        "nodeResolverCustom":[{"id":"loop-node","spec":format!("udp://{}",dns[0])}]
    });
    config
}

fn dns_resource(id: &str, port: u16) -> Value {
    json!({"id":id,"name":id,"enabled":true,"type":"udp",
        "endpoint":{"host":"127.0.0.1","port":port},"outbound":{"type":"direct"}})
}

fn changed_graph(mut config: Value) -> Value {
    set_fixture_route_target(&mut config, "node-b");
    config["dnsRules"][0]["effects"]["dns"]["action"]["serverId"] = "loop-b".into();
    config
}

// Mutate each projection in place: never erase extra rules or hidden fields to
// make admission pass. save_full sanitizes, but does not run load-time migration.
fn set_fixture_route_target(config: &mut Value, target: &str) {
    for key in ["trafficRules", "policyRules", "customRules"] {
        config[key][0]["targetServerId"] = target.into();
        config[key][0]["effects"]["route"]["targetServerId"] = target.into();
    }
}

// This admission is deliberately specific to this two-node fixture. It is not a
// general network sandbox: unused production DNS definitions remain unchanged.
fn canonical_fixture(config: &Value, requested: &Value) {
    for key in [
        "proxyModeType",
        "proxyMode",
        "mixedPort",
        "controlPort",
        "clashApiSecret",
        "logLevel",
        "servers",
        "selectedServerId",
        "dnsServers",
        "dnsConfig",
        "dnsDefaults",
        "trafficRules",
        "policyRules",
        "customRules",
        "dnsRules",
        "routeRuleOrder",
        "dnsRuleOrder",
    ] {
        assert_eq!(
            config[key], requested[key],
            "canonical fixture changed {key}"
        );
    }
    assert_eq!(config["proxyModeType"], "manual");
    assert_eq!(config["proxyMode"], "smart");
    for key in [
        "singboxDashboard",
        "allowLan",
        "bypassLAN",
        "appRoutingEnabled",
        "autoSelectEnabled",
        "autoSwitchNode",
        "autoCheckUpdate",
        "autoUpdateSubscriptionOnStart",
        "ruleResourceAutoUpdate",
        "interruptConnectionsOnSwitch",
        "enableIPv6",
        "blockQuic",
    ] {
        assert_eq!(config[key], false, "unsafe fixture switch {key}");
    }
    for key in [
        "subscriptions",
        "autoSelectPins",
        "ruleResources",
        "customRuleSets",
        "appRules",
        "networkProfiles",
        "dnsServerGroups",
    ] {
        assert_eq!(
            config[key],
            json!([]),
            "unexpected fixture dependency {key}"
        );
    }
    let servers = config["servers"].as_array().unwrap();
    assert_eq!(servers.len(), 2);
    for node in servers {
        assert_eq!(
            node.as_object().unwrap().len(),
            5,
            "no hidden node dial/TLS/mesh options"
        );
        assert_eq!(node["protocol"], "socks");
        assert_eq!(node["address"], "127.0.0.1", "node endpoints need no DNS");
        assert!(node["port"]
            .as_u64()
            .is_some_and(|p| p > 1024 && p <= 65535));
    }
    assert_eq!(config["dnsConfig"]["enableFakeIp"], false);
    assert_eq!(config["dnsConfig"]["fakeIpToggleMigrated"], true);
    assert_eq!(config["dnsConfig"]["fakeIpTunAutoEnable"], false);
    assert_eq!(config["dnsConfig"]["nodeResolverMigrated"], true);
    assert_eq!(config["dnsConfig"]["optimisticCache"], false);
    assert_eq!(config["dnsConfig"]["takeoverSystemDns"], false);
    assert_eq!(config["dnsConfig"]["resolveNodeDomainsAhead"], false);
    assert_eq!(config["dnsDefaults"]["connectionResolution"], "dnsRules");
    let target = requested["trafficRules"][0]["targetServerId"]
        .as_str()
        .unwrap();
    let dns_target = match target {
        "node-a" => "loop-a",
        "node-b" => "loop-b",
        _ => panic!("only owned fixture route targets"),
    };
    let traffic = json!([{
        "id":"loop-route","type":"domain","values":[DOMAIN],"action":"proxy",
        "enabled":true,"targetServerId":target,
        "effects":{"route":{"enabled":true,"action":"proxy","targetServerId":target}}
    }]);
    for key in ["trafficRules", "policyRules", "customRules"] {
        assert_eq!(config[key], traffic, "exact owned traffic projection {key}");
    }
    assert_eq!(
        config["dnsRules"],
        json!([{
            "id":"loop-dns","type":"domain","values":[DOMAIN],"action":"proxy","enabled":true,
            "effects":{"dns":{"enabled":true,"action":{"type":"server","serverId":dns_target}}}
        }]),
        "exact paired owned DNS rule"
    );
    assert_eq!(config["routeRuleOrder"], json!(["loop-route"]));
    assert_eq!(config["dnsRuleOrder"], json!(["loop-dns"]));
}

fn save_fixture_config(manager: &ConfigManager, requested: &Value) -> Value {
    // Admit caller intent before the store can sanitize it, then check both the
    // saved cache and actual App source. Never normalize input to pass this gate.
    canonical_fixture(requested, requested);
    manager.save_full(requested).expect("save admitted fixture");
    saved_fixture_config(manager, requested)
}

fn saved_fixture_config(manager: &ConfigManager, requested: &Value) -> Value {
    let saved = manager.current().unwrap();
    canonical_fixture(&saved, requested);
    let persisted: Value = serde_json::from_slice(&std::fs::read(manager.path()).unwrap()).unwrap();
    assert_eq!(
        persisted, saved,
        "App desired cache must equal durable source"
    );
    canonical_fixture(&persisted, requested);
    saved
}

fn strings(value: &Value) -> Vec<&str> {
    if value.is_null() {
        return Vec::new();
    }
    if let Some(value) = value.as_str() {
        return vec![value];
    }
    value
        .as_array()
        .unwrap()
        .iter()
        .map(|value| value.as_str().unwrap())
        .collect()
}

// A conservative exclusion proof for this one DOMAIN from mixed-in. An unknown
// matcher never proves exclusion. In particular an unknown rule_set/logical
// branch cannot hide an external DNS fallback behind a nonmatching domain list.
fn excludes_fixture(rule: &Value) -> bool {
    if rule["invert"] == true {
        return false;
    }
    if !rule["inbound"].is_null() && !strings(&rule["inbound"]).contains(&"mixed-in") {
        return true;
    }
    if rule["resolver_policy"] == "node-bootstrap" {
        return true;
    }
    if !rule["rule_set"].is_null() || !rule["rules"].is_null() {
        return false;
    }
    let mut has_domain = false;
    let mut matches = false;
    for key in ["domain", "domain_suffix", "domain_keyword", "domain_regex"] {
        for value in strings(&rule[key]) {
            has_domain = true;
            matches |= match key {
                "domain" => DOMAIN == value,
                "domain_suffix" => {
                    let suffix = value.trim_start_matches('.');
                    DOMAIN == suffix || DOMAIN.ends_with(&format!(".{suffix}"))
                }
                "domain_keyword" => DOMAIN.contains(value),
                "domain_regex" => regex::Regex::new(value).unwrap().is_match(DOMAIN),
                _ => unreachable!(),
            };
        }
    }
    has_domain && !matches
}

// A probe selector deliberately interrupts only its own reusable measurement
// slot. Admit that policy only after proving the exact private slot graph;
// a tag prefix alone never exempts a user selector from the false preference.
fn contains_probe_reference(value: &Value, probes: &std::collections::BTreeSet<String>) -> bool {
    match value {
        Value::String(tag) => probes.contains(tag),
        Value::Array(values) => values
            .iter()
            .any(|value| contains_probe_reference(value, probes)),
        Value::Object(fields) => fields
            .values()
            .any(|value| contains_probe_reference(value, probes)),
        _ => false,
    }
}

fn inspect_selectors(generated: &Value) -> Value {
    use std::collections::BTreeSet;
    let outbounds = generated["outbounds"].as_array().unwrap();
    let tags: BTreeSet<_> = outbounds
        .iter()
        .map(|outbound| outbound["tag"].as_str().unwrap())
        .collect();
    assert_eq!(tags.len(), outbounds.len(), "unique outbound identities");
    let inbounds = generated["inbounds"].as_array().unwrap();
    let routes = generated["route"]["rules"].as_array().unwrap();
    let physical: BTreeSet<String> = outbounds
        .iter()
        .filter(|outbound| outbound["type"] == "socks")
        .map(|outbound| outbound["tag"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(physical.len(), 2, "exact owned physical member set");
    let mut probe_members = physical.clone();
    probe_members.insert("direct".into());
    let mut probes = BTreeSet::new();
    let mut slots = BTreeSet::new();
    let mut users = BTreeSet::new();
    let mut projection = Vec::new();
    for outbound in outbounds
        .iter()
        .filter(|outbound| outbound["type"] == "selector")
    {
        let tag = outbound["tag"].as_str().unwrap();
        let members = strings(&outbound["outbounds"]);
        let member_set: BTreeSet<String> = members.iter().map(|tag| tag.to_string()).collect();
        assert_eq!(
            members.len(),
            member_set.len(),
            "duplicate selector member: {tag}"
        );
        assert!(
            members.contains(&outbound["default"].as_str().unwrap()),
            "selector default must be a member: {tag}"
        );
        if matches!(tag, "proxy-selector" | "rule-sel-loop-route") {
            assert!(users.insert(tag), "duplicate user selector: {tag}");
            assert_eq!(
                outbound["interrupt_exist_connections"], false,
                "user preference: {tag}"
            );
            let mut expected = physical.clone();
            expected.insert(
                if tag == "proxy-selector" {
                    "direct"
                } else {
                    "proxy-selector"
                }
                .into(),
            );
            assert_eq!(member_set, expected, "exact user selector members: {tag}");
        } else {
            let suffix = tag
                .strip_prefix("probe-selector-")
                .expect("unexpected fixture selector");
            let slot = suffix.parse::<usize>().expect("numeric private probe slot");
            assert_eq!(suffix, slot.to_string(), "canonical private probe slot");
            assert!(
                slots.insert(slot) && probes.insert(tag.to_string()),
                "duplicate probe slot"
            );
            assert_eq!(
                outbound["interrupt_exist_connections"], true,
                "private probe slot policy: {tag}"
            );
            assert_eq!(outbound["default"], "direct");
            assert_eq!(
                member_set, probe_members,
                "exact owned probe members: {tag}"
            );
            let inbound_tag = format!("probe-in-{slot}");
            let paired: Vec<_> = inbounds
                .iter()
                .filter(|inbound| inbound["tag"] == inbound_tag)
                .collect();
            assert_eq!(paired.len(), 1, "unique paired probe inbound: {tag}");
            assert_eq!(paired[0]["type"], "http");
            assert_eq!(paired[0]["listen"], "127.0.0.1");
            assert!(paired[0]["listen_port"]
                .as_u64()
                .is_some_and(|port| port > 1024 && port <= 65535));
            let expected_route = json!({"inbound":[inbound_tag],"action":"route","outbound":tag});
            assert_eq!(
                routes
                    .iter()
                    .filter(|rule| **rule == expected_route)
                    .count(),
                1,
                "unique exact private probe route: {tag}"
            );
        }
        projection.push(outbound.clone());
    }
    assert_eq!(
        users,
        BTreeSet::from(["proxy-selector", "rule-sel-loop-route"])
    );
    assert_eq!(
        slots,
        (0..slots.len()).collect::<BTreeSet<_>>(),
        "contiguous private probe slots"
    );
    let expected_inbounds: BTreeSet<String> = slots
        .iter()
        .map(|slot| format!("probe-in-{slot}"))
        .collect();
    let actual_inbounds: Vec<_> = inbounds
        .iter()
        .filter_map(|inbound| {
            let tag = inbound["tag"].as_str().unwrap();
            tag.starts_with("probe-in-").then_some(tag.to_string())
        })
        .collect();
    assert_eq!(actual_inbounds.len(), expected_inbounds.len());
    assert_eq!(
        actual_inbounds.into_iter().collect::<BTreeSet<_>>(),
        expected_inbounds
    );
    // The only route references allowed are the exact paired private rules.
    // Recursion also rejects a hidden logical branch, detour, or default edge.
    for rule in routes {
        if contains_probe_reference(rule, &probes) {
            assert!(
                slots.iter().any(|slot| *rule
                    == json!({
                        "inbound":[format!("probe-in-{slot}")],"action":"route",
                        "outbound":format!("probe-selector-{slot}")
                    })),
                "probe reference outside its exact private route"
            );
        }
    }
    let mut route = generated["route"].clone();
    route.as_object_mut().unwrap().remove("rules");
    assert!(
        !contains_probe_reference(&route, &probes),
        "probe route final or indirect edge"
    );
    for outbound in outbounds {
        let mut fields = outbound.clone();
        fields.as_object_mut().unwrap().remove("tag");
        assert!(
            !contains_probe_reference(&fields, &probes),
            "outbound reaches private probe selector"
        );
    }
    let servers = generated["dns"]["servers"].as_array().unwrap();
    for slot in &slots {
        let tag = format!("dns-probe-exit-{slot}");
        let paired: Vec<_> = servers
            .iter()
            .filter(|server| server["tag"] == tag)
            .collect();
        assert_eq!(paired.len(), 1, "unique private probe DNS server");
        assert_eq!(paired[0]["detour"], format!("probe-selector-{slot}"));
    }
    for server in servers {
        let mut fields = server.clone();
        if let Some(detour) = fields["detour"].as_str() {
            if probes.contains(detour) {
                let slot = detour.strip_prefix("probe-selector-").unwrap();
                assert_eq!(fields["tag"], format!("dns-probe-exit-{slot}"));
                fields.as_object_mut().unwrap().remove("detour");
            }
        }
        assert!(
            !contains_probe_reference(&fields, &probes),
            "unexpected probe DNS reference"
        );
    }
    // Reachability of these unchanged DNS definitions from mixed-in remains
    // independently excluded by inspect_generated's conservative DNS proof.
    json!(projection)
}

// Ordinary store checks use only private files and unopened fixed ports. They
// exercise save (sanitize/validate) versus load (migration) without a ProxyRuntime.
fn store_admission_fixture() -> Value {
    let address = |port| SocketAddr::from(([127, 0, 0, 1], port));
    fixture_config(
        [address(31011), address(31012)],
        [address(31013), address(31014)],
        31015,
        31016,
    )
}

// Exercise the exact production hot-intent gate from an existing ordinary CI
// entrypoint. These cases neither start a core nor change the legacy builder.
fn fixture_hot_route_reference_admission() {
    let original = store_admission_fixture();
    let check = |label: &str, value: Value, expected: bool| {
        let typed = serde_json::from_value(value).unwrap();
        assert_eq!(
            super::super::runtime_config::runtime_route_targets_valid(&typed),
            expected,
            "hot route reference admission: {label}"
        );
    };
    for (target, expected) in [
        ("node-a", true),
        ("node-b", true),
        ("", true),
        ("nonexistent-node", false),
        (" ", false),
    ] {
        let mut value = original.clone();
        set_fixture_route_target(&mut value, target);
        check(target, value, expected);
    }
    let mut missing = original.clone();
    set_fixture_route_target(&mut missing, "nonexistent-node");
    for projection in ["trafficRules", "policyRules", "customRules"] {
        let mut value = missing.clone();
        for earlier in ["trafficRules", "policyRules"] {
            if earlier == projection {
                break;
            }
            value.as_object_mut().unwrap().remove(earlier);
        }
        check(projection, value, false);
    }
    let mut value = missing.clone();
    value["trafficRules"] = json!([]);
    check("empty authoritative traffic collection", value, true);
    let mut value = missing.clone();
    value["trafficRules"] = original["trafficRules"].clone();
    check("stale legacy mirrors", value, true);
    let mut value = original.clone();
    value["trafficRules"][0]["targetServerId"] = "nonexistent-node".into();
    check("authoritative effect overrides legacy target", value, true);
    let mut value = original.clone();
    value["trafficRules"][0]["effects"]["route"]["targetServerId"] = "nonexistent-node".into();
    check("invalid authoritative effect", value, false);
    for (path, replacement) in [
        ("/trafficRules/0/enabled", json!(false)),
        ("/trafficRules/0/effects/route/enabled", json!(false)),
        ("/trafficRules/0/effects/route/action", json!("direct")),
        ("/trafficRules/0/effects/route/action", json!("block")),
        ("/trafficRules/0/effects", json!({})),
        ("/proxyMode", json!("global")),
        ("/proxyMode", json!("direct")),
    ] {
        let mut value = missing.clone();
        *value.pointer_mut(path).unwrap() = replacement;
        check(path, value, true);
    }
    let mut value = missing.clone();
    value["trafficRules"][0]["effects"]["route"]["resolutionOnly"] = true.into();
    check("resolution-only route", value, true);
    let mut value = original.clone();
    value["trafficRules"][0]["effects"]["route"]
        .as_object_mut()
        .unwrap()
        .remove("targetServerId");
    check("absent authoritative target inherits", value, true);
    let mut value = missing;
    let mut added = value["servers"][0].clone();
    added["id"] = "nonexistent-node".into();
    value["servers"].as_array_mut().unwrap().push(added);
    check("new candidate node is a valid target", value, true);

    let mut app = original.clone();
    app["appRules"] = json!([{"appId":"fixture-app","action":"proxy","enabled":true,
        "targetServerId":"nonexistent-node"}]);
    check("app routing disabled", app.clone(), true);
    app["appRoutingEnabled"] = true.into();
    check("active app target missing", app.clone(), false);
    for (path, replacement) in [
        ("/appRules/0/targetServerId", json!("node-b")),
        ("/appRules/0/targetServerId", json!("")),
        ("/appRules/0/enabled", json!(false)),
        ("/appRules/0/action", json!("direct")),
        ("/appRules/0/action", json!("block")),
        ("/proxyMode", json!("global")),
        ("/proxyMode", json!("direct")),
    ] {
        let mut value = app.clone();
        *value.pointer_mut(path).unwrap() = replacement;
        check(path, value, true);
    }
    app["appRules"][0]
        .as_object_mut()
        .unwrap()
        .remove("targetServerId");
    check("absent app target inherits", app, true);
}

fn fixture_store_transition_admission() {
    let directory = TestDir::new("polaris-fixture-store-admission-");
    let manager = ConfigManager::new(directory.to_path_buf());
    let initial = store_admission_fixture();
    let saved = save_fixture_config(&manager, &initial);
    let loaded = manager.load_full().unwrap();
    canonical_fixture(&loaded, &saved);
    saved_fixture_config(&manager, &saved);
    assert_eq!(manager.load_full().unwrap(), loaded, "idempotent load");

    // Isolate the old mirror transition. save_full preserves the empty
    // mirrors; cold-start journal processing reloads and materializes mirror A.
    let mut old_initial = initial.clone();
    for key in ["policyRules", "customRules"] {
        old_initial[key] = json!([]);
    }
    manager.save_full(&old_initial).unwrap();
    let before_load = manager.current().unwrap();
    for key in ["policyRules", "customRules"] {
        assert_eq!(before_load[key], json!([]), "save is not migration");
    }
    let reloaded = manager.load_full().unwrap();
    canonical_fixture(&reloaded, &initial);
    saved_fixture_config(&manager, &initial);

    let graph = changed_graph(reloaded.clone());
    let mut log_only = reloaded.clone();
    log_only["logLevel"] = "warn".into();
    let mut structural = reloaded.clone();
    structural["mixedPort"] = 31017.into();
    for requested in [graph, log_only, structural] {
        let saved = save_fixture_config(&manager, &requested);
        // A second desired document must keep exactly the same owned mirrors;
        // loading/persisting must not resurrect A after the graph changed to B.
        assert_eq!(save_fixture_config(&manager, &saved), saved);
        canonical_fixture(&manager.load_full().unwrap(), &requested);
        saved_fixture_config(&manager, &requested);
    }
}

fn fixture_projection_mutation_admission() {
    let original = store_admission_fixture();
    for (pointer, value) in [
        ("/customRules", json!([])),
        ("/policyRules", json!([])),
        ("/customRules/0/values", json!(["external.test"])),
        ("/policyRules/0/targetServerId", json!("node-b")),
        (
            "/customRules/0/effects/route/targetServerId",
            json!("node-b"),
        ),
        ("/trafficRules/0/targetServerId", json!("node-b")),
        ("/dnsRules/0/effects/dns/action/serverId", json!("loop-b")),
        ("/routeRuleOrder", json!([])),
        ("/dnsRuleOrder", json!([])),
        ("/dnsConfig/fakeIpToggleMigrated", json!(false)),
        ("/dnsConfig/fakeIpTunAutoEnable", json!(true)),
        ("/dnsConfig/nodeResolverMigrated", Value::Null),
    ] {
        let mut mutated = original.clone();
        *mutated.pointer_mut(pointer).unwrap() = value;
        assert!(std::panic::catch_unwind(|| canonical_fixture(&mutated, &original)).is_err());
        assert!(
            std::panic::catch_unwind(|| canonical_fixture(&mutated, &mutated)).is_err(),
            "input must not attest its own unsafe projection: {pointer}"
        );
    }
    for key in ["trafficRules", "policyRules", "customRules"] {
        for hidden in [false, true] {
            let mut mutated = original.clone();
            if hidden {
                mutated[key][0]["effects"]["dns"] = json!({"enabled":true});
            } else {
                let duplicate = mutated[key][0].clone();
                mutated[key].as_array_mut().unwrap().push(duplicate);
            }
            // The fixture mutation changes targets in place, and cannot erase
            // unexpected input to manufacture an admitted desired document.
            let mutated = changed_graph(mutated);
            assert!(std::panic::catch_unwind(|| canonical_fixture(&mutated, &mutated)).is_err());
        }
    }
    for key in [
        "subscriptions",
        "autoSelectPins",
        "ruleResources",
        "customRuleSets",
        "appRules",
        "networkProfiles",
        "dnsServerGroups",
    ] {
        let mut mutated = original.clone();
        mutated[key] = json!([{"id":"unexpected"}]);
        assert!(
            std::panic::catch_unwind(|| canonical_fixture(&mutated, &mutated)).is_err(),
            "{key}"
        );
    }

    let directory = TestDir::new("polaris-fixture-stale-mirror-");
    let manager = ConfigManager::new(directory.to_path_buf());
    save_fixture_config(&manager, &original);
    let mut stale = original.clone();
    stale["trafficRules"][0]["targetServerId"] = "node-b".into();
    stale["trafficRules"][0]["effects"]["route"]["targetServerId"] = "node-b".into();
    stale["dnsRules"][0]["effects"]["dns"]["action"]["serverId"] = "loop-b".into();
    manager.save_full(&stale).unwrap();
    let saved = manager.current().unwrap();
    assert_eq!(saved["trafficRules"], stale["trafficRules"]);
    for key in ["policyRules", "customRules"] {
        assert_eq!(
            saved[key], original["trafficRules"],
            "save preserves stale mirrors"
        );
    }
    assert!(std::panic::catch_unwind(|| canonical_fixture(&saved, &stale)).is_err());

    // Whole-object dnsConfig replacement also used to drop the production
    // migration markers. In manual mode a later load would turn FakeIP back on.
    let mut unmigrated = original.clone();
    for key in [
        "fakeIpToggleMigrated",
        "fakeIpTunAutoEnable",
        "nodeResolverMigrated",
    ] {
        unmigrated["dnsConfig"].as_object_mut().unwrap().remove(key);
    }
    manager.save_full(&unmigrated).unwrap();
    assert_eq!(
        manager.current().unwrap()["dnsConfig"]["enableFakeIp"],
        false
    );
    let migrated = manager.load_full().unwrap();
    assert_eq!(migrated["dnsConfig"]["enableFakeIp"], true);
    assert!(std::panic::catch_unwind(|| canonical_fixture(&migrated, &migrated)).is_err());

    // Invalid target remains a real store/dispatcher case in the ignored test.
    // Here prove the fixture writer keeps it invalid in all three projections;
    // if this store accepts it, it must not silently coerce it to an owned node.
    let mut invalid = original.clone();
    set_fixture_route_target(&mut invalid, "nonexistent-node");
    assert!(std::panic::catch_unwind(|| canonical_fixture(&invalid, &invalid)).is_err());
    let before = manager.current().unwrap();
    let bytes = std::fs::read(manager.path()).unwrap();
    if manager.save_full(&invalid).is_ok() {
        let saved = manager.current().unwrap();
        for key in ["trafficRules", "policyRules", "customRules"] {
            assert_eq!(saved[key], invalid[key]);
        }
    } else {
        assert_eq!(manager.current().unwrap(), before);
        assert_eq!(std::fs::read(manager.path()).unwrap(), bytes);
    }
}

// Pure selector admission: no ports, runtime, process, or management RPC is used.
fn selector_admission_fixture() -> Value {
    json!({
        "inbounds":[{"type":"http","tag":"probe-in-0","listen":"127.0.0.1","listen_port":31001}],
        "outbounds":[
            {"type":"socks","tag":"Loop A"},
            {"type":"socks","tag":"Loop B"},
            {"type":"selector","tag":"proxy-selector","outbounds":["Loop A","Loop B","direct"],
                "default":"Loop A","interrupt_exist_connections":false},
            {"type":"selector","tag":"rule-sel-loop-route","outbounds":["Loop A","Loop B","proxy-selector"],
                "default":"Loop A","interrupt_exist_connections":false},
            {"type":"direct","tag":"direct"},
            {"type":"selector","tag":"probe-selector-0","outbounds":["Loop A","Loop B","direct"],
                "default":"direct","interrupt_exist_connections":true}
        ],
        "route":{"final":"proxy-selector","rules":[
            {"inbound":["probe-in-0"],"action":"route","outbound":"probe-selector-0"},
            {"domain":[DOMAIN],"action":"route","outbound":"rule-sel-loop-route"}
        ]},
        "dns":{"servers":[{"tag":"dns-probe-exit-0","detour":"probe-selector-0"}]}
    })
}

#[test]
fn private_probe_selector_admission_accepts_only_owned_slot_graph() {
    fixture_hot_route_reference_admission();
    fixture_store_transition_admission();
    let mut generated = selector_admission_fixture();
    assert_eq!(inspect_selectors(&generated).as_array().unwrap().len(), 3);
    let projection = json!({"route":generated["route"],"dns":generated["dns"],
        "selectors":inspect_selectors(&generated)});
    let mut canonical = generated.clone();
    for index in [2, 3] {
        canonical["outbounds"][index]
            .as_object_mut()
            .unwrap()
            .remove("interrupt_exist_connections");
    }
    assert!(std::panic::catch_unwind(|| inspect_selectors(&canonical)).is_err());
    for rule in canonical["route"]["rules"].as_array_mut().unwrap() {
        rule.as_object_mut().unwrap().remove("action");
    }
    canonical["route"]["rules"][0]["inbound"] = "probe-in-0".into();
    canonical["route"]["rules"][1]["domain"] = DOMAIN.into();
    let restored = restore_core_projection_forms(&canonical, &projection);
    assert_eq!(inspect_selectors(&restored), projection["selectors"]);
    generated["outbounds"][3]["default"] = "Loop B".into();
    assert_eq!(inspect_selectors(&generated).as_array().unwrap().len(), 3);
    // An empty production pool is also legitimate after port allocation failure.
    generated["inbounds"] = json!([]);
    generated["outbounds"].as_array_mut().unwrap().pop();
    generated["route"]["rules"]
        .as_array_mut()
        .unwrap()
        .remove(0);
    generated["dns"]["servers"] = json!([]);
    assert_eq!(inspect_selectors(&generated).as_array().unwrap().len(), 2);
}

#[test]
fn private_probe_selector_admission_rejects_policy_and_reachability_mutations() {
    fixture_projection_mutation_admission();
    let original = selector_admission_fixture();
    let projection = json!({"route":original["route"],"dns":original["dns"],
        "selectors":inspect_selectors(&original)});
    let replacements = [
        (
            "main preference",
            "/outbounds/2/interrupt_exist_connections",
            json!(true),
        ),
        (
            "rule preference",
            "/outbounds/3/interrupt_exist_connections",
            json!(true),
        ),
        (
            "missing explicit preference",
            "/outbounds/2/interrupt_exist_connections",
            Value::Null,
        ),
        (
            "unknown user selector",
            "/outbounds/3/tag",
            json!("unexpected-selector"),
        ),
        (
            "prefix impersonation",
            "/outbounds/5/tag",
            json!("probe-selector-user"),
        ),
        (
            "noncanonical index",
            "/outbounds/5/tag",
            json!("probe-selector-00"),
        ),
        (
            "unpaired index",
            "/outbounds/5/tag",
            json!("probe-selector-1"),
        ),
        ("unpaired inbound", "/inbounds/0/tag", json!("probe-in-1")),
        ("public inbound", "/inbounds/0/listen", json!("0.0.0.0")),
        ("wrong inbound type", "/inbounds/0/type", json!("mixed")),
        (
            "probe policy",
            "/outbounds/5/interrupt_exist_connections",
            json!(false),
        ),
        ("probe default", "/outbounds/5/default", json!("Loop A")),
        (
            "unowned member",
            "/outbounds/5/outbounds/0",
            json!("external"),
        ),
        (
            "user reaches probe",
            "/outbounds/2/outbounds/0",
            json!("probe-selector-0"),
        ),
        ("route final", "/route/final", json!("probe-selector-0")),
        (
            "mixed ingress",
            "/route/rules/0/inbound",
            json!(["probe-in-0", "mixed-in"]),
        ),
        (
            "nonexclusive route",
            "/route/rules/0",
            json!({"action":"route","outbound":"probe-selector-0"}),
        ),
        ("DNS owner", "/dns/servers/0/tag", json!("dns-user")),
        (
            "DNS unpaired detour",
            "/dns/servers/0/detour",
            json!("proxy-selector"),
        ),
        (
            "physical tag impersonation",
            "/outbounds/0/tag",
            json!("probe-selector-0"),
        ),
    ];
    for (label, pointer, value) in replacements {
        let mut mutated = original.clone();
        *mutated.pointer_mut(pointer).unwrap() = value;
        assert!(
            std::panic::catch_unwind(|| inspect_selectors(&mutated)).is_err(),
            "accepted {label}"
        );
        let restored = restore_core_projection_forms(&mutated, &projection);
        assert!(
            std::panic::catch_unwind(|| inspect_selectors(&restored)).is_err(),
            "canonical accepted {label}"
        );
    }
    for (label, mutation) in [
        ("duplicate private route", 0),
        ("duplicate private inbound", 1),
        ("hidden logical route", 2),
        ("physical detour", 3),
        ("extra DNS borrower", 4),
        ("extra unpaired inbound", 5),
        ("nested probe edge", 6),
        ("duplicate private DNS server", 7),
    ] {
        let mut mutated = original.clone();
        match mutation {
            0 => {
                let rule = mutated["route"]["rules"][0].clone();
                mutated["route"]["rules"].as_array_mut().unwrap().push(rule);
            }
            1 => {
                let inbound = mutated["inbounds"][0].clone();
                mutated["inbounds"].as_array_mut().unwrap().push(inbound);
            }
            2 => mutated["route"]["rules"]
                .as_array_mut()
                .unwrap()
                .push(json!({
                    "type":"logical","mode":"or","rules":[{"domain":[DOMAIN]}],
                    "action":"route","outbound":"probe-selector-0"
                })),
            3 => mutated["outbounds"][0]["detour"] = "probe-selector-0".into(),
            4 => mutated["dns"]["servers"]
                .as_array_mut()
                .unwrap()
                .push(json!({
                    "tag":"dns-user","detour":"probe-selector-0"
                })),
            5 => mutated["inbounds"].as_array_mut().unwrap().push(json!({
                "type":"http","tag":"probe-in-00","listen":"127.0.0.1","listen_port":31002
            })),
            6 => mutated["route"]["rules"]
                .as_array_mut()
                .unwrap()
                .push(json!({
                    "type":"logical","mode":"or","rules":[{
                        "domain":[DOMAIN],"outbound":"probe-selector-0"
                    }],"action":"route","outbound":"direct"
                })),
            7 => {
                let server = mutated["dns"]["servers"][0].clone();
                mutated["dns"]["servers"]
                    .as_array_mut()
                    .unwrap()
                    .push(server);
            }
            _ => unreachable!(),
        }
        assert!(
            std::panic::catch_unwind(|| inspect_selectors(&mutated)).is_err(),
            "accepted {label}"
        );
        let restored = restore_core_projection_forms(&mutated, &projection);
        assert!(
            std::panic::catch_unwind(|| inspect_selectors(&restored)).is_err(),
            "canonical accepted {label}"
        );
    }
}

// Use the same production seed source as start(), restricted to this checkout.
fn fixture_bundled_rules_dir() -> PathBuf {
    let expected = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../resources/data")
        .canonicalize()
        .unwrap();
    let actual = crate::runtime::geo_seed::resolve_bundled_data_dir()
        .expect("admitted checkout must contain bundled rule sets")
        .canonicalize()
        .unwrap();
    assert_eq!(actual, expected, "no alternate bundled rule source");
    actual
}

// The route writer externalizes the mirrored custom rule even for one domain.
// DNS effects use their own literal matcher path. Keep these exact contracts
// together; an accepted inline source still has the same route rule_set edge.
fn fixture_route_terminal() -> Value {
    json!({"rule_set":[FIXTURE_RULE_SET],"action":"route","outbound":"rule-sel-loop-route"})
}

fn fixture_dns_terminal(server: &str) -> Value {
    json!({"domain":[DOMAIN],"action":"route","server":server})
}

fn fixture_custom_rules() -> Value {
    json!([{"domain":[DOMAIN]}])
}

// Binary sets remain local in HOT's accepted configuration. Prove their exact
// catalog ownership and bytes, rather than pretending the SRS contains DOMAIN.
// Ordered terminal rules keep later geo matches off the fixture DOMAIN path.
fn inspect_rule_sets(
    config_dir: &Path,
    custom_dir: &Path,
    bundled_dir: &Path,
    generated: &Value,
    expected_dns: &str,
) -> (Value, Vec<Value>) {
    use polaris_config_engine::user_config::builtin_geo_rulesets::{
        find_builtin, is_valid_srs_bytes,
    };
    let runtime_dir = config_dir.join("rules");
    let mut route = generated["route"].clone();
    let mut tags = std::collections::BTreeSet::new();
    let mut sources = Vec::new();
    for set in route["rule_set"].as_array_mut().unwrap() {
        let tag = set["tag"].as_str().unwrap().to_string();
        assert!(tags.insert(tag.clone()), "duplicate rule-set definition");
        if let Some(entry) = find_builtin(&tag) {
            let path = runtime_dir.join(entry.file_name);
            assert_eq!(
                *set,
                json!({"tag":tag,"type":"local","format":"binary","path":path}),
                "builtin rule sets require their exact private catalog path"
            );
            let source = bundled_dir.join(path.file_name().unwrap());
            let expected = regular(&source, 1024 * 1024);
            assert!(expected.len() >= 3);
            assert!(is_valid_srs_bytes(expected[..3].try_into().unwrap()));
            let actual = regular(&path, expected.len() as u64);
            assert!(
                actual == expected,
                "seeded SRS must equal checkout bytes: actual {} bytes/{}; expected {} bytes/{}",
                actual.len(),
                polaris_updater::sha256_hex(&actual),
                expected.len(),
                polaris_updater::sha256_hex(&expected)
            );
            sources.push(json!({"tag":tag,"path":path,"source":source,
                "size":actual.len(),"sha256":polaris_updater::sha256_hex(&actual)}));
            continue;
        }
        assert_eq!(tag, FIXTURE_RULE_SET, "only the owned custom source tag");
        let rules = fixture_custom_rules();
        match set["type"].as_str().unwrap() {
            "local" => {
                let path = custom_dir.join(format!("{FIXTURE_RULE_SET}.json"));
                assert_eq!(
                    *set,
                    json!({"tag":FIXTURE_RULE_SET,"type":"local","format":"source","path":path}),
                    "exact owned custom source identity"
                );
                let content: Value = serde_json::from_slice(&regular(&path, 65536)).unwrap();
                assert_eq!(
                    content,
                    json!({"version":1,"rules":rules}),
                    "exact owned source document"
                );
            }
            "inline" => assert_eq!(
                *set,
                json!({"tag":FIXTURE_RULE_SET,"type":"inline","rules":rules}),
                "exact accepted custom snapshot"
            ),
            _ => panic!("no remote or unreviewed rule source"),
        }
        // Only this verified custom source is embedded by the HOT snapshot.
        *set = json!({"tag":FIXTURE_RULE_SET,"type":"inline","rules":rules});
    }
    assert!(
        tags.contains(FIXTURE_RULE_SET),
        "owned terminal source must exist"
    );
    for (section, terminal) in [
        ("route", fixture_route_terminal()),
        ("dns", fixture_dns_terminal(expected_dns)),
    ] {
        let rules = generated[section]["rules"].as_array().unwrap();
        let positions: Vec<_> = rules
            .iter()
            .enumerate()
            .filter_map(|(index, rule)| (rule == &terminal).then_some(index))
            .collect();
        assert_eq!(
            positions.len(),
            1,
            "unique exact {section} fixture terminal rule"
        );
        for (index, rule) in rules.iter().enumerate() {
            assert!(rule["rules"].is_null(), "no unreviewed logical rule branch");
            for tag in strings(&rule["rule_set"]) {
                assert!(tags.contains(tag), "undefined rule-set reference: {tag}");
                if tag == FIXTURE_RULE_SET {
                    assert!(
                        section == "route" && index == positions[0] && rule == &terminal,
                        "owned custom source is reachable only through its exact route terminal"
                    );
                } else {
                    assert!(
                        index > positions[0],
                        "builtin rule-set matching must follow the fixture terminal rule"
                    );
                }
            }
        }
    }
    (route, sources)
}

fn fixture_cidr(value: &Value) -> Option<(std::net::IpAddr, u8)> {
    let text = value.as_str()?;
    let (address, prefix) = text
        .split_once('/')
        .map_or((text, None), |(a, p)| (a, Some(p)));
    let address: std::net::IpAddr = address.parse().ok()?;
    let width = if address.is_ipv4() { 32 } else { 128 };
    let prefix = match prefix {
        Some(prefix) => prefix.parse::<u8>().ok()?,
        None => width,
    };
    (prefix <= width).then_some((address, prefix))
}

// Accepted-file inspection only: c76 option/{group,rule_action,rule_set}.go and
// third_party/sing/.../badoption/{listable,netip}.go serialize these equivalent
// forms. Restore only against an already admitted production projection. Never
// use this on preflight/startup input: its explicit false policy remains required.
fn restore_core_projection_forms(generated: &Value, expected: &Value) -> Value {
    let mut restored = generated.clone();
    for outbound in restored
        .get_mut("outbounds")
        .and_then(Value::as_array_mut)
        .into_iter()
        .flatten()
    {
        if outbound["type"] == "selector"
            && matches!(
                outbound["tag"].as_str(),
                Some("proxy-selector" | "rule-sel-loop-route")
            )
            && outbound.get("interrupt_exist_connections").is_none()
            && expected["selectors"]
                .as_array()
                .unwrap()
                .iter()
                .any(|item| {
                    item["tag"] == outbound["tag"] && item["interrupt_exist_connections"] == false
                })
        {
            outbound["interrupt_exist_connections"] = false.into();
        }
    }
    for section in ["route", "dns"] {
        let expected_rules = expected[section]["rules"].as_array().into_iter().flatten();
        for (rule, template) in restored
            .pointer_mut(&format!("/{section}/rules"))
            .and_then(Value::as_array_mut)
            .into_iter()
            .flatten()
            .zip(expected_rules)
        {
            if rule.get("action").is_none() && template["action"] == "route" {
                rule["action"] = "route".into();
            }
            for key in [
                "inbound",
                "network",
                "protocol",
                "domain",
                "domain_suffix",
                "domain_keyword",
                "domain_regex",
                "ip_cidr",
                "port",
                "process_name",
                "rule_set",
                "query_type",
            ] {
                let Some(wanted) = template[key].as_array() else {
                    continue;
                };
                if wanted.is_empty() {
                    continue;
                }
                let Some(actual) = rule.get(key) else {
                    continue;
                };
                let actual = actual
                    .as_array()
                    .cloned()
                    .unwrap_or_else(|| vec![actual.clone()]);
                if actual.len() == wanted.len()
                    && actual.iter().zip(wanted).all(|(a, b)| {
                        a == b
                            || (key == "ip_cidr"
                                && fixture_cidr(a).is_some_and(|ip| Some(ip) == fixture_cidr(b)))
                    })
                {
                    rule[key] = Value::Array(wanted.clone());
                }
            }
        }
    }
    let expected_sets = expected["route"]["rule_set"]
        .as_array()
        .into_iter()
        .flatten();
    for (set, template) in restored
        .pointer_mut("/route/rule_set")
        .and_then(Value::as_array_mut)
        .into_iter()
        .flatten()
        .zip(expected_sets)
    {
        if set["tag"] != template["tag"] {
            continue;
        }
        if template["type"] == "local"
            && template["format"] == "binary"
            && set["type"] == "local"
            && set["path"] == template["path"]
            && set.get("format").is_none()
        {
            set["format"] = "binary".into();
        }
        if template["type"] == "inline" {
            if set["rules"] == json!([{"domain":DOMAIN}])
                && template["rules"] == json!([{"domain":[DOMAIN]}])
            {
                set["rules"] = template["rules"].clone();
            }
            if set.get("type").is_none() && set["rules"] == template["rules"] {
                set["type"] = "inline".into();
            }
        }
    }
    restored
}

fn rule_set_admission_fixture() -> (TestDir, Value) {
    let directory = TestDir::new("polaris-rule-set-admission-");
    for name in ["rules", "bundled", "custom"] {
        std::fs::create_dir(directory.join(name)).unwrap();
    }
    // Deliberately larger than the custom JSON limit; no SRS decoder is claimed.
    let mut bytes = vec![0u8; 70_000];
    bytes[..3].copy_from_slice(b"SRS");
    for name in ["rules", "bundled"] {
        std::fs::write(directory.join(name).join("geosite-cn.srs"), &bytes).unwrap();
    }
    let custom = directory.join(format!("custom/{FIXTURE_RULE_SET}.json"));
    let content = json!({"version":1,"rules":fixture_custom_rules()});
    std::fs::write(&custom, serde_json::to_vec(&content).unwrap()).unwrap();
    let generated = json!({
        "route":{"rule_set":[
            {"tag":"geosite-cn","type":"local","format":"binary",
                "path":directory.join("rules/geosite-cn.srs")},
            {"tag":FIXTURE_RULE_SET,"type":"local","format":"source","path":custom}
        ],"rules":[
            fixture_route_terminal(),
            {"rule_set":"geosite-cn","action":"route","outbound":"direct"},
            {"ip_cidr":["127.0.0.1/32","::1/128","::ffff:a00:0/104"],
                "action":"route","outbound":"direct"}
        ]},
        "dns":{"rules":[
            fixture_dns_terminal("dns-user-loop-a"),
            {"rule_set":"geosite-cn","server":"dns-domestic"}
        ]}
    });
    (directory, generated)
}

#[test]
fn builtin_rule_set_admission_accepts_owned_sources_and_order() {
    fixture_generated_transition_admission();
    let (directory, generated) = rule_set_admission_fixture();
    let inspect = |value: &Value| {
        inspect_rule_sets(
            &directory,
            &directory.join("custom"),
            &directory.join("bundled"),
            value,
            "dns-user-loop-a",
        )
    };
    let (route, sources) = inspect(&generated);
    assert_eq!(route["rule_set"][0], generated["route"]["rule_set"][0]);
    assert_eq!(sources.len(), 1);
    assert_eq!(sources[0]["size"], 70_000);
    let source = regular(&directory.join("bundled/geosite-cn.srs"), 1024 * 1024);
    assert_eq!(sources[0]["sha256"], polaris_updater::sha256_hex(&source));
    assert_eq!(
        route["rule_set"][1],
        json!({"tag":FIXTURE_RULE_SET,"type":"inline","rules":fixture_custom_rules()})
    );
    let mut inline = generated.clone();
    inline["route"]["rule_set"][1] = route["rule_set"][1].clone();
    assert_eq!(inspect(&inline), (route.clone(), sources.clone()));
    let projection = json!({"route":route,"dns":generated["dns"],"selectors":[]});
    let mut canonical = inline.clone();
    canonical["route"]["rule_set"][0]
        .as_object_mut()
        .unwrap()
        .remove("format");
    canonical["route"]["rule_set"][1]
        .as_object_mut()
        .unwrap()
        .remove("type");
    canonical["route"]["rule_set"][1]["rules"] = json!([{"domain":DOMAIN}]);
    for section in ["route", "dns"] {
        canonical[section]["rules"][0]
            .as_object_mut()
            .unwrap()
            .remove("action");
        let matcher = if section == "route" {
            "rule_set"
        } else {
            "domain"
        };
        canonical[section]["rules"][0][matcher] = if section == "route" {
            FIXTURE_RULE_SET.into()
        } else {
            DOMAIN.into()
        };
    }
    let addresses = json!(["127.0.0.1", "::1", "::ffff:10.0.0.0/104"]);
    canonical["route"]["rules"][2]["ip_cidr"] = addresses;
    let restored = restore_core_projection_forms(&canonical, &projection);
    assert_eq!(inspect(&restored), (projection["route"].clone(), sources));
}

#[test]
fn builtin_rule_set_admission_rejects_source_and_order_mutations() {
    let (directory, generated) = rule_set_admission_fixture();
    let inspect = |value: &Value| {
        inspect_rule_sets(
            &directory,
            &directory.join("custom"),
            &directory.join("bundled"),
            value,
            "dns-user-loop-a",
        )
    };
    for (pointer, replacement) in [
        ("/route/rule_set/0/tag", json!("geosite-cn-lookalike")),
        ("/route/rule_set/0/type", json!("remote")),
        ("/route/rule_set/0/format", json!("source")),
        (
            "/route/rule_set/0/path",
            json!(directory.join("bundled/geosite-cn.srs")),
        ),
        (
            "/route/rule_set/0/path",
            json!(directory.join("rules/../rules/geosite-cn.srs")),
        ),
        ("/route/rule_set/0/path", json!("rules/geosite-cn.srs")),
        (
            "/route/rule_set/1/path",
            json!(directory.join("bundled/owned.json")),
        ),
        ("/route/rule_set/1/format", json!("binary")),
        ("/route/rule_set/1/tag", json!("custom-rule-unowned")),
        ("/route/rules/0/rule_set", json!(["geosite-cn"])),
        ("/route/rules/1/rule_set", json!([FIXTURE_RULE_SET])),
        ("/dns/rules/1/rule_set", json!([FIXTURE_RULE_SET])),
        ("/route/rules/0/outbound", json!("direct")),
        ("/dns/rules/0/server", json!("dns-domestic")),
        ("/route/rules/1/rule_set", json!("undefined")),
        ("/dns/rules/1/rule_set", json!("undefined")),
    ] {
        let mut mutated = generated.clone();
        *mutated.pointer_mut(pointer).unwrap() = replacement;
        assert!(
            std::panic::catch_unwind(|| inspect(&mutated)).is_err(),
            "{pointer}"
        );
    }
    // Old handcrafted inline terminals must not stand in for a missing source.
    let mut old_inline = generated.clone();
    old_inline["route"]["rules"][0] =
        json!({"domain":[DOMAIN],"action":"route","outbound":"rule-sel-loop-route"});
    assert!(std::panic::catch_unwind(|| inspect(&old_inline)).is_err());
    for section in ["route", "dns"] {
        for kind in 0..3 {
            let mut mutated = generated.clone();
            let rules = mutated[section]["rules"].as_array_mut().unwrap();
            match kind {
                0 => rules.swap(0, 1),
                1 => rules.push(rules[0].clone()),
                2 => rules.push(json!({"type":"logical","mode":"or","rules":[
                    {"rule_set":"geosite-cn"}
                ]})),
                _ => unreachable!(),
            }
            assert!(
                std::panic::catch_unwind(|| inspect(&mutated)).is_err(),
                "{section}/{kind}"
            );
        }
    }
    for kind in 0..4 {
        let mut mutated = generated.clone();
        let sets = mutated["route"]["rule_set"].as_array_mut().unwrap();
        match kind {
            0 => sets.push(sets[0].clone()),
            1 => {
                sets[0] = json!({
                    "tag":"geosite-cn","type":"inline","rules":[{"domain":[DOMAIN]}]
                });
            }
            2 => sets[0]["url"] = "https://unreviewed.invalid/rules.srs".into(),
            3 => {
                sets[1] = json!({
                    "tag":FIXTURE_RULE_SET,"type":"inline","rules":[{"domain":["other.test"]}]
                });
            }
            _ => unreachable!(),
        }
        assert!(std::panic::catch_unwind(|| inspect(&mutated)).is_err());
    }
    let (route, _) = inspect(&generated);
    let projection = json!({"route":route,"dns":generated["dns"],"selectors":[]});
    for (pointer, value) in [
        ("/route/rule_set/0/format", Value::Null),
        ("/route/rules/0/action", Value::Null),
        ("/route/rules/0/rule_set", json!("other")),
        ("/dns/rules/0/domain", json!([DOMAIN, "other.test"])),
        (
            "/route/rules/2/ip_cidr",
            json!(["127.0.0.1/24", "::1", "::ffff:10.0.0.0/104"]),
        ),
        (
            "/route/rules/2/ip_cidr",
            json!(["::1", "127.0.0.1", "::ffff:10.0.0.0/104"]),
        ),
        ("/route/rules/0/rule_set", json!(true)),
        ("/route/rules/2/ip_cidr", json!("invalid/32")),
    ] {
        let mut mutated = generated.clone();
        *mutated.pointer_mut(pointer).unwrap() = value;
        let restored = restore_core_projection_forms(&mutated, &projection);
        let result = std::panic::catch_unwind(|| inspect(&restored));
        assert!(
            result.is_err() || result.unwrap().0 != projection["route"],
            "{pointer}"
        );
    }
    let custom = directory.join(format!("custom/{FIXTURE_RULE_SET}.json"));
    let custom_bytes = std::fs::read(&custom).unwrap();
    for document in [
        json!({"version":3,"rules":fixture_custom_rules()}),
        json!({"version":1,"rules":[{"domain":["other.test"]}]}),
        json!({"version":1,"rules":[{"domain":[DOMAIN],"invert":true}]}),
        json!({"version":1,"rules":fixture_custom_rules(),"extra":true}),
    ] {
        std::fs::write(&custom, serde_json::to_vec(&document).unwrap()).unwrap();
        assert!(std::panic::catch_unwind(|| inspect(&generated)).is_err());
    }
    std::fs::remove_file(&custom).unwrap();
    assert!(std::panic::catch_unwind(|| inspect(&generated)).is_err());
    std::fs::write(&custom, custom_bytes).unwrap();
    let path = directory.join("rules/geosite-cn.srs");
    let original = std::fs::read(&path).unwrap();
    let mut changed = original.clone();
    changed[3] = 1;
    std::fs::write(&path, changed).unwrap();
    assert!(std::panic::catch_unwind(|| inspect(&generated)).is_err());
    std::fs::remove_file(&path).unwrap();
    assert!(std::panic::catch_unwind(|| inspect(&generated)).is_err());
    std::os::unix::fs::symlink(directory.join("bundled/geosite-cn.srs"), &path).unwrap();
    assert!(std::panic::catch_unwind(|| inspect(&generated)).is_err());
    std::fs::remove_file(&path).unwrap();
    std::fs::write(&path, b"not SRS").unwrap();
    std::fs::write(directory.join("bundled/geosite-cn.srs"), b"not SRS").unwrap();
    assert!(std::panic::catch_unwind(|| inspect(&generated)).is_err());
}

fn inspect_generated(rt: &ProxyRuntime, config: &Value, generated: &Value) -> Value {
    let mut listeners = Vec::new();
    for inbound in generated["inbounds"].as_array().unwrap() {
        assert!(matches!(
            inbound["type"].as_str(),
            Some("mixed" | "http" | "socks")
        ));
        let tag = inbound["tag"].as_str().unwrap();
        assert!(
            matches!(
                tag,
                "mixed-in" | "update-in" | "subscription-update-in" | "probe-proxy-in"
            ) || tag
                .strip_prefix("probe-in-")
                .is_some_and(|n| n.parse::<usize>().is_ok())
        );
        assert_eq!(inbound["listen"], "127.0.0.1", "no TUN or public listener");
        listeners
            .push(json!({"type":inbound["type"],"tag":inbound["tag"],"listen":inbound["listen"]}));
    }
    assert!(listeners.iter().any(|i| i["tag"] == "mixed-in"));
    for service in generated["services"].as_array().into_iter().flatten() {
        assert_eq!(service["type"], "api");
        assert_eq!(service["listen"], "127.0.0.1");
        assert!(service["dashboard"].is_null(), "no dashboard download");
    }
    assert!(
        generated["endpoints"].as_array().is_none_or(Vec::is_empty),
        "no mesh endpoints"
    );
    let outbounds = generated["outbounds"].as_array().unwrap();
    let tags: std::collections::BTreeSet<_> = outbounds
        .iter()
        .map(|o| o["tag"].as_str().unwrap())
        .collect();
    let owned_ports: Vec<_> = config["servers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["port"].clone())
        .collect();
    let mut physical = Vec::new();
    for outbound in outbounds {
        match outbound["type"].as_str().unwrap() {
            "socks" => {
                assert_eq!(outbound["server"], "127.0.0.1");
                assert!(owned_ports.contains(&outbound["server_port"]));
                assert!(outbound["detour"].is_null());
                physical.push(json!({"tag":outbound["tag"],"server":outbound["server"],"port":outbound["server_port"]}));
            }
            "selector" => {
                for tag in strings(&outbound["outbounds"]) {
                    assert!(tags.contains(tag));
                }
            }
            "direct" | "block" => {}
            other => panic!("unexpected outbound or autonomous probe: {other}"),
        }
    }
    assert_eq!(physical.len(), 2);
    let selectors = inspect_selectors(generated);
    for (tag, target) in [
        ("proxy-selector", &config["selectedServerId"]),
        (
            "rule-sel-loop-route",
            &config["trafficRules"][0]["targetServerId"],
        ),
    ] {
        let node = config["servers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|node| &node["id"] == target)
            .expect("selector target must be an owned node");
        let selector = outbounds
            .iter()
            .find(|outbound| outbound["tag"] == tag)
            .unwrap();
        assert_eq!(
            selector["default"], node["name"],
            "exact fixture selector target: {tag}"
        );
    }
    let expected_dns = polaris_config_engine::user_config::dns_policy::dns_server_tag(
        config["dnsRules"][0]["effects"]["dns"]["action"]["serverId"]
            .as_str()
            .unwrap(),
    );
    let (route, rule_sources) = inspect_rule_sets(
        rt.config.dir(),
        &rt.custom_rules_dir(),
        &fixture_bundled_rules_dir(),
        generated,
        &expected_dns,
    );
    assert!(
        !rule_sources.is_empty(),
        "production builtin seed must be present"
    );
    let dns = &generated["dns"];
    let owned_dns_ports: Vec<_> = config["dnsServers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["endpoint"]["port"].clone())
        .collect();
    let mut owned_dns = std::collections::BTreeSet::new();
    let mut unused_dns = Vec::new();
    for server in dns["servers"].as_array().unwrap() {
        let tag = server["tag"].as_str().unwrap();
        if server["type"] == "udp" && server["server"] == "127.0.0.1" {
            assert!(owned_dns_ports.contains(&server["server_port"]));
            assert!(server["domain_resolver"].is_null());
            assert!(server["detour"].is_null() || server["detour"] == "direct");
            owned_dns.insert(tag);
        } else {
            assert!(
                matches!(tag, "dns-node" | "dns-local" | "dns-mdns" | "dns-lan")
                    || tag.starts_with("dns-probe-exit-"),
                "unexpected non-owned DNS definition: {tag}"
            );
            unused_dns.push(tag);
        }
    }
    assert!(
        owned_dns.contains(dns["final"].as_str().unwrap()),
        "no external DNS final"
    );
    assert!(owned_dns.contains(
        generated["route"]["default_domain_resolver"]
            .as_str()
            .unwrap()
    ));
    let mut explicit_domain_routes = Vec::new();
    for rule in dns["rules"].as_array().unwrap() {
        assert!(
            rule["rules"].is_null(),
            "fixture DNS must not grow unreviewed logical branches"
        );
        if let Some(server) = rule["server"].as_str() {
            if !owned_dns.contains(server) {
                assert!(
                    excludes_fixture(rule),
                    "reachable external DNS rule: {server}"
                );
            }
            if rule["domain"] == json!([DOMAIN]) && !excludes_fixture(rule) {
                explicit_domain_routes.push(server);
            }
        }
    }
    assert_eq!(
        explicit_domain_routes,
        vec![expected_dns.as_str()],
        "unique owned fixture DNS rule"
    );
    assert_ne!(dns["optimistic"], true);
    assert!(dns["fakeip"].is_null());
    json!({"listeners":listeners,"physicalOutbounds":physical,"selectors":selectors,"dns":dns,"route":route,
        "ruleSources":rule_sources,"unusedDnsDefinitions":unused_dns,"domain":DOMAIN,"onlyTrafficInbound":"mixed-in"})
}

fn generate_fixture_graph(rt: &ProxyRuntime, config: &Value) -> Value {
    let typed: UserConfig = serde_json::from_value(config.clone()).unwrap();
    // No generated JSON is rewritten and no metadata/kernel check is bypassed.
    let deps = rt.generate_candidate_deps(&typed, config).unwrap();
    let outcome = polaris_config_engine::builder::generate_sing_box_config_with_report(
        &typed,
        &std::collections::BTreeMap::new(),
        &deps,
    )
    .expect("production builder safety preflight before core execution");
    assert!(outcome.invalid_nodes.is_empty());
    assert!(outcome.mesh_route_candidates.is_empty());
    assert!(outcome.network_canary.is_none());
    serde_json::to_value(outcome.config).unwrap()
}

fn preflight(rt: &ProxyRuntime, config: &Value) -> Value {
    inspect_generated(rt, config, &generate_fixture_graph(rt, config))
}

// Shared by real startup and ordinary preparation: production externalization,
// seeding, dependency capture, generation, and complete admission, without start.
async fn prepare_fixture_graph(rt: &ProxyRuntime, config: &Value) -> (Value, Value) {
    assert!(rt.management_api_stub.lock().unwrap().is_none());
    assert!(
        rt.error_emitter.get().is_none(),
        "no attached automatic probe emitter"
    );
    rt.write_custom_rule_files(&serde_json::from_value(config.clone()).unwrap())
        .await;
    let _ = fixture_bundled_rules_dir();
    crate::runtime::geo_seed::seed_builtin_rule_sets_into(
        rt.config.dir(),
        "real HOT fixture preflight",
        &crate::runtime::geo_seed::SeedOptions::default(),
    );
    let generated = generate_fixture_graph(rt, config);
    let checked = inspect_generated(rt, config, &generated);
    (generated, checked)
}

// Default selection, DNS and Auto already have strict compiler contracts. Keep
// them on the ordinary preparation path without duplicating their validators.
fn fixture_existing_reference_contracts(rt: &ProxyRuntime, original: &Value) {
    let typed = serde_json::from_value(original.clone()).unwrap();
    let deps = rt.generate_candidate_deps(&typed, original).unwrap();
    let check = |label: &str, value: Value, expected: bool| {
        let typed = serde_json::from_value(value).unwrap();
        assert!(super::super::runtime_config::runtime_route_targets_valid(
            &typed
        ));
        let result = polaris_config_engine::builder::generate_sing_box_config_with_report(
            &typed,
            &std::collections::BTreeMap::new(),
            &deps,
        );
        assert_eq!(
            result.is_ok(),
            expected,
            "existing reference contract: {label}"
        );
    };
    for (id, expected) in [
        ("node-b", true),
        ("__direct__", true),
        ("__block__", true),
        ("nonexistent-node", false),
    ] {
        let mut value = original.clone();
        value["selectedServerId"] = id.into();
        check("default selection", value, expected);
    }
    for (path, replacement) in [
        ("/dnsDefaults/proxyServerId", json!("missing-dns")),
        (
            "/dnsRules/0/effects/dns/action",
            json!({"type":"server","serverId":"missing-dns"}),
        ),
        (
            "/dnsRules/0/effects/dns/action",
            json!({"type":"group","groupId":"missing-group"}),
        ),
        (
            "/dnsServers/3/outbound",
            json!({"type":"node","nodeId":"nonexistent-node"}),
        ),
    ] {
        let mut value = original.clone();
        *value.pointer_mut(path).unwrap() = replacement;
        check(path, value, false);
    }
    let mut inherited = original.clone();
    inherited["dnsDefaults"]["proxyServerId"] = "".into();
    check("empty DNS default retains inheritance", inherited, true);

    let mut auto = original.clone();
    auto["autoSelectEnabled"] = true.into();
    auto["selectedServerId"] = "nonexistent-node".into();
    auto["selectionIntent"] = json!({"mode":"auto","scope":"manual"});
    check("Auto supersedes concrete default", auto.clone(), true);
    auto["selectionIntent"] =
        json!({"mode":"auto","scope":"subscription","subscriptionId":"missing-sub"});
    check("Auto absent scope", auto, false);

    let mut auto = original.clone();
    auto["autoSelectEnabled"] = true.into();
    for key in ["trafficRules", "policyRules", "customRules"] {
        let route = &mut auto[key][0];
        route.as_object_mut().unwrap().remove("targetServerId");
        route["effects"]["route"]
            .as_object_mut()
            .unwrap()
            .remove("targetServerId");
        route["effects"]["route"]["targetAutoSelect"] = json!({"scope":"manual"});
    }
    check("rule Auto remains valid", auto.clone(), true);
    auto["autoSelectEnabled"] = false.into();
    auto["autoSelectPins"] = json!([{"target":{"scope":"manual"},"serverId":"node-b"}]);
    check("frozen Auto with confirmed pin", auto.clone(), true);
    auto["autoSelectPins"][0]["serverId"] = "nonexistent-node".into();
    check("frozen Auto invalid pin", auto, false);
}

// This runs in the existing ordinary exact entrypoint. Production dependency
// capture briefly binds/releases loopback ports; no core, process, RPC, backend,
// DNS request, or outbound traffic is started. Each scenario owns a fresh dir.
fn fixture_generated_transition_admission() {
    let mut failures = Vec::new();
    for scenario in [
        "apply-and-status",
        "persistence-retry",
        "public-rpc-contract",
        "rejections-preserve-runtime",
    ] {
        let result = std::panic::catch_unwind(|| {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(async {
                    let directory = TestDir::new("polaris-fixture-generated-admission-");
                    let rt = test_runtime_in(directory.to_path_buf());
                    let initial = save_fixture_config(&rt.config, &store_admission_fixture());
                    let (generated, initial_projection) =
                        prepare_fixture_graph(&rt, &initial).await;
                    if scenario == "apply-and-status" {
                        fixture_existing_reference_contracts(&rt, &initial);
                    }
                    let loaded = rt.config.load_full().unwrap();
                    canonical_fixture(&loaded, &initial);
                    saved_fixture_config(&rt.config, &initial);
                    assert_eq!(preflight(&rt, &loaded), initial_projection);
                    assert_eq!(generated["log"]["level"], initial["logLevel"]);
                    if scenario == "public-rpc-contract" {
                        // Public RPC edits the generated core document, never App D.
                        let desired_before = rt.config.current().unwrap();
                        let before = std::fs::read(rt.config.path()).unwrap();
                        let mut candidate = generated;
                        candidate["log"]["level"] = "debug".into();
                        assert_eq!(
                            inspect_generated(&rt, &initial, &candidate),
                            initial_projection
                        );
                        assert_eq!(candidate["log"]["level"], "debug");
                        assert_eq!(rt.config.current().unwrap(), desired_before);
                        assert_eq!(std::fs::read(rt.config.path()).unwrap(), before);
                    } else {
                        let mut requested = initial.clone();
                        match scenario {
                            "apply-and-status" => requested = changed_graph(requested),
                            "persistence-retry" => requested["logLevel"] = "warn".into(),
                            "rejections-preserve-runtime" => requested["mixedPort"] = 31017.into(),
                            _ => unreachable!(),
                        }
                        let desired = save_fixture_config(&rt.config, &requested);
                        let desired_projection = preflight(&rt, &desired);
                        let loaded = rt.config.load_full().unwrap();
                        canonical_fixture(&loaded, &desired);
                        saved_fixture_config(&rt.config, &desired);
                        assert_eq!(preflight(&rt, &loaded), desired_projection);
                        // Use the admitted snapshot form only after actual production
                        // generation; this is preparation, not a claimed kernel Apply.
                        let mut inline_candidate = generate_fixture_graph(&rt, &desired);
                        assert_eq!(
                            inspect_generated(&rt, &desired, &inline_candidate),
                            desired_projection
                        );
                        let custom = inline_candidate["route"]["rule_set"]
                            .as_array_mut()
                            .unwrap()
                            .iter_mut()
                            .find(|set| set["tag"] == FIXTURE_RULE_SET)
                            .unwrap()
                            .as_object_mut()
                            .unwrap();
                        custom.remove("path");
                        custom.remove("format");
                        custom.insert("type".into(), "inline".into());
                        custom.insert("rules".into(), fixture_custom_rules());
                        assert_eq!(
                            inspect_generated(&rt, &desired, &inline_candidate),
                            desired_projection
                        );
                        assert_eq!(inline_candidate["log"]["level"], desired["logLevel"]);
                        assert_eq!(
                            inline_candidate["inbounds"]
                                .as_array()
                                .unwrap()
                                .iter()
                                .find(|inbound| inbound["tag"] == "mixed-in")
                                .unwrap()["listen_port"],
                            desired["mixedPort"]
                        );
                        assert_eq!(save_fixture_config(&rt.config, &desired), desired);
                        let loaded = rt.config.load_full().unwrap();
                        canonical_fixture(&loaded, &desired);
                        saved_fixture_config(&rt.config, &desired);
                        assert_eq!(preflight(&rt, &loaded), desired_projection);
                        if scenario == "rejections-preserve-runtime" {
                            let mut invalid = initial.clone();
                            set_fixture_route_target(&mut invalid, "nonexistent-node");
                            let before = rt.config.current().unwrap();
                            let bytes = std::fs::read(rt.config.path()).unwrap();
                            if rt.config.save_full(&invalid).is_ok() {
                                for key in ["trafficRules", "policyRules", "customRules"] {
                                    assert_eq!(rt.config.current().unwrap()[key], invalid[key]);
                                }
                            } else {
                                assert_eq!(rt.config.current().unwrap(), before);
                                assert_eq!(std::fs::read(rt.config.path()).unwrap(), bytes);
                            }
                            // Even if the store admits it, generation's selector
                            // fallback must not pass our exact owned-target gate.
                            assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                preflight(&rt, &invalid)
                            }))
                            .is_err());
                        }
                    }
                    assert!(!rt.status().running);
                    assert_eq!(rt.status().pid, 0);
                    assert!(rt.current_config_snapshot().is_none());
                    assert!(
                        !rt.runtime_config_path().exists(),
                        "preparation cannot publish a CLI source"
                    );
                });
        });
        match result {
            Ok(()) => eprintln!("prepared real fixture: {scenario}"),
            Err(error) => {
                let message = error
                    .downcast_ref::<String>()
                    .map(String::as_str)
                    .or_else(|| error.downcast_ref::<&str>().copied())
                    .unwrap_or("non-string panic");
                failures.push(format!("{scenario}: {message}"));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "production fixture preparation failures:\n{}",
        failures.join("\n")
    );
}

struct Running {
    // First field cleans the owned Child before its directory is dropped on unwind.
    _cleanup: RealCoreCleanupGuard,
    _directory: TestDir,
    rt: Arc<ProxyRuntime>,
    client: SingBoxApiClient,
    initial: daemon::RuntimeCapabilities,
    pid: u32,
    mixed: u16,
}

impl Running {
    async fn start(admission: &Admission, config: Value, evidence: &mut Evidence) -> Self {
        // Avoid the legacy test factory's age-based sweep of other tests' dirs.
        let directory = TestDir::new("polaris-runtime-loopback-");
        let rt = test_runtime_in(directory.clone());
        let cleanup = RealCoreCleanupGuard::new(&rt);
        if admission.binding_sha256.is_some() {
            admission.verify_binary();
        }
        rt.inject_real_core_for_test(admission.binary.clone());
        let config = save_fixture_config(&rt.config, &config);
        let (_, checked) = prepare_fixture_graph(&rt, &config).await;
        evidence.record(json!({"phase":"prestart-safety","projection":checked,
            "claim":"fixture-reachable closure only; unused production DNS definitions are statically excluded",
            "autoProbeEmitterAttached":false,"canonicalNoTun":true}));
        if admission.binding_sha256.is_some() {
            admission.verify_binary();
        }
        let status = tokio::time::timeout(Duration::from_secs(45), rt.start(config.clone()))
            .await
            .expect("bounded real start")
            .expect("real App start");
        assert!(status.running && status.pid != 0 && ps_alive(status.pid));
        // Cold start drains the deletion journal through a real store reload.
        // Its compatibility projections must agree with the admitted start input.
        saved_fixture_config(&rt.config, &config);
        assert_eq!(rt.current_config_snapshot().as_ref(), Some(&config));
        let executable = std::fs::read_link(format!("/proc/{}/exe", status.pid)).unwrap();
        assert_eq!(
            executable, admission.binary,
            "actual child executable identity"
        );
        admission.verify_binary();
        let client = SingBoxApiClient::connect(
            Endpoint::new("127.0.0.1", status.clash_api_port),
            FIXTURE_MANAGEMENT_SECRET,
        )
        .await
        .expect("real authenticated management client");
        let initial = client.get_runtime_capabilities().await.unwrap();
        evidence.record(
            json!({"phase":"started","pid":status.pid,"capabilities":capabilities(&initial)}),
        );
        admission.capabilities(&initial);
        let path = rt.runtime_config_path();
        let bytes = regular(&path, 4 * 1024 * 1024);
        let generated: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            inspect_generated(&rt, &config, &generated),
            checked,
            "actual generated safety projection must match production preflight"
        );
        let argv = std::fs::read(format!("/proc/{}/cmdline", status.pid)).unwrap();
        let args: Vec<_> = argv
            .split(|b| *b == 0)
            .filter(|b| !b.is_empty())
            .map(|b| String::from_utf8(b.to_vec()).unwrap())
            .collect();
        let config_paths: Vec<_> = args
            .windows(2)
            .filter(|a| a[0] == "-c")
            .map(|a| a[1].as_str())
            .collect();
        assert_eq!(
            config_paths,
            vec![path.to_str().unwrap()],
            "the persisted file must be actual CLI restart source"
        );
        assert!(args.iter().any(|a| a == "run"));
        evidence.record(json!({"phase":"source","cliConfigPath":path,
            "initialBytesSha256":polaris_updater::sha256_hex(&bytes),"argv":args,
            "actualExecutable":executable,"actualExecutableSha256":admission.receipt["binary"]["sha256"]}));
        Self {
            _cleanup: cleanup,
            _directory: directory,
            rt,
            client,
            initial,
            pid: status.pid,
            mixed: status.mixed_port,
        }
    }

    async fn status(&self, transaction: &str) -> daemon::RuntimeConfigStatus {
        self.client
            .get_runtime_config_status(daemon::RuntimeConfigStatusRequest {
                contract_version: 1,
                transaction_id: transaction.into(),
                runtime_incarnation: self.initial.active.as_ref().unwrap().incarnation.clone(),
            })
            .await
            .unwrap()
    }

    async fn unchanged_process(&self) -> daemon::RuntimeCapabilities {
        assert_eq!(self.rt.status().pid, self.pid);
        assert!(ps_alive(self.pid));
        let cap = self.client.get_runtime_capabilities().await.unwrap();
        assert_eq!(
            cap.active.as_ref().unwrap().incarnation,
            self.initial.active.as_ref().unwrap().incarnation
        );
        assert_eq!(
            cap.implementation_manifest_digest,
            self.initial.implementation_manifest_digest
        );
        cap
    }

    async fn finish(&self, admission: &Admission, evidence: &mut Evidence) {
        admission.verify_binary();
        tokio::time::timeout(Duration::from_secs(15), self.rt.stop())
            .await
            .unwrap()
            .unwrap();
        assert!(!self.rt.status().running);
        assert!(!ps_alive(self.pid), "owned core reaped");
        admission.verify_binary();
        evidence.record(json!({"phase":"stopped","pid":self.pid,"ownedChildReaped":true}));
    }
}

async fn connect(mixed: u16) -> tokio::net::TcpStream {
    tokio::time::timeout(IO_BOUND, async {
        let mut socket = tokio::net::TcpStream::connect(("127.0.0.1", mixed))
            .await
            .unwrap();
        socket.write_all(&[5, 1, 0]).await.unwrap();
        let mut greeting = [0; 2];
        socket.read_exact(&mut greeting).await.unwrap();
        assert_eq!(greeting, [5, 0]);
        let mut request = vec![5, 1, 0, 3, u8::try_from(DOMAIN.len()).unwrap()];
        request.extend_from_slice(DOMAIN.as_bytes());
        request.extend_from_slice(&80u16.to_be_bytes());
        socket.write_all(&request).await.unwrap();
        let mut reply = [0; 4];
        socket.read_exact(&mut reply).await.unwrap();
        assert_eq!(reply[..3], [5, 0, 0]);
        let len = match reply[3] {
            1 => 4,
            4 => 16,
            3 => usize::from(socket.read_u8().await.unwrap()),
            _ => panic!("SOCKS reply address"),
        };
        let mut bound = vec![0; len + 2];
        socket.read_exact(&mut bound).await.unwrap();
        socket
    })
    .await
    .expect("bounded real SOCKS connect")
}

async fn exchange(socket: &mut tokio::net::TcpStream, label: u8, nonce: &[u8]) {
    tokio::time::timeout(IO_BOUND, async {
        socket.write_all(nonce).await.unwrap();
        let mut actual = vec![0; nonce.len() * 2];
        socket.read_exact(&mut actual).await.unwrap();
        let expected: Vec<_> = nonce.iter().flat_map(|b| [label, *b]).collect();
        assert_eq!(actual, expected, "actual retained TCP byte stream");
    })
    .await
    .expect("bounded retained TCP exchange");
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "GitHub exact-source real core only; POLARIS_SINGBOX_PATH alone is insufficient"]
async fn real_hot_dispatcher_apply_and_status() {
    let _serial = lock_real_core_tests().await;
    let admission = Admission::read();
    let mut evidence = Evidence::new(&admission, "real_hot_dispatcher_apply_and_status");
    let network = Network::new();
    let running = Running::start(&admission, network.config(), &mut evidence).await;
    let mut old = connect(running.mixed).await;
    let nonce = format!(
        "{}-old-before",
        running.initial.active.as_ref().unwrap().incarnation
    );
    exchange(&mut old, b'A', nonce.as_bytes()).await;
    assert_eq!(network.a.targets.lock().unwrap()[0].host, "127.0.0.2");
    assert_eq!(network.a.targets.lock().unwrap()[0].port, 80);
    assert_eq!(network.a.targets.lock().unwrap().len(), 1);
    assert!(network.b.targets.lock().unwrap().is_empty());
    assert!(network.dns_a.count() > 0);
    assert_eq!(network.dns_b.count(), 0);
    let a_queries = network.dns_a.count();
    let requested = changed_graph(running.rt.config.current().unwrap());
    let desired = save_fixture_config(&running.rt.config, &requested);
    let desired_projection = preflight(&running.rt, &desired);
    evidence.record(json!({"phase":"preapply-safety","projection":desired_projection}));
    let result = running.rt.apply_pending_transaction(None).await;
    evidence.record(json!({"phase":"apply","appResult":result}));
    assert_eq!(
        result.status, "hotApplied",
        "real graph Apply must be durable; no restart/selector substitute"
    );
    assert!(result.restart_confirmation.is_none() && result.apply_id.is_none());
    let transaction = result.transaction_id.as_deref().expect("real transaction");
    let status = running.status(transaction).await;
    assert_eq!(
        status.lookup_state,
        daemon::RuntimeLookupState::Found as i32
    );
    let op = status.operation.as_ref().unwrap();
    assert!(op.durable);
    assert_eq!(op.persistence_mode, "exact_configuration_file");
    assert_eq!(
        running.rt.current_config_snapshot().as_ref(),
        Some(&desired)
    );
    assert_eq!(status.durable_applied_digest, op.candidate_digest);
    let accepted: Value =
        serde_json::from_slice(&std::fs::read(running.rt.runtime_config_path()).unwrap()).unwrap();
    let accepted = restore_core_projection_forms(&accepted, &desired_projection);
    assert_eq!(
        inspect_generated(&running.rt, &desired, &accepted),
        desired_projection
    );
    assert_eq!(
        op.active.as_ref().unwrap().applied_revision,
        running.initial.active.as_ref().unwrap().applied_revision + 1
    );
    let after_nonce = format!("{}-old-after", transaction);
    exchange(&mut old, b'A', after_nonce.as_bytes()).await;
    let mut fresh = connect(running.mixed).await;
    let new_nonce = format!("{}-new-after", transaction);
    exchange(&mut fresh, b'B', new_nonce.as_bytes()).await;
    assert_eq!(
        network.a.targets.lock().unwrap().len(),
        1,
        "no new admission to A"
    );
    assert_eq!(network.b.targets.lock().unwrap().len(), 1);
    assert_eq!(network.b.targets.lock().unwrap()[0].host, "127.0.0.3");
    assert_eq!(network.b.targets.lock().unwrap()[0].port, 80);
    assert_eq!(network.dns_a.count(), a_queries);
    assert!(
        network.dns_b.count() > 0,
        "fresh TTL0 upstream query, not cached evidence"
    );
    let cap = running.unchanged_process().await;
    assert_eq!(cap.active, op.active);
    evidence.record(json!({"phase":"data-plane","operation":operation(op),"capabilities":capabilities(&cap),
        "appRuntimeSnapshotMatchesDesired":true,"acceptedRestartSourceMatchesDesiredProjection":true,
        "oldSocketRetained":true,"oldBeforeNonce":nonce,"oldAfterNonce":after_nonce,"newNonce":new_nonce,
        "aTargets":network.a.observations(),"bTargets":network.b.observations(),
        "dnsAQueries":network.dns_a.count(),"dnsBQueries":network.dns_b.count(),"answerTtl":0,
        "runtimeFileSha256":polaris_updater::sha256_hex(&std::fs::read(running.rt.runtime_config_path()).unwrap())}));
    drop(old);
    drop(fresh);
    running.finish(&admission, &mut evidence).await;
    network.assert_only_fixture_queries();
    drop(network);
    evidence.passed();
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "GitHub exact-source real core only; POLARIS_SINGBOX_PATH alone is insufficient"]
async fn real_hot_dispatcher_persistence_retry() {
    let _serial = lock_real_core_tests().await;
    let admission = Admission::read();
    let mut evidence = Evidence::new(&admission, "real_hot_dispatcher_persistence_retry");
    let network = Network::new();
    let running = Running::start(&admission, network.config(), &mut evidence).await;
    let path = running.rt.runtime_config_path();
    let original = std::fs::read(&path).unwrap();
    let mut disturbed = original.clone();
    disturbed.push(b'\n'); // valid JSON, exact CLI owner bytes deliberately differ.
    std::fs::write(&path, &disturbed).unwrap();
    let mut desired = running.rt.config.current().unwrap();
    desired["logLevel"] = "warn".into();
    let desired = save_fixture_config(&running.rt.config, &desired);
    let desired_projection = preflight(&running.rt, &desired);
    let result = running.rt.apply_pending_transaction(None).await;
    evidence.record(json!({"phase":"pending","appResult":result}));
    assert_eq!(result.status, "hotDurabilityPending");
    let transaction = result.transaction_id.clone().unwrap();
    let pending = running.status(&transaction).await;
    let op = pending.operation.as_ref().unwrap();
    assert_eq!(
        op.state,
        daemon::RuntimeOperationState::CommittedDurabilityPending as i32
    );
    assert!(!op.durable);
    assert_eq!(op.persistence_mode, "exact_configuration_file");
    let published = op.active.clone();
    assert_eq!(
        published.as_ref().unwrap().applied_revision,
        running.initial.active.as_ref().unwrap().applied_revision + 1
    );
    assert_eq!(std::fs::read(&path).unwrap(), disturbed);
    // R describes the committed live graph even while CLI persistence is pending.
    assert_eq!(
        running.rt.current_config_snapshot().as_ref(),
        Some(&desired)
    );
    for _ in 0..3 {
        let polled = running.rt.runtime_apply_status(transaction.clone()).await;
        assert_eq!(polled.status, "hotDurabilityPending");
        assert_eq!(
            std::fs::read(&path).unwrap(),
            disturbed,
            "status must not retry persistence"
        );
        assert_eq!(running.status(&transaction).await.active, published);
        assert_eq!(
            running.rt.current_config_snapshot().as_ref(),
            Some(&desired)
        );
    }
    assert!(running.rt.pending_force_restart.read().unwrap().is_none());
    evidence.record(json!({"phase":"poll-read-only","operation":operation(op),
        "originalSha256":polaris_updater::sha256_hex(&original),
        "disturbedSha256":polaris_updater::sha256_hex(&disturbed),"polls":3,
        "appRuntimeSnapshotAlreadyMatchesCommittedGraph":true,"cliSourceDurabilityPending":true}));
    std::fs::write(&path, &original).unwrap();
    let recovered = running
        .rt
        .retry_runtime_persistence(transaction.clone())
        .await;
    assert_eq!(recovered.status, "hotApplied");
    let terminal = running.status(&transaction).await;
    let durable = terminal.operation.as_ref().unwrap();
    assert!(durable.durable);
    assert_eq!(durable.active, published, "retry must not republish graph");
    assert_eq!(terminal.durable_applied_digest, durable.candidate_digest);
    let accepted = std::fs::read(&path).unwrap();
    assert_ne!(accepted, original);
    let content: Value = serde_json::from_slice(&accepted).unwrap();
    assert_eq!(content["log"]["level"], "warn");
    let content = restore_core_projection_forms(&content, &desired_projection);
    assert_eq!(
        inspect_generated(&running.rt, &desired, &content),
        desired_projection
    );
    assert_eq!(
        running.rt.current_config_snapshot().as_ref(),
        Some(&desired)
    );
    let repeated = running
        .rt
        .retry_runtime_persistence(transaction.clone())
        .await;
    assert_eq!(repeated.status, "hotApplied");
    assert_eq!(std::fs::read(&path).unwrap(), accepted);
    assert_eq!(running.unchanged_process().await.active, published);
    evidence.record(
        json!({"phase":"retry-durable","operation":operation(durable),
        "appResult":recovered,"repeatedAppResult":repeated,
        "acceptedRestartSource":path,"acceptedBytesSha256":polaris_updater::sha256_hex(&accepted),
        "singleGraphPublication":true,"statusDidNotWrite":true}),
    );
    running.finish(&admission, &mut evidence).await;
    network.assert_only_fixture_queries();
    drop(network);
    evidence.passed();
}

fn unauthenticated<T>(result: Result<T, RuntimeConfigError>) {
    // Frozen gRPC status code 16; the App need not add a direct tonic dependency.
    assert!(
        matches!(result, Err(RuntimeConfigError::Call(ClientError::Status(ref status)))
        if status.code() as i32 == 16),
        "all six public methods authenticate before dispatch"
    );
}

fn failed_precondition<T>(result: Result<T, RuntimeConfigError>) {
    // Transport/deadline failures cannot stand in for a kernel rejection.
    assert!(
        matches!(result, Err(RuntimeConfigError::Call(ClientError::Status(ref status)))
        if status.code() as i32 == 9),
        "expected actual FailedPrecondition RPC rejection"
    );
}

fn apply_request(plan: &daemon::RuntimeConfigPlan) -> daemon::ApplyRuntimeConfigRequest {
    daemon::ApplyRuntimeConfigRequest {
        contract_version: 1,
        transaction_id: plan.transaction_id.clone(),
        plan_id: plan.plan_id.clone(),
        expected: plan.expected.clone(),
        candidate_digest: plan.candidate_digest.clone(),
        accepted_impact_digest: plan.impact_digest.clone(),
        expected_control_revision: plan.expected_control_revision,
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "GitHub exact-source real core only; POLARIS_SINGBOX_PATH alone is insufficient"]
async fn real_hot_public_rpc_contract() {
    let _serial = lock_real_core_tests().await;
    let admission = Admission::read();
    let mut evidence = Evidence::new(&admission, "real_hot_public_rpc_contract");
    let network = Network::new();
    let running = Running::start(&admission, network.config(), &mut evidence).await;
    for secret in ["", "incorrect-loopback-credential"] {
        let client = SingBoxApiClient::connect(
            Endpoint::new("127.0.0.1", running.rt.status().clash_api_port),
            secret,
        )
        .await
        .unwrap();
        unauthenticated(client.get_runtime_capabilities().await);
        unauthenticated(client.plan_runtime_config(Default::default()).await);
        unauthenticated(client.apply_runtime_config(Default::default()).await);
        unauthenticated(client.get_runtime_config_status(Default::default()).await);
        unauthenticated(client.retry_runtime_persistence(Default::default()).await);
        unauthenticated(
            client
                .acknowledge_runtime_persistence(Default::default())
                .await,
        );
    }
    let original: Value =
        serde_json::from_slice(&std::fs::read(running.rt.runtime_config_path()).unwrap()).unwrap();
    let request = daemon::PlanRuntimeConfigRequest {
        contract_version: 1,
        expected: running.initial.active.clone(),
        config_content: serde_json::to_string(&original).unwrap(),
        overrides: None,
        expected_control_revision: running.initial.control_revision,
        selector_intents: vec![],
    };
    let no_op = running
        .client
        .plan_runtime_config(request.clone())
        .await
        .unwrap();
    assert!(no_op.applicable && no_op.no_op && no_op.errors.is_empty());
    let no_op_result = running
        .client
        .apply_runtime_config(apply_request(&no_op))
        .await
        .unwrap();
    assert_eq!(no_op_result.active, running.initial.active);
    assert_eq!(no_op_result.before, no_op_result.active);
    assert_eq!(
        no_op_result.control_revision,
        running.initial.control_revision
    );
    let mut candidate = original;
    candidate["log"]["level"] = "debug".into();
    let plan = running
        .client
        .plan_runtime_config(daemon::PlanRuntimeConfigRequest {
            config_content: serde_json::to_string(&candidate).unwrap(),
            ..request.clone()
        })
        .await
        .unwrap();
    assert!(plan.applicable && !plan.no_op && plan.errors.is_empty());
    let apply = apply_request(&plan);
    let op = running
        .client
        .apply_runtime_config(apply.clone())
        .await
        .unwrap();
    assert!(op.durable);
    assert_eq!(op.persistence_mode, "exact_configuration_file");
    let replay = running
        .client
        .apply_runtime_config(apply.clone())
        .await
        .unwrap();
    assert_eq!(replay.active, op.active);
    assert_eq!(replay.transaction_id, op.transaction_id);
    let status = running.status(&op.transaction_id).await;
    assert_eq!(
        status.lookup_state,
        daemon::RuntimeLookupState::Found as i32
    );
    assert_eq!(status.operation.as_ref().unwrap().active, op.active);
    let retry = running
        .client
        .retry_runtime_persistence(daemon::RuntimePersistenceRetryRequest {
            contract_version: 1,
            transaction_id: op.transaction_id.clone(),
            active: op.active.clone(),
            control_revision: op.control_revision,
            persistence_mode: op.persistence_mode.clone(),
        })
        .await
        .unwrap();
    assert!(retry.durable);
    assert_eq!(retry.active, op.active);
    let ack = running
        .client
        .acknowledge_runtime_persistence(daemon::RuntimePersistenceAcknowledgementRequest {
            contract_version: 1,
            transaction_id: op.transaction_id.clone(),
            active: op.active.clone(),
            control_revision: op.control_revision,
            persistence_mode: "app_authoritative_regeneration".into(),
            desired_state_digest: "0".repeat(64),
            desired_state_revision: "fixture-not-an-owner-receipt".into(),
        })
        .await;
    failed_precondition(ack); // Exact-file CLI cannot manufacture regeneration Ack.
    let mut wrong = apply;
    wrong.candidate_digest = "0".repeat(64);
    failed_precondition(running.client.apply_runtime_config(wrong).await);
    let wrong_status = running
        .client
        .get_runtime_config_status(daemon::RuntimeConfigStatusRequest {
            contract_version: 1,
            transaction_id: op.transaction_id.clone(),
            runtime_incarnation: "different-instance".into(),
        })
        .await
        .unwrap();
    assert_eq!(
        wrong_status.lookup_state,
        daemon::RuntimeLookupState::WrongIncarnation as i32
    );
    failed_precondition(running.client.plan_runtime_config(request).await);
    assert_eq!(running.unchanged_process().await.active, op.active);
    evidence.record(json!({"phase":"six-public-rpc","methods":METHODS,"unauthenticatedPerMethod":2,
        "noOpOperation":operation(&no_op_result),"operation":operation(&op),"sameTransactionReplay":true,
        "terminalRetry":operation(&retry),"wrongIncarnationLookup":wrong_status.lookup_state,
        "cliRegenerationAckRejected":true,"positiveRegenerationAck":"requires real libbox handler companion evidence"}));
    running.finish(&admission, &mut evidence).await;
    network.assert_only_fixture_queries();
    drop(network);
    evidence.passed();
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "GitHub exact-source real core only; POLARIS_SINGBOX_PATH alone is insufficient"]
async fn real_hot_dispatcher_rejections_preserve_runtime() {
    let _serial = lock_real_core_tests().await;
    let admission = Admission::read();
    let mut evidence = Evidence::new(
        &admission,
        "real_hot_dispatcher_rejections_preserve_runtime",
    );
    let network = Network::new();
    let running = Running::start(&admission, network.config(), &mut evidence).await;
    let mut old = connect(running.mixed).await;
    exchange(&mut old, b'A', b"rejections-old-before").await;
    assert_eq!(network.a.targets.lock().unwrap()[0].host, "127.0.0.2");
    assert_eq!(network.a.targets.lock().unwrap()[0].port, 80);
    let snapshot = running.rt.current_config_snapshot().unwrap();
    let source = std::fs::read(running.rt.runtime_config_path()).unwrap();
    let mut desired = running.rt.config.current().unwrap();
    let port = loop {
        let p = free_port();
        if p != running.mixed {
            break p;
        }
    };
    desired["mixedPort"] = port.into();
    let desired = save_fixture_config(&running.rt.config, &desired);
    let desired_bytes = std::fs::read(running.rt.config.path()).unwrap();
    let result = running.rt.apply_pending_transaction(None).await;
    let after = running.client.get_runtime_capabilities().await;
    let observation = rejection_observation("structural-rejection", &running, &result, &after);
    evidence.record(observation.clone());
    eprintln!("{observation}");
    assert_eq!(result.status, "restartRequired");
    assert!(result.restart_required && result.restart_confirmation.is_some());
    assert!(result.apply_id.is_none());
    assert!(
        running.rt.pending_force_restart.read().unwrap().is_none(),
        "no implicit restart source"
    );
    assert_eq!(running.rt.current_config_snapshot().unwrap(), snapshot);
    assert_eq!(running.rt.config.current().unwrap(), desired);
    assert_eq!(
        std::fs::read(running.rt.config.path()).unwrap(),
        desired_bytes
    );
    assert_eq!(desired["mixedPort"], port);
    assert_eq!(
        std::fs::read(running.rt.runtime_config_path()).unwrap(),
        source
    );
    assert_eq!(
        running.unchanged_process().await.active,
        running.initial.active
    );
    assert_eq!(
        after.unwrap().control_revision,
        running.initial.control_revision
    );
    exchange(&mut old, b'A', b"rejections-old-after-structural").await;
    // The real store is allowed to reject an invalid reference before dispatcher.
    let mut invalid = snapshot.clone();
    set_fixture_route_target(&mut invalid, "nonexistent-node");
    let invalid_saved = running.rt.config.save_full(&invalid);
    let invalid_stage = if invalid_saved.is_err() {
        evidence.record(json!({"phase":"invalid-reference-store-rejection","saveRejected":true}));
        assert_eq!(running.rt.config.current().unwrap(), desired);
        assert_eq!(
            std::fs::read(running.rt.config.path()).unwrap(),
            desired_bytes
        );
        "store"
    } else {
        let invalid_bytes = std::fs::read(running.rt.config.path()).unwrap();
        let rejected = running.rt.apply_pending_transaction(None).await;
        let after = running.client.get_runtime_capabilities().await;
        let observation =
            rejection_observation("invalid-reference-rejection", &running, &rejected, &after);
        evidence.record(observation.clone());
        eprintln!("{observation}");
        assert_eq!(rejected.status, "rejected", "{observation}");
        assert!(rejected.apply_id.is_none() && rejected.transaction_id.is_none());
        assert!(!rejected.restart_required && rejected.restart_confirmation.is_none());
        assert_eq!(
            serde_json::to_value(&rejected.reasons).unwrap(),
            json!([{"code":"INVALID_ROUTE_TARGET","path":"route.targetServerId"}])
        );
        assert_eq!(
            after.unwrap().control_revision,
            running.initial.control_revision
        );
        assert_eq!(running.rt.config.current().unwrap(), invalid);
        assert_eq!(
            std::fs::read(running.rt.config.path()).unwrap(),
            invalid_bytes
        );
        "dispatcher"
    };
    assert!(running.rt.pending_force_restart.read().unwrap().is_none());
    assert_eq!(
        running.unchanged_process().await.active,
        running.initial.active
    );
    assert_eq!(running.rt.current_config_snapshot().unwrap(), snapshot);
    assert_eq!(
        std::fs::read(running.rt.runtime_config_path()).unwrap(),
        source
    );
    exchange(&mut old, b'A', b"rejections-old-after-invalid").await;
    assert_eq!(network.a.targets.lock().unwrap().len(), 1);
    assert!(network.b.targets.lock().unwrap().is_empty());
    drop(old);
    evidence.record(json!({"phase":"rejected","structuralResult":result,
        "invalidReferenceRejectedAt":invalid_stage,"savedStructuralIntentPreservedBeforeInvalidCase":true,
        "runtimeSourceUnchanged":true,"restartConfirmed":false,"oldTcpStreamPreserved":true}));
    running.finish(&admission, &mut evidence).await;
    network.assert_only_fixture_queries();
    drop(network);
    evidence.passed();
}

// Flush safe, bounded observations before assertions so failures retain the
// actual status and identity without exposing config contents or API secrets.
fn rejection_observation(
    phase: &str,
    running: &Running,
    result: &super::super::runtime_config::RuntimeApplyResult,
    after: &Result<daemon::RuntimeCapabilities, RuntimeConfigError>,
) -> Value {
    let reasons: Vec<_> = result
        .reasons
        .iter()
        .take(8)
        .map(|reason| {
            let reason = serde_json::to_value(reason).unwrap();
            let bounded = |key: &str| {
                reason[key]
                    .as_str()
                    .unwrap_or("")
                    .chars()
                    .take(160)
                    .collect::<String>()
            };
            json!({"code":bounded("code"),"path":bounded("path")})
        })
        .collect();
    json!({"phase":phase,"status":result.status,"reasons":reasons,
        "hasApplyId":result.apply_id.is_some(),"hasTransactionId":result.transaction_id.is_some(),
        "restartRequired":result.restart_required,"hasRestartConfirmation":result.restart_confirmation.is_some(),
        "before":identity(running.initial.active.as_ref()),
        "after":after.as_ref().ok().map(|value| identity(value.active.as_ref())),
        "beforeControlRevision":running.initial.control_revision,
        "afterControlRevision":after.as_ref().ok().map(|value|value.control_revision),
        "capabilitiesReadSucceeded":after.is_ok(),
        "expectedPid":running.pid,"observedPid":running.rt.status().pid})
}
