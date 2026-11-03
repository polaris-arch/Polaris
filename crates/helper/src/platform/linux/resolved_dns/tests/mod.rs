use super::*;
use std::sync::{Arc, Mutex};

#[derive(Clone)]
struct MockRunner {
    state: Arc<Mutex<MockResolvedState>>,
}
struct MockResolvedState {
    exists: bool,
    link: LinkIdentity,
    settings: Settings,
    baseline: Settings,
    calls: Vec<Vec<String>>,
    fail_mutation: Option<String>,
    bad_readback_after_write: bool,
    bad_readback_pending: bool,
    foreign_after_write: bool,
}

impl MockRunner {
    fn healthy() -> Self {
        let baseline = Settings {
            servers: vec![],
            domains: vec![],
            default_route: false,
            dnssec: "no".into(),
            dns_over_tls: "no".into(),
        };
        Self {
            state: Arc::new(Mutex::new(MockResolvedState {
                exists: true,
                link: LinkIdentity {
                    ifindex: 7,
                    sysfs_device: 1,
                    sysfs_inode: 70,
                    netns_inode: 9,
                    tun_flags: 1,
                },
                settings: baseline.clone(),
                baseline,
                calls: vec![],
                fail_mutation: None,
                bad_readback_after_write: false,
                bad_readback_pending: false,
                foreign_after_write: false,
            })),
        }
    }
    fn ops(&self) -> ResolvectlDnsOps {
        ResolvectlDnsOps {
            runner: Box::new(self.clone()),
            ..ResolvectlDnsOps::default()
        }
    }
    fn mutations(&self) -> Vec<Vec<String>> {
        self.state
            .lock()
            .unwrap()
            .calls
            .iter()
            .filter(|args| args.len() > 2 || args[0] == "revert")
            .cloned()
            .collect()
    }
}

impl ResolvectlRunner for MockRunner {
    fn link_exists(&self, _: &str) -> bool {
        self.state.lock().unwrap().exists
    }
    fn link_identity(&self) -> Result<LinkIdentity, String> {
        let state = self.state.lock().unwrap();
        if state.exists {
            Ok(state.link.clone())
        } else {
            Err("managed kernel link missing".into())
        }
    }
    fn link_gone(&self, expected: &LinkIdentity) -> Result<bool, String> {
        let state = self.state.lock().unwrap();
        Ok(!state.exists && expected == &state.link)
    }
    fn pin_link(&self, expected: &LinkIdentity) -> Result<std::fs::File, String> {
        assert_eq!(expected, &self.state.lock().unwrap().link);
        // Unit runner seam only. Production SystemResolvectlRunner opens and
        // verifies the real sysfs object, not this inert fixture file.
        tempfile::tempfile().map_err(|error| error.to_string())
    }
    fn run(&self, args: &[&str]) -> Result<String, String> {
        let mut state = self.state.lock().unwrap();
        state
            .calls
            .push(args.iter().map(|arg| (*arg).into()).collect());
        assert_eq!(args[1], state.link.ifindex.to_string());
        if args.len() == 2 && args[0] != "revert" {
            if state.bad_readback_pending {
                state.bad_readback_pending = false;
                return Ok("Link 99 (foreign): no".into());
            }
            let value = match args[0] {
                "dns" => state.settings.servers.join(" "),
                "domain" => state.settings.domains.join(" "),
                "default-route" => if state.settings.default_route {
                    "yes"
                } else {
                    "no"
                }
                .into(),
                "dnssec" => state.settings.dnssec.clone(),
                "dnsovertls" => state.settings.dns_over_tls.clone(),
                _ => return Err("unexpected query".into()),
            };
            return Ok(format!(
                "Link {} ({}): {value}",
                state.link.ifindex, TUN_INTERFACE_NAME
            ));
        }
        match args[0] {
            "dnssec" => state.settings.dnssec = args[2].into(),
            "dnsovertls" => state.settings.dns_over_tls = args[2].into(),
            "dns" => state.settings.servers = vec![args[2].into()],
            "domain" => state.settings.domains = vec![args[2].into()],
            "default-route" => state.settings.default_route = args[2] == "yes",
            "revert" => state.settings = state.baseline.clone(),
            _ => return Err("unexpected mutation".into()),
        }
        if state.bad_readback_after_write {
            state.bad_readback_after_write = false;
            state.bad_readback_pending = true;
        }
        if state.foreign_after_write {
            state.foreign_after_write = false;
            state.settings.servers = vec!["9.9.9.9".into()];
        }
        if state.fail_mutation.as_deref() == Some(args[0]) {
            return Err(format!("{} failed", args[0]));
        }
        Ok(String::new())
    }
}

#[test]
fn takeover_sets_and_attests_the_managed_link() {
    let runner = MockRunner::healthy();
    let ops = runner.ops();
    ops.takeover(TUN_INTERFACE_NAME, CONTROLLED_DNS_IP).unwrap();
    let mutations = runner.mutations();
    assert_eq!(mutations.len(), 5);
    assert_eq!(mutations[0], ["dnssec", "7", "no"]);
    assert_eq!(mutations[1], ["dnsovertls", "7", "no"]);
    assert_eq!(mutations[2], ["dns", "7", CONTROLLED_DNS_IP]);
    assert_eq!(mutations[3], ["domain", "7", ROUTE_ALL_DOMAIN]);
    assert_eq!(mutations[4], ["default-route", "7", "yes"]);
    let state = runner.state.lock().unwrap();
    assert!(state.settings.controlled());
    let final_reads = &state.calls[state.calls.len() - 5..];
    assert_eq!(final_reads[0], ["dns", "7"]);
    assert_eq!(final_reads[1], ["domain", "7"]);
    assert_eq!(final_reads[2], ["default-route", "7"]);
    assert_eq!(final_reads[3], ["dnssec", "7"]);
    assert_eq!(final_reads[4], ["dnsovertls", "7"]);
    assert!(ops.owned_link.lock().unwrap().is_some());
}

#[test]
fn resolvectl_poll_interval_backs_off_to_the_existing_ceiling() {
    let mut interval = INITIAL_POLL_INTERVAL;
    let mut observed = vec![interval];
    for _ in 0..6 {
        interval = next_poll_interval(interval);
        observed.push(interval);
    }
    assert_eq!(
        observed,
        [1, 2, 4, 8, 16, 20, 20].map(Duration::from_millis).to_vec()
    );
}

#[test]
fn invalid_request_is_rejected_without_commands() {
    let runner = MockRunner::healthy();
    let ops = runner.ops();
    assert!(ops.takeover("eth0", "1.1.1.1").is_err());
    assert!(ops.revert("eth0").is_err());
    assert!(runner.state.lock().unwrap().calls.is_empty());
}

#[test]
fn partial_failure_reverts_the_link() {
    let runner = MockRunner::healthy();
    let ops = runner.ops();
    runner.state.lock().unwrap().fail_mutation = Some("dnsovertls".into());
    assert_eq!(
        ops.takeover(TUN_INTERFACE_NAME, CONTROLLED_DNS_IP)
            .unwrap_err(),
        "dnsovertls failed"
    );
    assert!(ops.owned_link.lock().unwrap().is_some());
    runner.state.lock().unwrap().fail_mutation = None;
    ops.revert(TUN_INTERFACE_NAME).unwrap();
    assert_eq!(runner.mutations().last().unwrap(), &["revert", "7"]);
    {
        let state = runner.state.lock().unwrap();
        assert_eq!(state.settings, state.baseline);
    }
    assert!(ops.owned_link.lock().unwrap().is_none());
}

#[test]
fn missing_link_is_already_reverted_but_cannot_be_taken_over() {
    let runner = MockRunner::healthy();
    runner.state.lock().unwrap().exists = false;
    let ops = runner.ops();
    assert!(ops.takeover(TUN_INTERFACE_NAME, CONTROLLED_DNS_IP).is_err());
    ops.revert(TUN_INTERFACE_NAME).unwrap();
    assert!(runner.state.lock().unwrap().calls.is_empty());
}

#[test]
fn failed_attestation_reverts_the_link() {
    let runner = MockRunner::healthy();
    let ops = runner.ops();
    runner.state.lock().unwrap().bad_readback_after_write = true;
    assert_eq!(
        ops.takeover(TUN_INTERFACE_NAME, CONTROLLED_DNS_IP)
            .unwrap_err(),
        "resolved readback has wrong link identity"
    );
    assert!(ops.owned_link.lock().unwrap().is_some());
    ops.revert(TUN_INTERFACE_NAME).unwrap();
    assert_eq!(runner.mutations().last().unwrap(), &["revert", "7"]);
    assert!(ops.owned_link.lock().unwrap().is_none());
    assert!(runner.state.lock().unwrap().settings.unconfigured());
}

#[test]
fn checked_revert_rejects_successor_and_foreign_values_but_accepts_original_object_absence() {
    for successor in [false, true] {
        let runner = MockRunner::healthy();
        let ops = runner.ops();
        ops.takeover(TUN_INTERFACE_NAME, CONTROLLED_DNS_IP).unwrap();
        if successor {
            runner.state.lock().unwrap().link.sysfs_inode += 1;
        } else {
            runner.state.lock().unwrap().settings.servers = vec!["9.9.9.9".into()];
        }
        let before = runner.mutations().len();
        assert!(ops.revert(TUN_INTERFACE_NAME).is_err());
        assert_eq!(runner.mutations().len(), before);
        assert!(ops.owned_link.lock().unwrap().is_some());
    }
    let runner = MockRunner::healthy();
    let ops = runner.ops();
    ops.takeover(TUN_INTERFACE_NAME, CONTROLLED_DNS_IP).unwrap();
    runner.state.lock().unwrap().exists = false;
    let before = runner.mutations().len();
    ops.revert(TUN_INTERFACE_NAME).unwrap();
    assert_eq!(runner.mutations().len(), before);
    assert!(ops.owned_link.lock().unwrap().is_none());
}

#[test]
fn strict_snapshot_has_no_writes_and_checked_mutation_stops_at_foreign_readback() {
    let runner = MockRunner::healthy();
    let ops = runner.ops();
    let before = ops.snapshot().unwrap();
    assert!(runner.mutations().is_empty());
    assert!(ops.original_settings().is_none());
    runner.state.lock().unwrap().foreign_after_write = true;
    assert!(ops.compare(&before, true).is_err());
    assert_eq!(
        runner.mutations(),
        vec![vec!["dnssec".to_owned(), "7".into(), "no".into()]]
    );
    assert_eq!(
        runner.state.lock().unwrap().settings.servers,
        vec!["9.9.9.9"]
    );
    assert!(ops.revert(TUN_INTERFACE_NAME).is_err());
    assert_eq!(runner.mutations().len(), 1);
    assert!(ops.owned_link.lock().unwrap().is_some());
}

// ── 管道排空必须早于等待（同根因姊妹腿：测速临时核 2026-09-02）──

/// 输出远超管道容量时仍能正常收完，而不是与轮询互等到超时。
///
/// 300 KB ≫ Linux 匿名管道的默认 64 KiB。若 [`run_resolvectl`] 回到「先 `try_wait` 轮询、退出之后才
/// 读管道」的形态，子进程写满管道即阻塞、父进程等它退出，两边互等到 5 秒硬超时 ⇒ 本条拿到的是
/// `Err(... timed out ...)` 而当场红。
///
/// 被测输入用的是 `/bin/sh`，不是 `resolvectl`：本条要压的是「管道排空的形态」，跟系统 DNS 状态无关，
/// 也绝不能去碰它。
#[test]
fn resolvectl_runner_drains_output_larger_than_the_pipe_buffer() {
    let out = run_resolvectl("/bin/sh", &["-c", "yes polaris | head -c 300000"])
        .expect("大输出须正常收完，而不是与 try_wait 轮询互等到超时");
    assert!(
        out.len() >= 299_000,
        "只收到 {} 字节 —— 管道没有被读空",
        out.len()
    );
}

/// 锚点所属块的**闭合**切片：从锚点起，按花括号配平切到与块首 `{` 配对的 `}` 为止。
///
/// # 为什么不能切到文件尾
///
/// 原写法是 `&src[at..]`（一路切到文件末尾）。那样的话「`drain(` 在 `try_wait(` 之前」这条断言随时
/// 可能落在**别的函数**里：`drain(` 既能匹配到本模块下面那个 `fn drain(` 的定义，也能匹配到任何
/// 后来新增的同名调用。今天它仍然红得对，纯粹是因为定义恰好写在被守函数的下面——判据靠的是书写
/// 顺序，把两个函数换个位置就静默失效。
///
/// 形态与仓内既有的配平切块一致（`src-tauri/tests/windows_proxy_registry_write_order.rs` 的
/// `unique_braced_scope`、`helper-client/src/connector/tests/set_read_timeout_gate.rs` 的 impl 块切片）。
/// 在**净化过**的面上数花括号是词法安全的：注释与字符串字面量已整段抹成空格，会骗过计数器的那几类
/// 花括号在这个面上根本不存在。
///
/// # Panics
///
/// 锚点不存在 / 不唯一 / 块不闭合，一律 panic —— 门失去判据时必须转红，不能静默退化成「切了个空片、
/// 断言恒真」。
fn braced_block<'a>(masked: &'a str, anchor: &str) -> &'a str {
    let hits = masked.matches(anchor).count();
    assert_eq!(
        hits, 1,
        "锚点 `{anchor}` 命中 {hits} 次（应为 1）。为 0 = 锚点消失，门已失去判据，不是「通过」；\
         >1 = 切片指向哪一处取决于书写顺序"
    );
    let start = masked.find(anchor).expect("上面已断言恰好一处");
    let open = masked[start..]
        .find('{')
        .map(|off| start + off)
        .unwrap_or_else(|| panic!("锚点 `{anchor}` 之后没有 `{{` —— 它不是一个块的签名"));
    let mut depth = 0usize;
    for (offset, byte) in masked.as_bytes()[open..].iter().enumerate() {
        match byte {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return &masked[start..=open + offset];
                }
            }
            _ => {}
        }
    }
    panic!("锚点 `{anchor}` 的块没有闭合 —— 花括号配平走到文件尾")
}

/// 源码级：排空写在等待之前。
///
/// 上一条行为门只有在输出真的越过管道容量时才红。把排空挪回 `try_wait` 之后、而调用的命令输出仍然
/// 很小的改动它抓不住——那恰恰是本腿修复前的样子（靠「每次只回一行」侥幸不发作）。故再钉一条源码级
/// 次序断言，让形态本身不能退回去。全仓同类站点由 `src-tauri/tests/subprocess_stdio_discipline.rs`
/// 统一守，这一条让 helper crate 单独跑测时也带着判据。
///
/// 取材先过 `mask_comments_and_strings`：本文件与被测文件的注释里都写着 `try_wait` 与 `drain`，
/// 不剥的话次序断言会跑在注释上。切片按花括号配平封口（见 [`braced_block`]），不切到文件尾。
#[test]
fn draining_is_wired_before_waiting_in_the_source() {
    let src = polaris_source_probe::mask_comments_and_strings(
        &polaris_source_probe::crate_source!("platform/linux/resolved_dns.rs"),
    );
    let body = braced_block(&src, "fn run_resolvectl(");
    // 切片封口的正向对照：`fn drain(` 的定义写在被守函数**下面**，闭合切片里不该看得见它。
    // 看得见 = 切到文件尾了，那么下面那条次序断言随时可能落在别的函数上（改个书写顺序就静默失效）。
    assert!(
        !body.contains("fn drain("),
        "切片没有封口 —— 它一路切到了文件尾，次序断言可能跑在别的函数上"
    );
    let drain_at = body
        .find("drain(")
        .expect("排空形态 `drain(` 消失 —— 管道又变回没人读了");
    let wait_at = body
        .find("try_wait(")
        .expect("等待形态 `try_wait(` 消失 —— 次序断言已失去对象");
    assert!(
        drain_at < wait_at,
        "排空（块内偏移 {drain_at}）晚于等待（块内偏移 {wait_at}）—— 这是死锁形态：\
         子进程写满管道后等父进程读，父进程等子进程退出"
    );
}

/// 特权二进制里的不变式：`run_resolvectl` 的**生产**调用点恰好一个，且只喂 [`RESOLVECTL_BIN`]。
///
/// # 为什么这条要落成代码
///
/// 把可执行文件参数化是为了让回归测试能喂 shim（见 `run_resolvectl` 的文档）。代价是这个函数从
/// 「只会跑 `/usr/bin/resolvectl`」变成「能跑任何东西」，而它跑在 **root helper** 里。本批把
/// 「生产只有一个调用点、只喂 `RESOLVECTL_BIN`」写在了注释里——**注释对执行没有任何强制力**：
/// 下一个人在别处加一句 `run_resolvectl(&user_supplied, …)`，注释不会红，编译也不会红。
/// 本仓的规矩是判据由代码持有，故落成这一条。
///
/// # 判据形态
///
/// 在**生产**源码的净化面上，`run_resolvectl(` 恰好命中 2 次：一次是 `fn run_resolvectl(` 的定义，
/// 一次是唯一的生产调用点，且那一处的第一个实参必须逐字是 `RESOLVECTL_BIN`。测试文件不在这个面上
/// （`crate_source!` 取的是 `src/platform/linux/resolved_dns.rs` 本身），故喂 `/bin/sh` 的那两条
/// 回归测试不参与计数——它们本来就该能喂别的程序。
#[test]
fn the_only_production_call_site_feeds_the_pinned_binary() {
    const FORM: &str = "run_resolvectl(";
    let src = polaris_source_probe::mask_comments_and_strings(
        &polaris_source_probe::crate_source!("platform/linux/resolved_dns.rs"),
    );
    let sites: Vec<usize> = {
        let mut out = Vec::new();
        let mut from = 0usize;
        while let Some(at) = src[from..].find(FORM) {
            out.push(from + at);
            from += at + FORM.len();
        }
        out
    };
    assert_eq!(
        sites.len(),
        2,
        "生产源码里 `{FORM}` 命中 {} 处（应为 2：定义 + 唯一生产调用点）。\
         为 0/1 = 判据失去对象；>2 = 这个能跑任意可执行文件的函数在 root helper 里多了一个调用点，\
         而它多出来的每一处都得自己证明喂进去的是什么",
        sites.len()
    );

    let definition = sites
        .iter()
        .filter(|at| src[..**at].ends_with("fn "))
        .count();
    assert_eq!(
        definition, 1,
        "两处命中里 `fn {FORM}` 的定义有 {definition} 处（应为 1）—— 取材面或判据坏了"
    );

    let call = sites
        .iter()
        .find(|at| !src[..**at].ends_with("fn "))
        .expect("找不到非定义的那一处 —— 生产调用点消失了");
    assert!(
        src[*call..].starts_with("run_resolvectl(RESOLVECTL_BIN"),
        "唯一的生产调用点没有逐字喂 `RESOLVECTL_BIN`，而是：{}",
        &src[*call..(*call + 60).min(src.len())]
    );
}

/// [`join_within`] 必须**有界**：读线程迟迟不结束时，调用方不能被它挂住。
///
/// 这条钉的是本批新引入的那一格：把「先读后等」搬进来之后，两个读线程持有管道读端，超时路径上
/// 撒手不管就是每次泄漏两个线程与两个 fd，而成功路径上的无界 `join()` 在子进程留下继承写端的后代时
/// 会永久挂住 root helper 的一条请求路径。被测对象是收口本身，不起任何子进程。
#[test]
fn join_within_gives_up_instead_of_hanging_on_a_slow_reader() {
    // 正向对照：正常结束的线程必须原样把内容交回来，否则「有界」可能只是「永远拿不到东西」。
    let quick = std::thread::spawn(|| "polaris".to_owned());
    assert_eq!(
        join_within(quick, Duration::from_secs(1)),
        "polaris",
        "有界 join 把正常结束的线程的结果丢了"
    );

    let started = Instant::now();
    let slow = std::thread::spawn(|| {
        std::thread::sleep(Duration::from_secs(2));
        "polaris".to_owned()
    });
    let got = join_within(slow, Duration::from_millis(50));
    let waited = started.elapsed();
    assert_eq!(got, "", "放弃 join 之后应当返回空串");
    assert!(
        waited < Duration::from_secs(1),
        "等了 {waited:?} —— join 没有在预算内收口，调用方仍会被读线程挂住"
    );
}
