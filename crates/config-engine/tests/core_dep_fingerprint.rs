//! 随包核的 `sing-tun` 依赖必须与发布合同声明的模块版本与本地替换逐字一致。
//!
//! sing-tun 的 DNS 模式默认值（`DNSModeOrDefault()`）是本仓 DNS 分流的前提；换核时它随合同
//! 一并评审。本门只保证「盘上每份核的内嵌 BuildInfo 与合同声明的 sing-tun 行一致」，不验默认值本身。
//!
//! # 射程（自曝，别把绿读大）
//!
//! - 覆盖面 = **盘上 present 的每一份核**；平台名单从 `support/core_locator.rs` 的 `CORE_MATRIX` 派生。
//! - 盘上缺哪个平台就少看哪个平台；一份都没有时跳过，`POLARIS_REQUIRE_KERNEL_GATE=1` 时缺一即红
//!   （打包腿强制生效，`ci_step_still_wired` 守着）。
//! - 🔴 **「跳过」是静默的**：提示语归 libtest 捕获，只在测试失败时才回放。

#[path = "support/core_locator.rs"]
mod core_locator;

use core_locator::{present_cores, repo_root, require_all_present};

/// 缺核信息里用来说明「是哪道门没跑全」的名字。
const GATE_NAME: &str = "依赖指纹门未完整执行";

/// 被钉的依赖模块路径。
const SING_TUN_MODULE: &str = "github.com/sagernet/sing-tun";

// Decode only Go 1.18+ inline BuildInfo, not arbitrary plaintext in the binary.
// Format/framing comes from installed Go debug/buildinfo and cmd/go/modload.
#[derive(Debug)]
struct EmbeddedReplacement {
    path: String,
    version: String,
    h1: String,
}

#[derive(Debug)]
struct EmbeddedModule {
    kind: String,
    version: String,
    replacement: Option<EmbeddedReplacement>,
}

#[derive(Debug)]
struct EmbeddedIdentity {
    modules: std::collections::BTreeMap<String, EmbeddedModule>,
    settings: std::collections::BTreeMap<String, String>,
}

fn inline_string<'a>(bin: &'a [u8], cursor: &mut usize) -> Result<&'a [u8], &'static str> {
    let mut len = 0u64;
    let mut terminated = false;
    for shift in (0..70).step_by(7) {
        let byte = *bin.get(*cursor).ok_or("truncated inline string length")?;
        *cursor += 1;
        if shift == 63 && byte > 1 {
            return Err("overflow inline string length");
        }
        len |= u64::from(byte & 127) << shift;
        if byte < 128 {
            terminated = true;
            break;
        }
    }
    if !terminated || len > 1024 * 1024 {
        return Err("invalid inline string length");
    }
    let end = cursor
        .checked_add(len as usize)
        .ok_or("inline string overflow")?;
    let data = bin.get(*cursor..end).ok_or("truncated inline string")?;
    *cursor = end;
    Ok(data)
}

// Go h1 checksums are canonical base64 encodings of a 32-byte module hash.
// Parsing the shape does not authorize its value: the existing release/candidate
// verifier compares the complete module graph and original bytes to the receipt.
fn module_h1_checksum(sum: &str) -> bool {
    let Some(encoded) = sum.strip_prefix("h1:") else {
        return false;
    };
    let bytes = encoded.as_bytes();
    bytes.len() == 44
        && bytes[43] == b'='
        && bytes[..43]
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(*byte, b'+' | b'/'))
        && b"AEIMQUYcgkosw048".contains(&bytes[42])
}

fn embedded_identity(bin: &[u8]) -> Result<EmbeddedIdentity, &'static str> {
    const MAGIC: &[u8] = b"\xff Go buildinf:";
    const START: &[u8] = &[
        0x30, 0x77, 0xaf, 0x0c, 0x92, 0x74, 0x08, 0x02, 0x41, 0xe1, 0xc1, 0x07, 0xe6, 0xd6, 0x18,
        0xe6,
    ];
    const END: &[u8] = &[
        0xf9, 0x32, 0x43, 0x31, 0x86, 0x18, 0x20, 0x72, 0x00, 0x82, 0x42, 0x10, 0x41, 0x16, 0xd8,
        0xf2,
    ];
    let headers: Vec<_> = bin
        .windows(MAGIC.len())
        .enumerate()
        .filter_map(|(at, value)| {
            let header = bin.get(at..at + 32)?;
            (value == MAGIC
                && at.is_multiple_of(16)
                && header[14] == 8
                && header[15] & 2 == 2
                && header[15] <= 3
                && header[16..].iter().all(|byte| *byte == 0))
            .then_some(at)
        })
        .collect();
    if headers.len() != 1 {
        return Err("missing/duplicate inline Go BuildInfo");
    }
    let mut cursor = headers[0] + 32;
    std::str::from_utf8(inline_string(bin, &mut cursor)?).map_err(|_| "invalid Go version")?;
    let framed = inline_string(bin, &mut cursor)?;
    if framed.len() < 33
        || !framed.starts_with(START)
        || !framed.ends_with(END)
        || framed[framed.len() - 17] != b'\n'
    {
        return Err("invalid module framing");
    }
    let info =
        std::str::from_utf8(&framed[16..framed.len() - 16]).map_err(|_| "invalid module text")?;
    let mut result = EmbeddedIdentity {
        modules: Default::default(),
        settings: Default::default(),
    };
    let mut previous: Option<String> = None;
    for line in info.split_terminator('\n') {
        let parts: Vec<_> = line.split('\t').collect();
        match parts[0] {
            "mod" | "dep" => {
                if !(3..=4).contains(&parts.len())
                    || parts[1].is_empty()
                    || parts[2].is_empty()
                    || result.modules.contains_key(parts[1])
                {
                    return Err("duplicate/malformed module");
                }
                result.modules.insert(
                    parts[1].to_owned(),
                    EmbeddedModule {
                        kind: parts[0].to_owned(),
                        version: parts[2].to_owned(),
                        replacement: None,
                    },
                );
                previous = Some(parts[1].to_owned());
            }
            "=>" => {
                let key = previous.take().ok_or("unbound replacement")?;
                if !(3..=4).contains(&parts.len()) || parts[1].is_empty() {
                    return Err("malformed replacement");
                }
                let sum = parts.get(3).copied().unwrap_or("");
                // fe182's original four-platform BuildInfo contains both local
                // (devel) replacements with an empty sum and a versioned
                // tailscale module replacement with its h1 sum. Keep the forms
                // distinct; never discard a nonempty sum to make a row local.
                if parts[2] == "(devel)" {
                    if !sum.is_empty() {
                        return Err("malformed local replacement");
                    }
                } else if !parts[2].starts_with('v')
                    || parts[2].len() == 1
                    || parts[2].chars().any(char::is_whitespace)
                    || parts[1].starts_with(['.', '/', '\\'])
                    || parts[1].chars().any(char::is_whitespace)
                    || !module_h1_checksum(sum)
                {
                    return Err("malformed versioned replacement");
                }
                let module = result
                    .modules
                    .get_mut(&key)
                    .ok_or("missing replacement module")?;
                if module.replacement.is_some() {
                    return Err("duplicate replacement");
                }
                module.replacement = Some(EmbeddedReplacement {
                    path: parts[1].to_owned(),
                    version: parts[2].to_owned(),
                    h1: sum.to_owned(),
                });
            }
            "build" => {
                if parts.len() != 2 {
                    return Err("malformed build setting");
                }
                let (key, value) = parts[1].split_once('=').ok_or("invalid build setting")?;
                if key.is_empty() || result.settings.contains_key(key) {
                    return Err("duplicate build setting");
                }
                let value = if value.starts_with('"') {
                    serde_json::from_str::<String>(value)
                        .map_err(|_| "invalid quoted build setting")?
                } else {
                    value.to_owned()
                };
                result.settings.insert(key.to_owned(), value);
                previous = None;
            }
            "path" if parts.len() == 2 && !parts[1].is_empty() => {
                previous = None;
            }
            "" if line.is_empty() => {
                previous = None;
            }
            _ => return Err("unsupported BuildInfo row"),
        }
    }
    Ok(result)
}

fn verify_release_dependency(
    bytes: &[u8],
    declared: &serde_json::Value,
) -> Result<(), &'static str> {
    let identity = embedded_identity(bytes)?;
    let actual = identity
        .modules
        .get(SING_TUN_MODULE)
        .ok_or("missing published sing-tun")?;
    if declared["module"] != SING_TUN_MODULE
        || declared["version"].as_str() != Some(actual.version.as_str())
        || actual.kind != "dep"
        || actual.replacement.as_ref().map(|replacement| {
            (
                replacement.path.as_str(),
                replacement.version.as_str(),
                replacement.h1.as_str(),
            )
        }) != Some(("./third_party/sing-tun", "(devel)", ""))
    {
        return Err("published sing-tun dependency/replacement differs");
    }
    Ok(())
}

#[test]
fn bundled_core_sing_tun_matches_the_release_contract() {
    let present = present_cores();
    let complete = require_all_present(&present, GATE_NAME);
    if present.is_empty() {
        eprintln!(
            "⚠ 跳过依赖指纹门：盘上一份随包核都没有（`.gitignore` 的 /resources/*）。\
             跑 `node scripts/fetch-core.mjs` 后本门自动生效；\
             打包腿带 POLARIS_REQUIRE_KERNEL_GATE=1 强制生效。"
        );
        return;
    }
    if !complete {
        eprintln!("⚠ 依赖指纹门只看到部分平台，未覆盖的平台本轮没有被检查。");
    }

    for (core, path) in &present {
        // Emit from Rust: Command::output captures the verifier's own stderr until exit.
        let stage_started = |stage: &str| {
            eprintln!(
                "CORE_DEP_FINGERPRINT platform={} stage={stage} event=start",
                core.key
            );
            std::time::Instant::now()
        };
        let stage_finished = |stage: &str, started: std::time::Instant| {
            eprintln!(
                "CORE_DEP_FINGERPRINT platform={} stage={stage} event=done elapsed_ms={}",
                core.key,
                started.elapsed().as_millis()
            );
        };
        let started = stage_started("read-core");
        let bytes =
            std::fs::read(path).unwrap_or_else(|e| panic!("读不到 {}: {e}", path.display()));
        stage_finished("read-core", started);

        // The existing release verifier checks the exact public contract, App
        // adapter/receipt, original bytes, full module graph and actual BuildID.
        // It observes native Mac signatures on Mac; no foreign core is run.
        let started = stage_started("release-verify");
        let inspected = std::process::Command::new("node")
            .args(["--input-type=module", "-e", "import fs from 'node:fs'; import {loadReleaseContract,verifyPackagedRelease,releaseSingTunDependency} from './scripts/desktop-core/release-consumer.mjs'; const root=process.cwd(), m=JSON.parse(fs.readFileSync('src-tauri/core-manifest.json')), key=process.argv[1], core=process.argv[2], c=loadReleaseContract(root,m); verifyPackagedRelease(root,m,key,core); console.log(JSON.stringify(releaseSingTunDependency(c.platforms[key])));"])
            .arg(core.key)
            .arg(path)
            .current_dir(repo_root())
            .output()
            .expect("Node release dependency verifier required");
        assert!(
            inspected.status.success(),
            "{}: published dependency verification failed: {}",
            core.key,
            String::from_utf8_lossy(&inspected.stderr)
        );
        let declared: serde_json::Value =
            serde_json::from_slice(&inspected.stdout).expect("Release dependency row must be JSON");
        stage_finished("release-verify", started);
        let started = stage_started("embedded-scan");
        verify_release_dependency(&bytes, &declared)
            .unwrap_or_else(|error| panic!("{}: {error}", core.key));
        stage_finished("embedded-scan", started);
    }
}

/// 打包腿上本门必须真的在跑 —— 否则「缺核强制红」那条开关形同虚设。
#[test]
fn ci_step_still_wired() {
    let wf = repo_root().join(".github/workflows/package.yml");
    let raw =
        std::fs::read_to_string(&wf).unwrap_or_else(|e| panic!("读不到 {}: {e}", wf.display()));
    // 匹配 **run 命令本身**而不是裸词：本文件在 package.yml 里还留了一段说明注释，
    // 注释里就写着 `core_dep_fingerprint`。用裸词做判据的话，把整个 step 删掉、注释留下，
    // 本断言照样绿 —— 实测过，这正是「绿没有信息量」的一个新鲜样本。
    assert!(
        raw.contains("--test core_dep_fingerprint"),
        "package.yml 里找不到 `--test core_dep_fingerprint` —— 打包腿没在跑依赖指纹门，\
         缺核时它会静静跳过而没人知道"
    );
}

fn framed_identity_fixture(info: &str) -> Vec<u8> {
    fn append(bytes: &mut Vec<u8>, data: &[u8]) {
        let mut size = data.len();
        while size >= 128 {
            bytes.push((size as u8 & 127) | 128);
            size >>= 7;
        }
        bytes.push(size as u8);
        bytes.extend_from_slice(data);
    }
    let mut bytes = vec![0u8; 32];
    bytes[..14].copy_from_slice(b"\xff Go buildinf:");
    bytes[14] = 8;
    bytes[15] = 2;
    append(&mut bytes, b"go1.25.5");
    let mut framed = vec![
        0x30, 0x77, 0xaf, 0x0c, 0x92, 0x74, 0x08, 0x02, 0x41, 0xe1, 0xc1, 0x07, 0xe6, 0xd6, 0x18,
        0xe6,
    ];
    framed.extend_from_slice(info.as_bytes());
    framed.extend_from_slice(&[
        0xf9, 0x32, 0x43, 0x31, 0x86, 0x18, 0x20, 0x72, 0x00, 0x82, 0x42, 0x10, 0x41, 0x16, 0xd8,
        0xf2,
    ]);
    append(&mut bytes, &framed);
    bytes
}

#[test]
fn source_owned_embedded_identity_rejects_plaintext_and_broken_framing() {
    let info = "path\tgithub.com/sagernet/sing-box/cmd/sing-box\nmod\tgithub.com/sagernet/sing-box\tv0.0.0-fixture\ndep\tgithub.com/sagernet/sing-tun\tv0.9.7-fixture\n=>\t./third_party/sing-tun\t(devel)\n\nbuild\tvcs.modified=false\n";
    let bytes = framed_identity_fixture(info);
    assert_eq!(
        embedded_identity(&bytes).unwrap().modules[SING_TUN_MODULE]
            .replacement
            .as_ref()
            .unwrap()
            .path,
        "./third_party/sing-tun"
    );
    assert!(embedded_identity(info.as_bytes()).is_err());
    assert!(embedded_identity(&bytes[..bytes.len() - 1]).is_err());
    let mut bad = bytes.clone();
    *bad.last_mut().unwrap() ^= 1;
    assert!(embedded_identity(&bad).is_err());
    let mut duplicate = bytes.clone();
    while !duplicate.len().is_multiple_of(16) {
        duplicate.push(0);
    }
    duplicate.extend_from_slice(&bytes);
    assert!(embedded_identity(&duplicate).is_err());
    for extra in [
        "dep\tgithub.com/sagernet/sing-tun\tv9\n",
        "build\tvcs.modified=true\n",
        "=>\t./arbitrary\t(devel)\n",
    ] {
        assert!(embedded_identity(&framed_identity_fixture(&format!("{info}{extra}"))).is_err());
    }
}

#[test]
fn published_dependency_still_rejects_wrong_version_path_and_missing_modinfo() {
    let declared = serde_json::json!({"module": SING_TUN_MODULE, "version": "v0.9.7-fixture"});
    let info = "path\tgithub.com/sagernet/sing-box/cmd/sing-box\nmod\tgithub.com/sagernet/sing-box\tv0.0.0-fixture\ndep\tgithub.com/sagernet/sing-tun\tv0.9.7-fixture\n=>\t./third_party/sing-tun\t(devel)\n\nbuild\tvcs.modified=false\n";
    assert!(verify_release_dependency(&framed_identity_fixture(info), &declared).is_ok());
    for bad in [
        info.replace("v0.9.7-fixture", "v0.9.8-fixture"),
        info.replace("./third_party/sing-tun", "./other"),
        info.replace("(devel)", "v1.0.0"),
        info.replace(
            "github.com/sagernet/sing-tun",
            "github.com/sagernet/sing-tun-extra",
        ),
    ] {
        assert!(verify_release_dependency(&framed_identity_fixture(&bad), &declared).is_err());
    }
    assert!(verify_release_dependency(info.as_bytes(), &declared).is_err());
}

// Exact replacement rows shared by all four fe182 native candidate archives.
// Ordinary dependencies/build settings are omitted only from this parser fixture;
// production still checks the entire receipt-bound graph before this Rust gate.
fn native_candidate_replacement_fixture() -> &'static str {
    concat!(
        "path\tgithub.com/sagernet/sing-box/cmd/sing-box\n",
        "mod\tgithub.com/sagernet/sing-box\tv1.15.0-polaris-alpha.12\t\n",
        "dep\tgithub.com/metacubex/utls\tv1.8.8\n",
        "=>\t./third_party/metacubex-utls\t(devel)\t\n\n",
        "dep\tgithub.com/sagernet/sing-tun\tv0.9.7-0.20261013140216-9ffa5391a2ad\n",
        "=>\t./third_party/sing-tun\t(devel)\t\n\n",
        "dep\tgithub.com/sagernet/tailscale\tv1.102.1-sing-box-1.14-mod.5.0.20260925112514-35e61219dedd\n",
        "=>\tgithub.com/polaris-arch/tailscale\tv1.102.1-sing-box-1.14-mod.5.0.20261016043520-2f8810107046\th1:+7AyRBB7rvyU/QQ7eOQuGvLxmPF3kA7O/r+ar0I1X50=\n\n",
    )
}

#[test]
fn published_dependency_accepts_native_candidate_mixed_replacements() {
    let info = native_candidate_replacement_fixture();
    let bytes = framed_identity_fixture(info);
    let identity = embedded_identity(&bytes).unwrap();
    assert_eq!(identity.modules.len(), 4);
    for (module, path, version, h1) in [
        (
            "github.com/metacubex/utls",
            "./third_party/metacubex-utls",
            "(devel)",
            "",
        ),
        (SING_TUN_MODULE, "./third_party/sing-tun", "(devel)", ""),
        (
            "github.com/sagernet/tailscale",
            "github.com/polaris-arch/tailscale",
            "v1.102.1-sing-box-1.14-mod.5.0.20261016043520-2f8810107046",
            "h1:+7AyRBB7rvyU/QQ7eOQuGvLxmPF3kA7O/r+ar0I1X50=",
        ),
    ] {
        let actual = identity.modules[module].replacement.as_ref().unwrap();
        assert_eq!(actual.path, path);
        assert_eq!(actual.version, version);
        assert_eq!(actual.h1, h1);
    }
    let declared = serde_json::json!({
        "module": SING_TUN_MODULE,
        "version": "v0.9.7-0.20261013140216-9ffa5391a2ad",
        "h1": "",
    });
    verify_release_dependency(&bytes, &declared).unwrap();
    // Local rows without Go's trailing empty sum field remain supported.
    verify_release_dependency(
        &framed_identity_fixture(&info.replace("(devel)\t\n", "(devel)\n")),
        &declared,
    )
    .unwrap();
    for changed in [
        info.replace("9ffa5391a2ad", "000000000000"),
        info.replace("./third_party/sing-tun", "./third_party/sing-tun-extra"),
        info.replace("=>\t./third_party/sing-tun\t(devel)\t\n", ""),
        info.replace(
            "=>\t./third_party/sing-tun\t(devel)\t",
            "=>\tgithub.com/polaris-arch/sing-tun\tv1.0.0\th1:+7AyRBB7rvyU/QQ7eOQuGvLxmPF3kA7O/r+ar0I1X50=",
        ),
    ] {
        // These are parseable graphs, but none can replace the exact local pin.
        assert!(embedded_identity(&framed_identity_fixture(&changed)).is_ok());
        assert!(verify_release_dependency(&framed_identity_fixture(&changed), &declared).is_err());
    }
}

#[test]
fn embedded_replacements_reject_malformed_checksums_and_unbound_rows() {
    let sum = "h1:+7AyRBB7rvyU/QQ7eOQuGvLxmPF3kA7O/r+ar0I1X50=";
    let dep = "dep\tgithub.com/sagernet/tailscale\tv1.0.0\n";
    let replacement = format!("=>\tgithub.com/polaris-arch/tailscale\tv1.0.1\t{sum}\n");
    let info = format!("{dep}{replacement}");
    assert!(embedded_identity(&framed_identity_fixture(&info)).is_ok());
    for changed in [
        info.replace(sum, ""),
        info.replace(&format!("\t{sum}"), ""),
        info.replace(sum, "h1:AAAA="),
        info.replace("h1:", "h2:"),
        info.replace("X50=", "X50!"),
        info.replace("X50=", "X51="), // Noncanonical base64 padding bits.
        info.replace("+7Ay", "_7Ay"),
        info.replace("\tv1.0.1\t", "\t(devel)\t"),
        info.replace("\tv1.0.1\t", "\t\t"),
        info.replace("\tv1.0.1\t", "\tv\t"),
        info.replace("\tv1.0.1\t", "\tv1.0.1 extra\t"),
        info.replace(
            "github.com/polaris-arch/tailscale",
            "./third_party/tailscale",
        ),
        info.replace("github.com/polaris-arch/tailscale", ""),
        info.replace(sum, &format!("{sum}\textra")),
        format!("{info}{replacement}"),
        format!("{dep}\n{replacement}"),
        format!("{dep}build\tvcs.modified=false\n{replacement}"),
        replacement,
    ] {
        assert!(
            embedded_identity(&framed_identity_fixture(&changed)).is_err(),
            "malformed or unbound replacement was accepted: {changed:?}"
        );
    }
    let local = "dep\tgithub.com/sagernet/sing-tun\tv0.9.7-fixture\n=>\t./third_party/sing-tun\t(devel)\t\n";
    for changed in [
        local.replace("(devel)\t", &format!("(devel)\t{sum}")),
        local.replace("(devel)", "v1.0.0"),
        local.replace("(devel)", ""),
    ] {
        assert!(embedded_identity(&framed_identity_fixture(&changed)).is_err());
    }
}
