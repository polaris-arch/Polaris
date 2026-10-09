//! 测试里「落一个文件再把它当程序起」必须走共用办法 —— 全仓源码级门。
//!
//! # 守的是什么
//!
//! 测试进程自己写出一个脚本、加执行位、随即执行它，在多线程的测试进程里会偶发
//! `Text file busy (os error 26)`：本线程持有写句柄的那一小段时间里，别的线程 `fork` 出的
//! 子进程继承了这枚写句柄，在它 `exec` 之前内核拒绝执行该文件。机制与修法见
//! `src-tauri/src/test_support.rs` 的 `write_executable_stand_in`。
//!
//! 这件事没有稳定的运行期表现可以断言（单跑、低负载下那种写法照样绿），所以在源码层面钉：
//! 凡是能让一个刚写出的文件变得**可执行**的写法，只允许出现在共用办法里。
//!
//! # 三条判据，各堵一条路
//!
//! 一个文件要被直接执行，必须有执行位。测试代码拿到「带执行位的新文件」只有三条路：
//!
//! 1. **自己加执行位**（`from_mode(0o755)` / `OpenOptions::mode(0o700)` / `S_IXUSR`）；
//! 2. **覆写一个已经有执行位的文件**（先调共用办法落盘，再 `std::fs::write` 改内容 ——
//!    执行位留着，写句柄又在本进程里开了一次）。这条路在词法上没有稳定的形状，抓的是它
//!    几乎必然带着的东西：一段 shebang 字面量。shebang 只许出现在共用办法的实参里；
//! 3. **复制一个可执行文件**（`std::fs::copy` 连执行位一起带过去）。
//!
//! 每条判据之外的既有命中（目录权限、从不执行的夹具、内存文件）逐文件登记数量与理由；
//! 数量对不上就红，登记项因此不会悄悄失效，新增命中也不会被旧登记盖住。
//!
//! # 取材面
//!
//! `crates/` 与 `src-tauri/` 下全部测试代码：路径里带 `tests/`（或 `*_tests/`）段的 `.rs`
//! （单元测试、集成测试、本目录的源码门），外加各包的 `test_support.rs`。本文件除外 —— 它的自检样本里
//! 全是被禁的写法。生产代码不在面内：那里落脚本、加执行位是正当职责（安装脚本、换核）。
//!
//! # 射程如实登记
//!
//! - 判据 2 抓的是 shebang 字面量。脚本正文完全来自变量、且覆写一个已有执行位的文件，
//!   它看不见。
//! - 执行位经非字面量传入（`from_mode(mode)`）时一律判为命中，不去求值。

/// 共用办法的函数名。
const HELPER: &str = "write_executable_stand_in";

/// 共用办法所在文件（相对仓库根）。
const HELPER_FILE: &str = "src-tauri/src/test_support.rs";

/// 本文件（相对仓库根），不进取材面。
const THIS_FILE: &str = "src-tauri/tests/executable_stand_in_discipline.rs";

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Kind {
    /// 加执行位。
    ExecBit,
    /// 共用办法实参之外的 shebang 字面量。
    Shebang,
    /// `fs::copy`。
    Copy,
}

/// 既有命中的逐文件登记：`(文件, 判据, 数量, 为什么它不是「落替身再执行」)`。
const REGISTERED: [(&str, Kind, usize, &str); 23] = [
    (
        HELPER_FILE,
        Kind::ExecBit,
        1,
        "共用办法自己：按路径 chmod，不开文件",
    ),
    (
        HELPER_FILE,
        Kind::Shebang,
        1,
        "sleeping_probe_script 只产出脚本正文，落盘仍经共用办法",
    ),
    (
        "crates/helper/src/core_install/tests/mod.rs",
        Kind::ExecBit,
        1,
        "假核是安装器的输入字节，只被复制与校验哈希，从不执行",
    ),
    (
        "crates/helper/src/platform/linux/core_installer/tests/mod.rs",
        Kind::ExecBit,
        2,
        "源目录权限与安装器输入字节，从不执行",
    ),
    (
        "crates/helper-client/src/manager/tests/mod.rs",
        Kind::Shebang,
        2,
        "断言生成的安装脚本首行，不落盘执行",
    ),
    (
        "crates/helper/src/core_install/src_dir/tests/mod.rs",
        Kind::ExecBit,
        1,
        "仅设置源目录的遍历权限",
    ),
    (
        "crates/helper/src/core_install/tests/unix_src.rs",
        Kind::ExecBit,
        3,
        "权限拒绝用例经变量设置目录与输入文件模式，从不执行",
    ),
    (
        "crates/helper/src/platform/linux/handler/tests/mod.rs",
        Kind::ExecBit,
        1,
        "仅设置源目录的遍历权限",
    ),
    (
        "crates/helper/src/platform/macos/handler/tests/mod.rs",
        Kind::ExecBit,
        1,
        "源目录权限与假核输入字节，xattr/codesign 经桩执行，从不起核",
    ),
    (
        "crates/helper/src/platform/windows/helper/tests/mod.rs",
        Kind::ExecBit,
        1,
        "unix 宿主上设置源目录与安装器输入字节权限，从不执行",
    ),
    (
        "src-tauri/src/runtime/core_promote/tests/mod.rs",
        Kind::ExecBit,
        1,
        "构造组可写的安装输入验证暂存权限收紧，从不执行",
    ),
    (
        "crates/core-supervisor/src/config_gate/check_custody/tests/mod.rs",
        Kind::ExecBit,
        1,
        "memfd：匿名内存文件不计写句柄，执行不受影响",
    ),
    (
        "crates/core-supervisor/src/exact_spawn/tests/mod.rs",
        Kind::ExecBit,
        2,
        "DirBuilder 建目录的权限",
    ),
    (
        "crates/core-supervisor/src/exact_spawn/tests/mod.rs",
        Kind::Shebang,
        1,
        "喂给 ELF 校验器的非法输入，从不落盘",
    ),
    (
        "src-tauri/src/runtime/proxy/mesh_apply/candidate/check/tests/mod.rs",
        Kind::ExecBit,
        1,
        "memfd：匿名内存文件不计写句柄，执行不受影响",
    ),
    (
        "src-tauri/src/runtime/proxy/mesh_apply/candidate/check/tests/mod.rs",
        Kind::Copy,
        1,
        "副本只被读进 memfd，执行的是 memfd 不是这个路径",
    ),
    (
        "src-tauri/src/runtime/proxy/tests/process_supervision.rs",
        Kind::Copy,
        1,
        "非 unix 腿：那里没有 fork 继承写句柄，unix 腿走共用办法",
    ),
    (
        "src-tauri/src/runtime/proxy/tests/startup.rs",
        Kind::Shebang,
        1,
        "只验路径解析，文件没有执行位、从不执行",
    ),
    (
        "src-tauri/src/runtime/speedtest/tests/mod.rs",
        Kind::Shebang,
        1,
        "起核走假 spawner，文件没有执行位、从不执行",
    ),
    (
        "src-tauri/src/runtime/speedtest/tests/pc_custody.rs",
        Kind::Shebang,
        1,
        "被测行为正是「没有执行位所以起不来」",
    ),
    (
        "src-tauri/src/runtime/env_trust/tests/mod.rs",
        Kind::Shebang,
        1,
        "只验路径信任分类，从不执行",
    ),
    (
        "src-tauri/src/runtime/update_install/tests/mod.rs",
        Kind::Shebang,
        1,
        "断言生产代码生成的脚本文本，不落盘",
    ),
    (
        "src-tauri/src/test_support/tests/mod.rs",
        Kind::Copy,
        2,
        "复制 .rs 源文件做取材面副本",
    ),
];

/// 这些文件里的共用办法调用数下限：防「调用点全没了，三条判据在空集上恒真」。
const MINIMUM_HELPER_CALLS: usize = 8;

fn is_test_code(rel: &str) -> bool {
    rel != THIS_FILE
        && (rel.ends_with("/test_support.rs")
            || rel
                .split('/')
                .any(|part| part == "tests" || part.ends_with("_tests")))
}

/// 取材面：`crates/` 与 `src-tauri/` 下的全部测试代码。
fn scan_surface() -> Vec<(String, String)> {
    let manifest = env!("CARGO_MANIFEST_DIR");
    let mut files = polaris_source_probe::repo_dir_files_in(manifest, "crates", "rs");
    files.extend(polaris_source_probe::repo_dir_files_in(
        manifest,
        "src-tauri",
        "rs",
    ));
    files.retain(|(rel, _)| is_test_code(rel));
    files
}

fn line_of(source: &str, offset: usize) -> usize {
    source[..offset].matches('\n').count() + 1
}

/// `open` 处那个 `(` 到与它配对的 `)` 的字节区间（在剥掉注释与字符串的面上数括号）。
fn paren_span(syntax: &str, open: usize) -> std::ops::Range<usize> {
    let mut depth = 0usize;
    for (offset, byte) in syntax.as_bytes()[open..].iter().enumerate() {
        match byte {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return open..open + offset + 1;
                }
            }
            _ => {}
        }
    }
    panic!("括号未闭合（偏移 {open}）");
}

/// 一个权限实参是否带执行位。不是八进制字面量就按「带」处理（不求值，宁可多红）。
fn grants_exec(argument: &str) -> bool {
    let argument = argument.trim();
    let Some(digits) = argument.strip_prefix("0o") else {
        return true;
    };
    match u32::from_str_radix(&digits.replace('_', ""), 8) {
        Ok(mode) => mode & 0o111 != 0,
        Err(_) => true,
    }
}

/// 一个文件里的全部命中：`(判据, 行号)`。
fn hits(source: &str) -> Vec<(Kind, usize)> {
    let syntax = polaris_source_probe::mask_comments_and_strings(source);
    let literal = polaris_source_probe::mask_comments(source);
    let mut found = Vec::new();

    for needle in ["from_mode(", ".mode("] {
        for (start, _) in syntax.match_indices(needle) {
            let span = paren_span(&syntax, start + needle.len() - 1);
            let argument = &syntax[span.start + 1..span.end - 1];
            // `metadata.permissions().mode()` 是读权限，不是设权限。
            if !argument.trim().is_empty() && grants_exec(argument) {
                found.push((Kind::ExecBit, line_of(source, start)));
            }
        }
    }
    for needle in ["S_IXUSR", "S_IXGRP", "S_IXOTH"] {
        for (start, _) in syntax.match_indices(needle) {
            found.push((Kind::ExecBit, line_of(source, start)));
        }
    }

    let helper_call = format!("{HELPER}(");
    let helper_arguments: Vec<_> = syntax
        .match_indices(&helper_call)
        .map(|(start, _)| paren_span(&syntax, start + helper_call.len() - 1))
        .collect();
    for (start, _) in literal.match_indices("#!/") {
        if !helper_arguments.iter().any(|span| span.contains(&start)) {
            found.push((Kind::Shebang, line_of(source, start)));
        }
    }

    for (start, _) in syntax.match_indices("fs::copy(") {
        found.push((Kind::Copy, line_of(source, start)));
    }
    found.sort();
    found
}

/// 把取材面上的命中与登记表对拍，返回全部不一致。
fn violations(files: &[(String, String)], registered: &[(&str, Kind, usize, &str)]) -> Vec<String> {
    let mut problems = Vec::new();
    let mut actual = std::collections::BTreeMap::<(&str, Kind), Vec<usize>>::new();
    for (rel, source) in files {
        for (kind, line) in hits(source) {
            actual.entry((rel.as_str(), kind)).or_default().push(line);
        }
    }
    for (rel, kind, count, why) in registered {
        let lines = actual.remove(&(*rel, *kind)).unwrap_or_default();
        if lines.len() != *count {
            problems.push(format!(
                "{rel}：{kind:?} 登记 {count} 处（{why}），实际 {} 处，行 {lines:?}",
                lines.len()
            ));
        }
    }
    for ((rel, kind), lines) in actual {
        problems.push(format!("{rel}：未登记的 {kind:?}，行 {lines:?}"));
    }
    problems
}

/// 🔴 测试代码里能造出「带执行位的新文件」的写法，只许出现在共用办法里。
///
/// **变异探针**：把任意一个调用点改回 `std::fs::write` + `set_permissions(.., from_mode(0o755))`
/// ⇒ 本条转红并点名文件与行号；在共用办法落盘之后再 `std::fs::write` 一段带 shebang 的内容
/// ⇒ 同样转红。
#[test]
fn test_code_drops_executables_only_through_the_shared_helper() {
    let files = scan_surface();
    let problems = violations(&files, &REGISTERED);
    assert!(
        problems.is_empty(),
        "测试代码里出现了绕过 `{HELPER}` 的可执行文件落盘写法。写出文件、加执行位、随即执行，\
         会在别的测试线程正好 fork 时撞上 Text file busy（偶发，单跑不现）。\n\
         要执行的替身改走 `{HELPER}`；确实从不执行的，在本门的 REGISTERED 里登记数量与理由：\n  {}",
        problems.join("\n  ")
    );
}

/// 🔴 取材面与正面断言：面上确实有测试代码、确实有调用点，共用办法自己不在本进程里开文件。
///
/// 没有这条，上一条在「取材面选空了」或「调用点全删了」时恒真。
#[test]
fn the_scan_surface_and_the_helper_are_what_the_gate_assumes() {
    let files = scan_surface();
    for required in [
        HELPER_FILE,
        "src-tauri/src/runtime/proxy/tests/mod.rs",
        "crates/core-supervisor/src/config_gate/check_custody/tests/mod.rs",
        "crates/core-supervisor/tests/config_gate_process.rs",
        "src-tauri/tests/real_core_lock_wiring.rs",
    ] {
        assert!(
            files.iter().any(|(rel, _)| rel == required),
            "取材面里没有 `{required}` —— 单元测试 / 集成测试 / 源码门三种位置必须都在面内。\
             实际文件数：{}",
            files.len()
        );
    }
    assert!(
        !files.iter().any(|(rel, _)| rel == THIS_FILE),
        "本文件进了取材面：自检样本会被当成真实命中"
    );
    assert!(
        !files
            .iter()
            .any(|(rel, _)| rel == "src-tauri/src/runtime/update_install.rs"),
        "生产代码进了取材面"
    );

    let call = format!("{HELPER}(");
    let calls: usize = files
        .iter()
        .filter(|(rel, _)| rel != HELPER_FILE)
        .map(|(_, source)| {
            polaris_source_probe::mask_comments_and_strings(source)
                .matches(&call)
                .count()
        })
        .sum();
    assert!(
        calls >= MINIMUM_HELPER_CALLS,
        "`{HELPER}` 的调用点只剩 {calls} 处（下限 {MINIMUM_HELPER_CALLS}）—— 要么替身全没了，\
         要么共用办法改了名而本门的针没跟着改"
    );

    let helper_source = &files
        .iter()
        .find(|(rel, _)| rel == HELPER_FILE)
        .expect("共用办法所在文件")
        .1;
    let syntax = polaris_source_probe::mask_comments_and_strings(helper_source);
    let definition = format!("fn {HELPER}(");
    let starts: Vec<_> = syntax.match_indices(&definition).collect();
    assert_eq!(starts.len(), 1, "共用办法的定义必须唯一");
    let start = starts[0].0;
    let open = start + syntax[start..].find('{').expect("共用办法没有函数体");
    let mut depth = 0usize;
    let mut end = open;
    for (offset, byte) in syntax.as_bytes()[open..].iter().enumerate() {
        match byte {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    end = open + offset;
                    break;
                }
            }
            _ => {}
        }
    }
    let body_syntax = &syntax[open..=end];
    let body = &helper_source[open..=end];
    assert!(
        body.contains("Command::new(\"cp\")") && body_syntax.contains(".status()"),
        "切片自检：共用办法的函数体里应有写入子进程及对它的等待"
    );
    assert!(
        body_syntax.matches("fs::write(").count() == 1 && body.contains("fs::write(&staged,"),
        "共用办法在本进程里只许写暂存文件"
    );
    for forbidden in ["File::create", "OpenOptions", "fs::copy(", "File::options"] {
        assert!(
            !body_syntax.contains(forbidden),
            "共用办法自己在本进程里开了目标文件（`{forbidden}`）—— 写句柄一旦存在于测试进程，\
             别的线程 fork 时就能继承它，整套办法失效"
        );
    }
}

fn sample(source: &str) -> Vec<(String, String)> {
    vec![("crates/x/src/tests/mod.rs".to_owned(), source.to_owned())]
}

/// 🔴 反向对照：历史上真出过事的写法，逐个喂进去都必须红。
#[test]
fn the_gate_rejects_every_historical_shape() {
    let shapes = [
        (
            "原样：写脚本 + from_mode 加执行位",
            r##"fn f(dir: &Path) {
    let binary = dir.join("native-child-fixture.sh");
    std::fs::write(&binary, "#!/bin/sh\nexec sleep 30\n").unwrap();
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
}"##,
            vec![Kind::ExecBit, Kind::Shebang],
        ),
        (
            "覆写已有执行位的探针",
            r##"fn f() {
    let probe = write_sleeping_probe(dir.path(), &witness);
    std::fs::write(&probe, format!("#!/bin/sh\nsleep 10\n")).unwrap();
}"##,
            vec![Kind::Shebang],
        ),
        (
            "创建时就带执行位",
            "fn f() { OpenOptions::new().create(true).write(true).mode(0o755).open(p); }",
            vec![Kind::ExecBit],
        ),
        (
            "只给属主执行位",
            "fn f() { set_permissions(p, Permissions::from_mode(0o100)); }",
            vec![Kind::ExecBit],
        ),
        (
            "执行位经变量传入",
            "fn f(mode: u32) { set_permissions(p, Permissions::from_mode(mode)); }",
            vec![Kind::ExecBit],
        ),
        (
            "nix 的 fchmod",
            "fn f() { fchmod(&file, Mode::S_IRUSR | Mode::S_IXUSR); }",
            vec![Kind::ExecBit],
        ),
        (
            "复制可执行文件",
            "fn f() { std::fs::copy(&core, &foreign).unwrap(); }",
            vec![Kind::Copy],
        ),
    ];
    for (name, source, expected) in shapes {
        let kinds: Vec<Kind> = hits(source).into_iter().map(|(kind, _)| kind).collect();
        assert_eq!(kinds, expected, "样本「{name}」的命中不符");
        assert!(
            !violations(&sample(source), &[]).is_empty(),
            "样本「{name}」没有被判违规"
        );
    }
}

/// 🔴 正向对照：正当写法不得误伤，登记表的数量必须逐一对上。
#[test]
fn the_gate_accepts_legitimate_shapes_and_pins_registered_counts() {
    let clean = [
        r##"fn f() { write_executable_stand_in(&p, "#!/bin/sh\nexit 0\n"); }"##,
        r##"fn f() {
    crate::test_support::write_executable_stand_in(
        &p,
        format!("#!/bin/sh\ncase \" $* \" in *) exit 0;; esac\n{body}\n"),
    );
}"##,
        "fn f() { set_permissions(p, Permissions::from_mode(0o644)); }",
        "fn f() { OpenOptions::new().mode(0o600).open(p); }",
        "fn f() { assert_eq!(meta.permissions().mode() & 0o777, 0o755); }",
        "// std::fs::copy(a, b); from_mode(0o755); \"#!/bin/sh\"\nfn f() {}",
        r#"fn f() { let text = "fs::copy( from_mode(0o755) S_IXUSR"; }"#,
    ];
    for source in clean {
        assert_eq!(hits(source), Vec::new(), "误伤：{source}");
    }

    let one = "fn f() { std::fs::copy(a, b); }";
    let registered = [("crates/x/src/tests/mod.rs", Kind::Copy, 1, "样本")];
    assert!(violations(&sample(one), &registered).is_empty());
    let two = "fn f() { std::fs::copy(a, b); std::fs::copy(c, d); }";
    assert_eq!(
        violations(&sample(two), &registered).len(),
        1,
        "登记 1 处、实际 2 处必须红：新增命中不能被旧登记盖住"
    );
    assert_eq!(
        violations(&sample("fn f() {}"), &registered).len(),
        1,
        "登记 1 处、实际 0 处必须红：登记项不能悄悄失效"
    );

    assert!(is_test_code("crates/a/tests/x.rs"));
    assert!(is_test_code("src-tauri/src/runtime/proxy/tests/mod.rs"));
    assert!(is_test_code(HELPER_FILE));
    assert!(is_test_code("crates/a/src/test_support.rs"));
    assert!(is_test_code("crates/a/src/native/retirement_tests/mod.rs"));
    assert!(!is_test_code("src-tauri/src/runtime/update_install.rs"));
    assert!(!is_test_code(THIS_FILE));
}
