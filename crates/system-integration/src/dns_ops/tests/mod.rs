use super::*;
use crate::proxy::proxy_tests_helpers::MemFs;
use std::cell::RefCell;

struct MockDnsOps {
    targets: Vec<String>,
    dns_state: RefCell<BTreeMap<String, Vec<String>>>,
    apply_calls: RefCell<Vec<(String, Vec<String>)>>,
    apply_fail_targets: Vec<String>,
    read_fail_targets: Vec<String>,
    restore_fail_targets: Vec<String>,
    list_fails: bool,
    effective: RefCell<Vec<String>>,
    /// 模拟平台是否接管（mac=true / win·linux=false）。
    takeover: bool,
    suppress_apply: bool,
    foreign_after_apply: Option<String>,
    /// 仅对**该 target** 生效的「瞬时失败」计数器：每次对它的 `apply_dns` 调用消耗 1，耗尽后
    /// 转正常（成功）。与 `apply_fail_targets` 的**永久**失败区分——用于验证重试：先失败 N 次
    /// 再成功，且不误伤同一轮里的其它 target（同构 `proxy_ops.rs` `FlakyRunner` 设计）。
    transient_fail_target: Option<String>,
    transient_fail_count: RefCell<u32>,
    /// 瞬时失败时返回的错误消息（决定 `dns_set_should_retry` 判「重试」还是「放弃」）。
    transient_fail_msg: String,
}

/// `takeover: true` = mac 语义（默认）；win/linux 腿的测试显式置 false。
impl Default for MockDnsOps {
    fn default() -> Self {
        Self {
            targets: Vec::new(),
            dns_state: RefCell::new(BTreeMap::new()),
            apply_calls: RefCell::new(Vec::new()),
            apply_fail_targets: Vec::new(),
            read_fail_targets: Vec::new(),
            restore_fail_targets: Vec::new(),
            list_fails: false,
            effective: RefCell::new(vec!["192.168.1.1".into()]),
            takeover: true,
            suppress_apply: false,
            foreign_after_apply: None,
            transient_fail_target: None,
            transient_fail_count: RefCell::new(0),
            transient_fail_msg: String::new(),
        }
    }
}

impl SystemDnsOps for MockDnsOps {
    fn takeover_supported(&self) -> bool {
        self.takeover
    }
    fn list_targets(&self) -> Result<Vec<String>, crate::error::SystemIntegrationError> {
        if self.list_fails {
            return Err(crate::error::SystemIntegrationError::dns("list failed"));
        }
        Ok(self.targets.clone())
    }
    fn read_dns(&self, target: &str) -> Result<Vec<String>, crate::error::SystemIntegrationError> {
        if self.read_fail_targets.iter().any(|name| name == target) {
            return Err(crate::error::SystemIntegrationError::dns(
                "read unavailable",
            ));
        }
        Ok(self
            .dns_state
            .borrow()
            .get(target)
            .cloned()
            .unwrap_or_default())
    }
    fn apply_dns(
        &self,
        target: &str,
        ips: &[String],
    ) -> Result<(), crate::error::SystemIntegrationError> {
        self.apply_calls
            .borrow_mut()
            .push((target.to_string(), ips.to_vec()));
        if self.transient_fail_target.as_deref() == Some(target) {
            let mut rem = self.transient_fail_count.borrow_mut();
            if *rem > 0 {
                *rem -= 1;
                return Err(crate::error::SystemIntegrationError::dns(
                    self.transient_fail_msg.clone(),
                ));
            }
        }
        if self.apply_fail_targets.iter().any(|t| t == target) {
            return Err(crate::error::SystemIntegrationError::dns("apply failed"));
        }
        if self.restore_fail_targets.iter().any(|name| name == target)
            && ips != [controlled_tun_dns_ip().to_string()]
        {
            return Err(crate::error::SystemIntegrationError::dns(
                "restore unavailable",
            ));
        }
        if self.suppress_apply {
            return Ok(());
        }
        if self.foreign_after_apply.as_deref() == Some(target)
            && ips == [controlled_tun_dns_ip().to_string()]
        {
            self.dns_state
                .borrow_mut()
                .insert(target.to_owned(), vec!["9.9.9.9".into()]);
            return Err(SystemIntegrationError::dns("external state after write"));
        }
        // 模拟真实写入状态。
        self.dns_state
            .borrow_mut()
            .insert(target.to_string(), ips.to_vec());
        Ok(())
    }
    fn read_effective_resolvers(
        &self,
    ) -> Result<Vec<String>, crate::error::SystemIntegrationError> {
        Ok(self.effective.borrow().clone())
    }
}

fn mem_dns_marker() -> DnsMarker<MemFs> {
    DnsMarker::new(MemFs::new(), "/dns-marker.json")
}

fn controller(ops: MockDnsOps) -> SystemDnsController<MockDnsOps, MemFs> {
    SystemDnsController::new(ops, mem_dns_marker())
}

#[test]
fn set_dns_takes_over_writes_marker_applies_controlled_ip() {
    let ops = MockDnsOps {
        targets: vec!["Wi-Fi".into()],
        dns_state: RefCell::new({
            let mut m = BTreeMap::new();
            m.insert("Wi-Fi".into(), vec!["192.168.1.1".to_string()]);
            m
        }),
        ..Default::default()
    };
    let mut c = controller(ops);

    c.set_dns();
    assert!(c.has_marker());
    // apply 受控 IP。
    let applied = c.ops.apply_calls.borrow();
    assert!(applied
        .iter()
        .any(|(t, ips)| t == "Wi-Fi" && ips == &vec!["8.8.8.8".to_string()]));
    // marker.original 记录了接管前的真实 LAN（192.168.1.1）。
    let marker = c.marker.read().unwrap();
    assert_eq!(
        marker.original.get("Wi-Fi").unwrap(),
        &vec!["192.168.1.1".to_string()]
    );
}

#[test]
fn set_dns_noop_when_no_targets() {
    let mut c = controller(MockDnsOps::default());
    c.set_dns();
    assert!(!c.has_marker());
    assert!(c.ops.apply_calls.borrow().is_empty());
}

#[test]
fn set_dns_noop_when_controlled_ip_invalid() {
    // CONTROLLED_TUN_DNS_IP=8.8.8.8 不在 bootstrap-direct → 合法，set 正常。
    // 此测试验证守卫路径：构造一个 controlled_ip 非法的 marker 不易（常量），改为验证合法时不被拦截。
    let ops = MockDnsOps {
        targets: vec!["Wi-Fi".into()],
        ..Default::default()
    };
    let mut c = controller(ops);
    c.set_dns();
    assert!(c.has_marker(), "8.8.8.8 合法，应正常接管");
}

#[test]
fn set_dns_rolls_back_on_apply_failure() {
    // "apply failed"（`apply_fail_targets` 永久失败腿）非权限类 → 可重试，耗尽 maxRetries=2
    // （共 3 次 attempt）后仍失败 → 兜底还原（还原本身也因同一目标永久失败而失败）→ 补清 marker。
    let ops = MockDnsOps {
        targets: vec!["Wi-Fi".into()],
        dns_state: RefCell::new({
            let mut m = BTreeMap::new();
            m.insert("Wi-Fi".into(), vec!["192.168.1.1".to_string()]);
            m
        }),
        apply_fail_targets: vec!["Wi-Fi".into()],
        ..Default::default()
    };
    let mut c = controller(ops).with_noop_sleeper();
    c.set_dns();
    // Both takeover and rollback failed: the durable snapshot remains recoverable.
    assert!(
        c.has_marker(),
        "failed rollback must retain recovery intent"
    );
    assert_eq!(
        c.ops.apply_calls.borrow().len(),
        4,
        "重试 3 次（maxRetries=2）+ 回滚还原 1 次（不重试）"
    );
}

// ══════════ set_dns 重试（补齐与系统代理侧 `retry_op` 的不对称；仅 set_dns 一处）══════════

#[test]
fn dns_set_should_retry_aborts_on_permission_or_not_authorized() {
    // 纯谓词断言（同构 `proxy_ops.rs` 的 `mac_should_retry_aborts_on_permission_or_not_authorized`）。
    let ret = |m: &str| dns_set_should_retry(&crate::error::SystemIntegrationError::dns(m));
    assert!(
        !ret("networksetup: permission denied"),
        "permission → 不重试"
    );
    assert!(
        !ret("Error: not authorized to change"),
        "not authorized → 不重试"
    );
    assert!(ret("networksetup: temporarily unavailable"), "瞬时 → 重试");
    // 词表补齐后的关键形态：macOS `networksetup` 真实权限文案不含上游那两词。
    // 变异锁：把 `dns_set_should_retry` 改回手抄的 `permission || not authorized` → 本断言转红
    // （= 一次必败的权限错误会持 `dns_controller` 锁多跑 2 次重试 + 1.5s 退避）。
    assert!(
        !ret("networksetup: requires admin privileges to change DNS"),
        "requires admin privileges → 不重试（此前被误判成瞬时）"
    );
    assert!(
        !ret("setting DNS: Operation not permitted"),
        "EPERM → 不重试"
    );
}

/// 端到端形态（不只是谓词）：权限错误必须**只跑一次** apply，绝不重试。
///
/// 变异锁：这条与上面的纯谓词断言分工不同 —— 谓词断言证明「判据认得这句话」，本用例证明
/// 「判据真的接在 `DNS_SET_RETRY.should_retry` 上」（换回 `|_| true` 时纯谓词断言仍绿）。
#[test]
fn set_dns_admin_privileges_error_aborts_without_retry() {
    let ops = MockDnsOps {
        targets: vec!["Wi-Fi".into()],
        dns_state: RefCell::new({
            let mut m = BTreeMap::new();
            m.insert("Wi-Fi".into(), vec!["192.168.1.1".to_string()]);
            m
        }),
        transient_fail_target: Some("Wi-Fi".into()),
        transient_fail_count: RefCell::new(99),
        transient_fail_msg: "networksetup: requires admin privileges".into(),
        ..Default::default()
    };
    let mut c = controller(ops).with_noop_sleeper();
    c.set_dns();
    assert_eq!(
        c.ops.apply_calls.borrow().len(),
        2,
        "权限失败 → 1 次 apply + 1 次兜底还原（**不含**任何重试；\
             判据漏词时这里会变成 3+1=4，正是那 1.5s 白占锁的来源）"
    );
}

#[test]
fn set_dns_retries_transient_failure_then_succeeds() {
    // ① 瞬态失败一次后成功 → 整体成功且不回滚。
    let ops = MockDnsOps {
        targets: vec!["Wi-Fi".into()],
        dns_state: RefCell::new({
            let mut m = BTreeMap::new();
            m.insert("Wi-Fi".into(), vec!["192.168.1.1".to_string()]);
            m
        }),
        transient_fail_target: Some("Wi-Fi".into()),
        transient_fail_count: RefCell::new(1),
        transient_fail_msg: "networksetup: temporarily unavailable".into(),
        ..Default::default()
    };
    let mut c = controller(ops).with_noop_sleeper();
    c.set_dns();
    assert!(c.has_marker(), "重试后应成功接管，不应回滚");
    assert_eq!(
        c.ops.apply_calls.borrow().len(),
        2,
        "首次失败 + 1 次重试成功 = 2 次 apply"
    );
    assert_eq!(
        c.ops.dns_state.borrow().get("Wi-Fi").unwrap(),
        &vec!["8.8.8.8".to_string()],
        "重试成功后应已 apply 受控 IP"
    );
}

#[test]
fn set_dns_permission_error_aborts_without_retry() {
    // ② 权限类错误 → 立即失败不重试（断言尝试次数）。
    let ops = MockDnsOps {
        targets: vec!["Wi-Fi".into()],
        dns_state: RefCell::new({
            let mut m = BTreeMap::new();
            m.insert("Wi-Fi".into(), vec!["192.168.1.1".to_string()]);
            m
        }),
        // 权限错误不会自愈 → 计数给足（99），验证「不重试」而非「凑巧次数够用」。
        transient_fail_target: Some("Wi-Fi".into()),
        transient_fail_count: RefCell::new(99),
        transient_fail_msg: "networksetup: permission denied".into(),
        ..Default::default()
    };
    let mut c = controller(ops).with_noop_sleeper();
    c.set_dns();
    assert!(
        c.has_marker(),
        "接管及回滚都遭权限拒绝，必须保留恢复 intent"
    );
    assert_eq!(
        c.ops.dns_state.borrow()["Wi-Fi"],
        vec!["192.168.1.1".to_string()]
    );
    assert_eq!(
        c.ops.apply_calls.borrow().len(),
        2,
        "重试阶段仅 1 次尝试（不重试）+ 回滚还原 1 次（本身也不重试）= 2 次；\
             若误重试则耗尽 3 次 attempt + 回滚 ≥ 4 次"
    );
    *c.ops.transient_fail_count.borrow_mut() = 0;
    c.restore_dns();
    assert!(!c.has_marker(), "权限恢复后的真实还原成功才清恢复 intent");
    assert_eq!(c.ops.apply_calls.borrow().len(), 3);
    assert_eq!(
        c.ops.dns_state.borrow()["Wi-Fi"],
        vec!["192.168.1.1".to_string()]
    );
}

#[test]
fn set_dns_retry_exhausted_still_rolls_back_partial_takeover() {
    // ③ 重试耗尽 → 回滚半接管路径仍正确：Wi-Fi 每次 attempt 都成功切到受控 IP（半接管），
    // Ethernet 持续瞬时失败拖垮整轮 → 3 次 attempt（maxRetries=2）耗尽后放弃 → 兜底还原，
    // 须把「已半接管」的 Wi-Fi 也一并撤回原始值（不是只管失败的那个 target）。
    let ops = MockDnsOps {
        targets: vec!["Wi-Fi".into(), "Ethernet".into()],
        dns_state: RefCell::new({
            let mut m = BTreeMap::new();
            m.insert("Wi-Fi".into(), vec!["192.168.1.1".to_string()]);
            m.insert("Ethernet".into(), vec!["10.0.0.1".to_string()]);
            m
        }),
        // 恰好覆盖 3 次 attempt（耗尽后回滚阶段的 Ethernet apply 自然转为成功）。
        transient_fail_target: Some("Ethernet".into()),
        transient_fail_count: RefCell::new(3),
        transient_fail_msg: "networksetup: temporarily unavailable".into(),
        ..Default::default()
    };
    let mut c = controller(ops).with_noop_sleeper();
    c.set_dns();
    assert!(!c.has_marker(), "重试耗尽应回滚并清 marker");
    assert_eq!(
        c.ops.dns_state.borrow().get("Wi-Fi").unwrap(),
        &vec!["192.168.1.1".to_string()],
        "半接管的 Wi-Fi 必须被回滚为原始值，不能残留受控 IP"
    );
    assert_eq!(
        c.ops.dns_state.borrow().get("Ethernet").unwrap(),
        &vec!["10.0.0.1".to_string()],
        "Ethernet 还原为原始值"
    );
    assert_eq!(
        c.ops.apply_calls.borrow().len(),
        8,
        "3 次 attempt × 2 target + 回滚 2 target = 8 次 apply"
    );
}

#[test]
fn restore_dns_restores_original_and_clears_marker() {
    let ops = MockDnsOps {
        targets: vec!["Wi-Fi".into()],
        dns_state: RefCell::new({
            let mut m = BTreeMap::new();
            m.insert("Wi-Fi".into(), vec!["192.168.1.1".to_string()]);
            m
        }),
        ..Default::default()
    };
    let mut c = controller(ops);
    c.set_dns();
    assert!(c.has_marker());

    c.restore_dns();
    // 还原 → apply 原始 LAN IP + 清 marker。
    assert!(!c.has_marker());
    let applied = c.ops.apply_calls.borrow();
    assert!(applied
        .iter()
        .any(|(t, ips)| t == "Wi-Fi" && ips == &vec!["192.168.1.1".to_string()]));
}

#[test]
fn restore_dns_noop_when_no_marker_and_no_original() {
    let mut c = controller(MockDnsOps::default());
    c.restore_dns();
    assert!(c.ops.apply_calls.borrow().is_empty());
}

#[test]
fn reconcile_dns_idempotent_when_all_controlled() {
    // 所有服务已受控 → 幂等 no-op（不写 marker、不动系统）。
    let ops = MockDnsOps {
        targets: vec!["Wi-Fi".into()],
        dns_state: RefCell::new({
            let mut m = BTreeMap::new();
            m.insert("Wi-Fi".into(), vec!["8.8.8.8".to_string()]);
            m
        }),
        ..Default::default()
    };
    let mut c = controller(ops);
    // 先 set 建立 marker。
    c.set_dns();
    let calls_before = c.ops.apply_calls.borrow().len();

    c.reconcile_dns();
    // 全部已受控 → 不再 apply。
    assert_eq!(c.ops.apply_calls.borrow().len(), calls_before);
}

#[test]
fn reconcile_dns_takes_over_new_uncontrolled_service() {
    // Wi-Fi 已接管（marker 在），Ethernet 新出现且未受控 → reconcile 接管 Ethernet。
    let ops = MockDnsOps {
        targets: vec!["Wi-Fi".into(), "Ethernet".into()],
        dns_state: RefCell::new({
            let mut m = BTreeMap::new();
            m.insert("Wi-Fi".into(), vec!["8.8.8.8".to_string()]); // 已受控
            m.insert("Ethernet".into(), vec!["10.0.0.1".to_string()]); // 未受控（真实 LAN）
            m
        }),
        ..Default::default()
    };
    let mut c = controller(ops);

    // 模拟 set 已完成（marker 在，Wi-Fi=8.8.8.8 是我们设的，原始=用户真值）。
    // 直接写 marker 模拟接管态：
    let mut original = BTreeMap::new();
    original.insert("Wi-Fi".into(), vec!["192.168.1.1".to_string()]);
    c.marker.write(&original);
    c.original = Some(original);

    c.reconcile_dns();
    // Ethernet 被 apply 受控 IP。
    let applied = c.ops.apply_calls.borrow();
    assert!(applied
        .iter()
        .any(|(t, ips)| t == "Ethernet" && ips == &vec!["8.8.8.8".to_string()]));
    // Wi-Fi 不重复 apply（已受控跳过）。
    let wifi_applies = applied.iter().filter(|(t, _)| t == "Wi-Fi").count();
    assert_eq!(wifi_applies, 0);
    // marker.original 合并了 Ethernet 的真实原始（10.0.0.1）。
    let marker = c.marker.read().unwrap();
    assert_eq!(
        marker.original.get("Ethernet").unwrap(),
        &vec!["10.0.0.1".to_string()]
    );
    assert_eq!(
        marker.original.get("Wi-Fi").unwrap(),
        &vec!["192.168.1.1".to_string()]
    );
}

#[test]
fn reconcile_dns_noop_when_no_marker() {
    // marker 不在 = 接管未激活 → 绝不擅自接管。
    let ops = MockDnsOps {
        targets: vec!["Wi-Fi".into()],
        ..Default::default()
    };
    let mut c = controller(ops);
    c.reconcile_dns();
    assert!(c.ops.apply_calls.borrow().is_empty());
    assert!(!c.has_marker());
}

#[test]
fn get_lan_resolver_uses_marker_original_when_active() {
    let ops = MockDnsOps::default();
    let c = controller(ops);
    let mut original = BTreeMap::new();
    original.insert("Wi-Fi".into(), vec!["192.168.1.1".to_string()]);
    c.marker.write(&original);
    assert_eq!(
        c.get_lan_resolver_for_dns(),
        Some("192.168.1.1".to_string())
    );
}

#[test]
fn get_lan_resolver_reads_effective_when_no_marker() {
    let ops = MockDnsOps::default();
    let c = controller(ops);
    // 无 marker → read_effective_resolvers → 192.168.1.1（私网）。
    assert_eq!(
        c.get_lan_resolver_for_dns(),
        Some("192.168.1.1".to_string())
    );
}

// ══════════ takeover_supported 门（win/linux 不接管）══════════
//
// 这道门守的是上游 2026-06-17 真机实证修掉的 bug：win 上 `netsh set` 必 ACCESS DENIED，
// 而 set_dns 是「先写 marker 再 apply」→ marker 卡死 → 每次启动反复空跑还原刷错误日志。

#[test]
fn set_dns_writes_no_marker_when_platform_does_not_take_over() {
    // **关键**：不接管的平台连 marker 都不能写（写了就卡死）。
    let ops = MockDnsOps {
        targets: vec!["Wi-Fi".into()],
        takeover: false,
        ..Default::default()
    };
    let mut c = controller(ops);
    c.set_dns();
    assert!(!c.has_marker(), "不接管的平台绝不能写 marker（否则卡死）");
    assert!(c.ops.apply_calls.borrow().is_empty(), "不接管 → 不得 apply");
}

#[test]
fn restore_dns_clears_stuck_marker_on_non_takeover_platform() {
    // 上游 `WindowsSystemDns.restoreDns` = clearMarker()：清历史版本残留的 stuck marker，
    // 否则 has_marker 恒 true → 每个终态点/启动 recovery 反复空跑还原。
    let ops = MockDnsOps {
        targets: vec!["Wi-Fi".into()],
        takeover: false,
        ..Default::default()
    };
    let mut c = controller(ops);
    // 模拟历史遗留的 stuck marker（旧版本 netsh 失败留下的）。
    let mut original = BTreeMap::new();
    original.insert("Wi-Fi".into(), vec!["192.168.1.1".to_string()]);
    c.marker.write(&original);
    assert!(c.has_marker());

    c.restore_dns();
    assert!(!c.has_marker(), "stuck marker 须被清");
    assert!(
        c.ops.apply_calls.borrow().is_empty(),
        "不接管的平台不得往系统写 DNS"
    );
}

#[test]
fn reconcile_dns_noop_on_non_takeover_platform() {
    let ops = MockDnsOps {
        targets: vec!["Wi-Fi".into()],
        takeover: false,
        ..Default::default()
    };
    let mut c = controller(ops);
    let mut original = BTreeMap::new();
    original.insert("Wi-Fi".into(), vec!["192.168.1.1".to_string()]);
    c.marker.write(&original);

    c.reconcile_dns();
    assert!(c.ops.apply_calls.borrow().is_empty());
}

// ══════════ 生产实现接线（SystemDnsOpsImpl）══════════

use crate::exec::exec_tests_helpers::MockRunner;

fn dns_ops_for(platform: Platform, runner: MockRunner) -> SystemDnsOpsImpl<MockRunner> {
    SystemDnsOpsImpl::with_platform(runner, platform)
}

#[test]
fn impl_takeover_only_on_mac() {
    assert!(dns_ops_for(Platform::Mac, MockRunner::default()).takeover_supported());
    // win/linux 不接管 —— 判据见 trait doc（win netsh 需管理员 + strict_route 已劫持 :53）。
    assert!(!dns_ops_for(Platform::Win, MockRunner::default()).takeover_supported());
    assert!(!dns_ops_for(Platform::Linux, MockRunner::default()).takeover_supported());
    assert!(!dns_ops_for(Platform::Other, MockRunner::default()).takeover_supported());
    // Android：系统解析器不在链路上（DNS 由核在 tun fd 内自理），且非 root 无权改别的网卡。
    assert!(!dns_ops_for(Platform::Android, MockRunner::default()).takeover_supported());

    // 全变体穷举 + 正对照：接管的**只有** mac。
    let takeover: Vec<Platform> = Platform::ALL
        .iter()
        .copied()
        .filter(|p| dns_ops_for(*p, MockRunner::default()).takeover_supported())
        .collect();
    assert_eq!(takeover, vec![Platform::Mac]);
}

/// Android 上四个读写方法全部诚实 no-op，且**一条命令都不跑**。
///
/// 正对照：同一份 MockRunner 在 mac 上会记到 `networksetup` 调用（见本文件的 mac 各测），
/// 故「零调用」不是记账坏了。
#[test]
fn android_dns_ops_are_silent_noops_without_commands() {
    let ops = dns_ops_for(Platform::Android, MockRunner::default());
    assert!(ops.list_targets().unwrap().is_empty());
    assert!(ops.read_dns("wlan0").unwrap().is_empty());
    assert!(ops.apply_dns("wlan0", &["1.1.1.1".to_owned()]).is_ok());
    assert!(ops.read_effective_resolvers().unwrap().is_empty());
    assert!(
        ops.runner.snapshot().is_empty(),
        "Android 上不得 spawn 任何 DNS 命令（`networksetup`/`netsh`/`scutil` 一个都没有）"
    );
}

#[test]
fn impl_mac_list_targets_excludes_bluetooth() {
    let runner = MockRunner::default().with_arg_stdout(
        "-listallnetworkservices",
        "An asterisk...\nWi-Fi\nBluetooth PAN\n*Disabled Svc\nEthernet\n",
    );
    let t = dns_ops_for(Platform::Mac, runner).list_targets().unwrap();
    // 排除 Bluetooth PAN —— 否则 DNS 接管写到蓝牙网络，关闭后残留。
    assert_eq!(t, vec!["Wi-Fi".to_string(), "Ethernet".to_string()]);
}

#[test]
fn impl_mac_read_dns_parses_and_dhcp_is_empty() {
    let runner = MockRunner::default().with_arg_stdout("-getdnsservers", "192.168.1.1\n8.8.4.4\n");
    assert_eq!(
        dns_ops_for(Platform::Mac, runner)
            .read_dns("Wi-Fi")
            .unwrap(),
        vec!["192.168.1.1".to_string(), "8.8.4.4".to_string()]
    );
    // DHCP/自动 → networksetup 输出提示句 → []（不是把提示句当 IP）。
    let runner2 = MockRunner::default().with_arg_stdout(
        "-getdnsservers",
        "There aren't any DNS Servers set on Wi-Fi.\n",
    );
    assert!(dns_ops_for(Platform::Mac, runner2)
        .read_dns("Wi-Fi")
        .unwrap()
        .is_empty());
}

#[test]
fn impl_mac_apply_dns_uses_argv_and_empty_means_dhcp() {
    let ops = dns_ops_for(Platform::Mac, MockRunner::default());
    ops.apply_dns("USB 10/100/1000 LAN", &["8.8.8.8".to_string()])
        .unwrap();
    let cmds = ops.runner.snapshot();
    assert_eq!(cmds[0].program, "networksetup");
    // 服务名含空格经 argv 下发，无引号歧义。
    assert_eq!(cmds[0].args[1], "USB 10/100/1000 LAN");
    assert_eq!(cmds[0].args[2], "8.8.8.8");

    // 空 ips → `Empty`（还原为 DHCP）。
    let ops2 = dns_ops_for(Platform::Mac, MockRunner::default());
    ops2.apply_dns("Wi-Fi", &[]).unwrap();
    assert!(ops2.runner.ran_arg("Empty"));
}

#[test]
fn impl_mac_read_effective_resolvers_uses_scutil() {
    // scutil --dns 才拿得到 DHCP 下发的解析器（-getdnsservers 对 DHCP 返空）。
    let runner = MockRunner::default().with_arg_stdout(
        "--dns",
        "resolver #1\n  nameserver[0] : 192.168.1.1\n  nameserver[1] : 8.8.8.8\nresolver #2\n  nameserver[0] : 192.168.1.1\n",
    );
    let ops = dns_ops_for(Platform::Mac, runner);
    let r = ops.read_effective_resolvers().unwrap();
    assert_eq!(r, vec!["192.168.1.1".to_string(), "8.8.8.8".to_string()]);
    assert_eq!(ops.runner.snapshot()[0].program, "scutil");
}

#[test]
fn impl_win_apply_dns_is_noop_writes_nothing() {
    // 写路径 no-op：一条命令都不能发（netsh set 需管理员，GUI 非提权必失败）。
    let ops = dns_ops_for(Platform::Win, MockRunner::default());
    ops.apply_dns("Ethernet", &["8.8.8.8".to_string()]).unwrap();
    assert!(
        ops.runner.snapshot().is_empty(),
        "win 写路径必须零命令 —— 发了就是 ACCESS DENIED + marker 卡死"
    );
}

#[test]
fn impl_win_read_paths_stay_live_for_plan_b() {
    // 读路径保留（show 非提权可跑），供方案B getLanResolverForDns 用。
    let runner = MockRunner::default()
        .with_arg_stdout(
            "show",
            "Idx     Met         MTU          State                Name\n 12      10        1500  connected            Wi-Fi\n  1      75  4294967295  connected            Loopback Pseudo-Interface 1\n",
        );
    let ops = dns_ops_for(Platform::Win, runner);
    let targets = ops.list_targets().unwrap();
    assert_eq!(targets, vec!["Wi-Fi".to_string()], "loopback 须排除");
    assert!(ops
        .runner
        .snapshot()
        .iter()
        .any(|c| c.program.ends_with("netsh.exe")));
}

#[test]
fn impl_linux_dns_write_is_noop() {
    // 写路径继续 no-op；读路径单独测试。
    let ops = dns_ops_for(Platform::Linux, MockRunner::default());
    assert!(ops.list_targets().unwrap().is_empty());
    assert!(ops.read_dns("eth0").unwrap().is_empty());
    ops.apply_dns("eth0", &["8.8.8.8".to_string()]).unwrap();
    assert!(ops.runner.snapshot().is_empty(), "linux 全 no-op，零命令");
}

/// 组合面（§K7「两扇门之间的缝」）：生产 ops + 控制器一起跑，验 win 不写 marker。
/// 单测 ops 的 no-op 与单测控制器的门控**各自通过**并不能证明组合正确 —— 这条才是生产路径。
#[test]
fn impl_win_controller_combination_writes_no_marker() {
    let ops = dns_ops_for(Platform::Win, MockRunner::default());
    let mut c = SystemDnsController::new(ops, mem_dns_marker());
    c.set_dns();
    assert!(!c.has_marker(), "win 生产路径不得留 marker");
    assert!(
        c.ops.runner.snapshot().is_empty(),
        "win 生产路径不得发任何 DNS 写命令"
    );
}

/// 组合面：mac 生产 ops + 控制器 → 真接管（marker + apply 受控 IP）。
#[test]
fn impl_mac_controller_combination_takes_over() {
    let runner = MockRunner::default()
        .with_arg_stdout("-listallnetworkservices", "An asterisk...\nWi-Fi\n")
        .with_arg_stdout("-getdnsservers", "192.168.1.1\n");
    let ops = dns_ops_for(Platform::Mac, runner);
    let mut c = SystemDnsController::new(ops, mem_dns_marker());
    c.set_dns();
    assert!(c.has_marker(), "mac 应真接管");
    // 受控 IP 被 apply 到 Wi-Fi。
    assert!(c.ops.runner.ran_arg("-setdnsservers"));
    assert!(c.ops.runner.ran_arg(controlled_tun_dns_ip()));
    // marker 记录了接管前的真实 LAN。
    assert_eq!(
        c.marker.read().unwrap().original.get("Wi-Fi").unwrap(),
        &vec!["192.168.1.1".to_string()]
    );
}

// ── C1：接管挤掉的非公网解析器必须被观测到 ──────────────────────────────────────

/// 现场形态：macOS 上跑官方 Tailscale。`networksetup -getdnsservers` 看不见 quad100
/// （它由 NetworkExtension 装），只有 `scutil --dns` 看得见 —— 故观测必须走后者。
///
/// 这条同时是"别改回去用 marker 原始值判"的护栏：`-getdnsservers` 在本夹具里对
/// Wi-Fi 返的是「There aren't any DNS Servers set」，用它判会**恒空**、告警永不出现。
#[test]
fn set_dns_observes_resolver_displaced_by_takeover() {
    let runner = MockRunner::default()
        .with_arg_stdout("-listallnetworkservices", "An asterisk\nWi-Fi\n")
        .with_arg_stdout(
            "-getdnsservers",
            "There aren't any DNS Servers set on Wi-Fi.\n",
        )
        .with_arg_stdout(
            "--dns",
            "resolver #1\n  nameserver[0] : 192.168.1.1\n  nameserver[1] : 8.8.8.8\n\
             resolver #2\n  domain : example.ts.net\n  nameserver[0] : 100.100.100.100\n",
        );
    let mut controller =
        SystemDnsController::new(dns_ops_for(Platform::Mac, runner), mem_dns_marker())
            .with_noop_sleeper();
    controller.set_dns();

    let displaced = controller.displaced_resolvers().to_vec();
    assert!(
        displaced.iter().any(|ip| ip == "100.100.100.100"),
        "Tailscale 的 quad100 被接管挤掉却没被观测到：{displaced:?}"
    );
    assert!(
        displaced.iter().any(|ip| ip == "192.168.1.1"),
        "LAN 解析器同样被挤掉，也该报：{displaced:?}"
    );
    assert!(
        !displaced.iter().any(|ip| ip == "8.8.8.8"),
        "公网解析器不该报 —— 覆盖它正是接管的本意：{displaced:?}"
    );

    // 还原后观测作废（否则没接管时 UI 仍会显示告警）。
    controller.restore_dns();
    assert!(controller.displaced_resolvers().is_empty());
}

/// 不接管的平台（win/linux）不产生观测 —— 那里根本没挤掉任何东西。
#[test]
fn non_takeover_platforms_report_no_displacement() {
    for platform in [Platform::Win, Platform::Linux] {
        let mut controller = SystemDnsController::new(
            dns_ops_for(platform, MockRunner::default()),
            mem_dns_marker(),
        );
        controller.set_dns();
        assert!(
            controller.displaced_resolvers().is_empty(),
            "{platform:?} 不接管系统 DNS，不该报被挤掉的解析器"
        );
    }
}

#[test]
fn lan_dns_keeps_dhcp_before_mac_takeover_without_changing_restore_snapshot() {
    let mut c = controller(MockDnsOps {
        targets: vec!["Wi-Fi".into()],
        ..Default::default()
    });
    assert_eq!(c.get_lan_resolver_for_dns().as_deref(), Some("192.168.1.1"));
    c.set_dns();
    *c.ops.effective.borrow_mut() = vec!["8.8.8.8".into()];
    assert!(c.marker.read().unwrap().original["Wi-Fi"].is_empty());
    assert_eq!(c.get_lan_resolver_for_dns().as_deref(), Some("192.168.1.1"));
    c.set_dns();
    assert_eq!(c.get_lan_resolver_for_dns().as_deref(), Some("192.168.1.1"));
    c.restore_dns();
    assert!(c.ops.dns_state.borrow()["Wi-Fi"].is_empty());
    assert!(c.get_lan_resolver_for_dns().is_none());
    *c.ops.effective.borrow_mut() = vec!["10.20.0.1".into()];
    assert_eq!(c.get_lan_resolver_for_dns().as_deref(), Some("10.20.0.1"));
}

#[test]
fn lan_dns_without_private_upstream_fails_closed() {
    let c = controller(MockDnsOps {
        effective: RefCell::new(vec![
            "8.8.8.8".into(),
            "127.0.0.53".into(),
            "1.1.1.1".into(),
        ]),
        ..Default::default()
    });
    assert!(c.get_lan_resolver_for_dns().is_none());
}

#[test]
fn linux_reads_real_link_dns_and_falls_back_without_resolved() {
    let runner = MockRunner::default().with_arg_stdout(
        "dns",
        "Global: 127.0.0.53\nLink 2 (eth0): 192.168.1.1 fd00::53\nLink 3 (polaris-tun0): 8.8.8.8\n",
    );
    let ops = dns_ops_for(Platform::Linux, runner);
    assert_eq!(
        pick_lan_resolver_ip(&ops.read_effective_resolvers().unwrap(), "8.8.8.8").as_deref(),
        Some("192.168.1.1")
    );
    assert_eq!(ops.runner.snapshot().len(), 1);
    let runner = MockRunner {
        fail_programs: vec!["resolvectl".into()],
        ..Default::default()
    }
    .with_arg_stdout(
        "/etc/resolv.conf",
        "# nameserver 10.0.0.99\nsearch lan\nnameserver fd00::53\n",
    );
    let ops = dns_ops_for(Platform::Linux, runner);
    assert_eq!(ops.read_effective_resolvers().unwrap(), vec!["fd00::53"]);
}

#[test]
fn unreadable_dns_is_not_saved_as_dhcp_and_cannot_authorize_any_apply() {
    let ops = MockDnsOps {
        targets: vec!["A".into(), "B".into()],
        read_fail_targets: vec!["B".into()],
        ..Default::default()
    };
    let mut c = controller(ops).with_noop_sleeper();
    c.set_dns();
    assert!(c.ops.apply_calls.borrow().is_empty());
    assert!(!c.has_marker());
}

struct FailingMarkerFs {
    inner: MemFs,
    write_fails: std::cell::Cell<bool>,
    remove_fails: std::cell::Cell<bool>,
}
impl MarkerFs for FailingMarkerFs {
    fn write_marker(&self, path: &str, data: &str) -> std::io::Result<()> {
        if self.write_fails.get() {
            return Err(std::io::Error::other("injected write failure"));
        }
        self.inner.write_marker(path, data)
    }
    fn read_marker(&self, path: &str) -> Option<String> {
        self.inner.read_marker(path)
    }
    fn remove_marker(&self, path: &str) -> std::io::Result<()> {
        if self.remove_fails.get() {
            return Err(std::io::Error::other("injected remove failure"));
        }
        self.inner.remove_marker(path)
    }
}

#[test]
fn marker_persistence_failure_prevents_initial_and_reconcile_mutations() {
    let fs = FailingMarkerFs {
        inner: MemFs::new(),
        write_fails: true.into(),
        remove_fails: false.into(),
    };
    let mut c = SystemDnsController::new(
        MockDnsOps {
            targets: vec!["A".into()],
            ..Default::default()
        },
        DnsMarker::new(fs, "/dns-marker.json"),
    )
    .with_noop_sleeper();
    c.set_dns();
    assert!(c.ops.apply_calls.borrow().is_empty());
    assert!(c.original.is_none());
    c.marker.fs.write_fails.set(false);
    c.set_dns();
    let old = c.marker.fs.read_marker(c.marker.path()).unwrap();
    c.ops.targets.push("B".into());
    c.ops.apply_calls.borrow_mut().clear();
    c.marker.fs.write_fails.set(true);
    c.reconcile_dns();
    assert!(c.ops.apply_calls.borrow().is_empty());
    assert_eq!(c.marker.fs.read_marker(c.marker.path()).unwrap(), old);
}

#[test]
fn partial_takeover_and_failed_rollback_retain_snapshot_until_successful_recovery() {
    let mut c = controller(MockDnsOps {
        targets: vec!["A".into(), "B".into()],
        dns_state: RefCell::new(BTreeMap::from([
            ("A".into(), vec!["192.168.1.2".into()]),
            ("B".into(), Vec::new()),
        ])),
        apply_fail_targets: vec!["B".into()],
        restore_fail_targets: vec!["A".into()],
        ..Default::default()
    })
    .with_noop_sleeper();
    c.set_dns();
    assert!(c.has_marker());
    assert_eq!(
        c.marker.read().unwrap().original["A"],
        vec!["192.168.1.2".to_string()]
    );
    assert_eq!(
        c.ops.dns_state.borrow()["A"],
        vec![controlled_tun_dns_ip().to_string()]
    );
    c.ops.apply_fail_targets.clear();
    c.ops.restore_fail_targets.clear();
    c.restore_dns();
    assert!(!c.has_marker());
    assert_eq!(
        c.ops.dns_state.borrow()["A"],
        vec!["192.168.1.2".to_string()]
    );
    assert!(
        c.ops.dns_state.borrow()["B"].is_empty(),
        "observed DHCP must remain DHCP"
    );
}

#[test]
fn failed_marker_removal_keeps_original_and_recovery_authority() {
    let fs = FailingMarkerFs {
        inner: MemFs::new(),
        write_fails: false.into(),
        remove_fails: true.into(),
    };
    let mut c = SystemDnsController::new(
        MockDnsOps {
            targets: vec!["A".into()],
            ..Default::default()
        },
        DnsMarker::new(fs, "/dns-marker.json"),
    )
    .with_noop_sleeper();
    c.set_dns();
    c.restore_dns();
    assert!(c.has_marker());
    assert!(c.original.is_some());
    c.marker.fs.remove_fails.set(false);
    c.restore_dns();
    assert!(!c.has_marker());
    assert!(c.original.is_none());
}

#[test]
fn invisible_service_remains_owned_until_restore_succeeds() {
    let mut c = controller(MockDnsOps {
        targets: vec!["A".into(), "B".into()],
        dns_state: RefCell::new(BTreeMap::from([
            ("A".into(), vec!["10.0.0.1".into()]),
            ("B".into(), vec!["10.0.0.2".into()]),
        ])),
        ..Default::default()
    });
    c.set_dns();
    c.ops.targets = vec!["B".into()];
    c.set_dns();
    assert_eq!(c.marker.read().unwrap().original["A"], vec!["10.0.0.1"]);
    c.ops.apply_fail_targets = vec!["A".into()];
    assert!(!c.restore_dns_inner());
    assert!(c.marker.read().unwrap().original.contains_key("A"));
    c.ops.apply_fail_targets.clear();
    assert!(c.restore_dns_inner());
    assert!(!c.has_marker());
    assert_eq!(c.ops.dns_state.borrow()["A"], vec!["10.0.0.1"]);
}

fn live_dns_controller() -> SystemDnsController<MockDnsOps, MemFs> {
    controller(MockDnsOps {
        targets: vec!["A".into(), "B".into()],
        dns_state: RefCell::new(BTreeMap::from([
            ("A".into(), vec!["192.168.1.2".into()]),
            ("B".into(), Vec::new()),
        ])),
        ..Default::default()
    })
    .with_noop_sleeper()
}

#[test]
fn live_dns_prepare_is_read_only_and_receipt_requires_target_readback() {
    let mut c = live_dns_controller();
    let before = c.ops.dns_state.borrow().clone();
    let plan = c.prepare_effect(true).unwrap();
    assert_eq!(plan.before(), &before);
    assert!(plan
        .target()
        .values()
        .all(|ips| ips == &[controlled_tun_dns_ip().to_string()]));
    assert!(c.ops.apply_calls.borrow().is_empty());
    assert!(!c.has_marker());
    let receipt = c.apply_effect(&plan, "dns-one");
    assert_eq!(receipt.outcome, TakeoverOutcome::Applied);
    assert_eq!(receipt.marker_id.as_deref(), Some("dns-one"));
    assert!(!receipt.cleanup_pending);
    assert_eq!(&*c.ops.dns_state.borrow(), plan.target());
    assert_eq!(c.marker.read().unwrap().original, before);
    let raw = c.marker.fs.read_marker(c.marker.path()).unwrap();
    let unchanged = c.prepare_effect(true).unwrap();
    assert_eq!(
        c.apply_effect(&unchanged, "dns-two").outcome,
        TakeoverOutcome::Unchanged
    );
    assert_eq!(c.marker.fs.read_marker(c.marker.path()).unwrap(), raw);
    let release = c.prepare_effect(false).unwrap();
    assert_eq!(
        c.apply_effect(&release, "dns-release").outcome,
        TakeoverOutcome::Applied
    );
    assert_eq!(*c.ops.dns_state.borrow(), before);
    assert!(c.marker.read_checked().unwrap().is_none());
    assert!(c.original.is_none());
}

#[test]
fn live_dns_rejects_values_scope_and_marker_drift_without_mutation() {
    let mut c = live_dns_controller();
    let plan = c.prepare_effect(true).unwrap();
    c.ops
        .dns_state
        .borrow_mut()
        .insert("A".into(), vec!["10.0.0.1".into()]);
    assert_eq!(
        c.apply_effect(&plan, "drift").outcome,
        TakeoverOutcome::Superseded
    );
    assert!(c.ops.apply_calls.borrow().is_empty());
    let plan = c.prepare_effect(true).unwrap();
    c.ops.targets.push("C".into());
    assert_eq!(
        c.apply_effect(&plan, "scope").outcome,
        TakeoverOutcome::Superseded
    );
    assert!(c.ops.apply_calls.borrow().is_empty());
    c.ops.targets.pop();
    let plan = c.prepare_effect(true).unwrap();
    c.marker
        .fs
        .write_marker(c.marker.path(), "invalid")
        .unwrap();
    assert!(c.prepare_effect(true).is_err());
    assert_eq!(
        c.apply_effect(&plan, "invalid").outcome,
        TakeoverOutcome::FailedUnchanged
    );
    assert_eq!(
        c.marker.fs.read_marker(c.marker.path()).as_deref(),
        Some("invalid")
    );
    assert!(c.ops.apply_calls.borrow().is_empty());
}

#[test]
fn live_dns_partial_failure_compensates_to_before_not_user_baseline() {
    let mut c = live_dns_controller();
    let before = c.ops.dns_state.borrow().clone();
    let plan = c.prepare_effect(true).unwrap();
    c.ops.apply_fail_targets.push("B".into());
    let result = c.apply_effect(&plan, "partial");
    assert_eq!(result.outcome, TakeoverOutcome::FailedRestored);
    assert!(!result.cleanup_pending);
    assert_eq!(*c.ops.dns_state.borrow(), before);
    assert!(c.marker.read_checked().unwrap().is_none());
    assert!(c.original.is_none());
    assert_eq!(
        c.ops.apply_calls.borrow().as_slice(),
        &[
            ("A".to_string(), vec![controlled_tun_dns_ip().to_string()]),
            ("B".to_string(), vec![controlled_tun_dns_ip().to_string()]),
            ("A".to_string(), vec!["192.168.1.2".to_string()]),
        ]
    );
}

#[test]
fn live_dns_failed_compensation_retains_original_and_durable_intent() {
    let mut c = live_dns_controller();
    let plan = c.prepare_effect(true).unwrap();
    c.ops.apply_fail_targets.push("B".into());
    c.ops.restore_fail_targets.push("A".into());
    let result = c.apply_effect(&plan, "debt");
    assert_eq!(result.outcome, TakeoverOutcome::RecoveryPending);
    assert!(result.cleanup_pending);
    assert_eq!(c.marker.read().unwrap().original, plan.before);
    assert_eq!(c.original.as_ref(), Some(&plan.before));
    let raw: serde_json::Value =
        serde_json::from_str(&c.marker.fs.read_marker(c.marker.path()).unwrap()).unwrap();
    assert_eq!(raw["hostEffect"]["operationId"], "debt");
    assert_eq!(raw["hostEffect"]["phase"], "applying");
    c.ops.apply_fail_targets.clear();
    c.ops.restore_fail_targets.clear();
    c.restore_dns();
    assert_eq!(*c.ops.dns_state.borrow(), plan.before);
    assert!(!c.has_marker());
}

#[test]
fn live_dns_does_not_overwrite_foreign_values_during_compensation() {
    let mut c = live_dns_controller();
    let plan = c.prepare_effect(true).unwrap();
    c.ops.foreign_after_apply = Some("A".into());
    let result = c.apply_effect(&plan, "foreign");
    assert_eq!(result.outcome, TakeoverOutcome::RecoveryPending);
    assert_eq!(c.ops.dns_state.borrow()["A"], vec!["9.9.9.9"]);
    assert_eq!(c.ops.apply_calls.borrow().len(), 1);
    assert!(c.has_marker());
    assert_eq!(c.marker.read().unwrap().original, plan.before);
}

#[test]
fn live_dns_silent_success_cannot_produce_an_applied_receipt() {
    let mut c = live_dns_controller();
    c.ops.suppress_apply = true;
    let plan = c.prepare_effect(true).unwrap();
    let result = c.apply_effect(&plan, "readback");
    assert_eq!(result.outcome, TakeoverOutcome::FailedUnchanged);
    assert_eq!(*c.ops.dns_state.borrow(), plan.before);
    assert!(!c.has_marker());
}

#[test]
fn live_dns_marker_failure_prevents_writes_and_unreadable_snapshot_is_not_dhcp() {
    let fs = FailingMarkerFs {
        inner: MemFs::new(),
        write_fails: true.into(),
        remove_fails: false.into(),
    };
    let mut c =
        SystemDnsController::new(live_dns_controller().ops, DnsMarker::new(fs, "/live.json"));
    let plan = c.prepare_effect(true).unwrap();
    assert_eq!(
        c.apply_effect(&plan, "no-intent").outcome,
        TakeoverOutcome::FailedUnchanged
    );
    assert!(c.ops.apply_calls.borrow().is_empty());
    c.ops.read_fail_targets.push("B".into());
    assert!(c.prepare_effect(true).is_err());
    assert!(c.ops.apply_calls.borrow().is_empty());
}

#[test]
fn live_dns_late_release_cannot_clear_a_successor_marker() {
    let mut c = live_dns_controller();
    let plan = c.prepare_effect(true).unwrap();
    assert_eq!(
        c.apply_effect(&plan, "first").outcome,
        TakeoverOutcome::Applied
    );
    let release = c.prepare_effect(false).unwrap();
    let mut successor: serde_json::Value =
        serde_json::from_str(&c.marker.fs.read_marker(c.marker.path()).unwrap()).unwrap();
    successor["hostEffect"]["operationId"] = "successor".into();
    let raw = successor.to_string();
    c.marker.fs.write_marker(c.marker.path(), &raw).unwrap();
    c.ops.apply_calls.borrow_mut().clear();
    assert_eq!(
        c.apply_effect(&release, "late").outcome,
        TakeoverOutcome::Superseded
    );
    assert_eq!(
        c.marker.fs.read_marker(c.marker.path()).as_deref(),
        Some(raw.as_str())
    );
    assert!(c.ops.apply_calls.borrow().is_empty());
}

#[test]
fn live_dns_unsupported_platform_has_no_intent_or_fake_success() {
    let mut c = live_dns_controller();
    c.ops.takeover = false;
    assert!(matches!(
        c.prepare_effect(true),
        Err(SystemIntegrationError::UnsupportedPlatform(_))
    ));
    assert!(c.ops.apply_calls.borrow().is_empty());
    assert!(!c.has_marker());
}

#[test]
fn live_dns_unsettled_or_foreign_footprint_blocks_new_apply_and_stop_restore() {
    let mut c = live_dns_controller();
    let plan = c.prepare_effect(true).unwrap();
    c.ops.foreign_after_apply = Some("A".into());
    assert_eq!(
        c.apply_effect(&plan, "foreign-owner").outcome,
        TakeoverOutcome::RecoveryPending
    );
    assert!(c.prepare_effect(true).is_err());
    assert!(c.prepare_effect(false).is_err());
    let raw = c.marker.fs.read_marker(c.marker.path()).unwrap();
    c.ops.apply_calls.borrow_mut().clear();
    c.restore_dns();
    assert_eq!(c.ops.dns_state.borrow()["A"], vec!["9.9.9.9"]);
    assert!(c.ops.apply_calls.borrow().is_empty());
    assert_eq!(
        c.marker.fs.read_marker(c.marker.path()).as_deref(),
        Some(raw.as_str())
    );
    c.reconcile_dns();
    assert!(c.ops.apply_calls.borrow().is_empty());
}

#[test]
fn live_dns_original_dhcp_and_physical_lan_resolver_have_separate_custody() {
    let mut c = live_dns_controller();
    let plan = c
        .prepare_effect(true)
        .unwrap()
        .with_physical_lan_resolver(Some("192.168.1.1".into()));
    assert_eq!(
        c.apply_effect(&plan, "lan-owner").outcome,
        TakeoverOutcome::Applied
    );
    assert!(c.marker.read().unwrap().original["B"].is_empty());
    *c.ops.effective.borrow_mut() = vec![controlled_tun_dns_ip().into()];
    assert_eq!(c.get_lan_resolver_for_dns().as_deref(), Some("192.168.1.1"));
    let release = c.prepare_effect(false).unwrap();
    assert_eq!(
        c.apply_effect(&release, "lan-release").outcome,
        TakeoverOutcome::Applied
    );
    assert!(c.ops.dns_state.borrow()["B"].is_empty());
    assert!(c.original_lan_resolver.is_none());
}

struct LockedOutDnsFs(MemFs);
impl MarkerFs for LockedOutDnsFs {
    fn write_marker(&self, path: &str, data: &str) -> std::io::Result<()> {
        self.0.write_marker(path, data)
    }
    fn read_marker(&self, path: &str) -> Option<String> {
        self.0.read_marker(path)
    }
    fn remove_marker(&self, path: &str) -> std::io::Result<()> {
        self.0.remove_marker(path)
    }
    fn acquire_marker_mutation_lock(
        &self,
        _: &str,
    ) -> std::io::Result<crate::proxy::MarkerMutationLockGuard> {
        Err(std::io::Error::other("marker already held"))
    }
}

#[test]
fn live_dns_marker_lock_failure_blocks_new_and_existing_lifecycle_writers() {
    let mut c = SystemDnsController::new(
        live_dns_controller().ops,
        DnsMarker::new(LockedOutDnsFs(MemFs::new()), "/locked.json"),
    );
    let prepared = c.prepare_effect(true).unwrap();
    assert_eq!(
        c.apply_effect(&prepared, "locked").outcome,
        TakeoverOutcome::FailedUnchanged
    );
    c.set_dns();
    c.restore_dns();
    c.reconcile_dns();
    assert!(c.ops.apply_calls.borrow().is_empty());
    assert!(!c.has_marker());
}

#[test]
fn live_dns_invalid_operation_phase_and_scope_cannot_become_legacy_authority() {
    for mutation in ["identity", "phase", "scope", "target"] {
        let mut c = live_dns_controller();
        let prepared = c.prepare_effect(true).unwrap();
        assert_eq!(
            c.apply_effect(&prepared, "owned").outcome,
            TakeoverOutcome::Applied
        );
        let mut raw: serde_json::Value =
            serde_json::from_str(&c.marker.fs.read_marker(c.marker.path()).unwrap()).unwrap();
        match mutation {
            "identity" => raw["hostEffect"]["operationId"] = "".into(),
            "phase" => raw["hostEffect"]["phase"] = "unrecognized".into(),
            "scope" => {
                raw["hostEffect"]["target"]
                    .as_object_mut()
                    .unwrap()
                    .remove("A");
            }
            "target" => raw["hostEffect"]["target"]["A"] = serde_json::json!(["1.1.1.1"]),
            _ => unreachable!(),
        }
        let raw = raw.to_string();
        c.marker.fs.write_marker(c.marker.path(), &raw).unwrap();
        c.ops.apply_calls.borrow_mut().clear();
        assert!(c.prepare_effect(false).is_err(), "{mutation}");
        c.set_dns();
        c.reconcile_dns();
        c.restore_dns();
        assert!(c.ops.apply_calls.borrow().is_empty(), "{mutation}");
        assert_eq!(
            c.marker.fs.read_marker(c.marker.path()).as_deref(),
            Some(raw.as_str()),
            "{mutation}"
        );
    }
}

#[test]
fn live_dns_restore_silent_success_retains_restoring_marker_until_actual_readback() {
    let mut c = live_dns_controller();
    let prepared = c.prepare_effect(true).unwrap();
    assert_eq!(
        c.apply_effect(&prepared, "restore-owner").outcome,
        TakeoverOutcome::Applied
    );
    c.ops.suppress_apply = true;
    c.restore_dns();
    assert!(c.has_marker());
    let raw: serde_json::Value =
        serde_json::from_str(&c.marker.fs.read_marker(c.marker.path()).unwrap()).unwrap();
    assert_eq!(raw["hostEffect"]["operationId"], "restore-owner");
    assert_eq!(raw["hostEffect"]["phase"], "restoring");
    let marker_before = c.marker.fs.read_marker(c.marker.path()).unwrap();
    let values_before = c.ops.dns_state.borrow().clone();
    c.ops.apply_calls.borrow_mut().clear();
    let retry = c.prepare_effect(true).unwrap();
    assert!(retry.needs_prior_restore());
    assert_eq!(
        retry.prior_marker_operation_id().as_deref(),
        Some("restore-owner")
    );
    assert_eq!(retry.scope(), prepared.scope());
    assert_eq!(retry.original, prepared.before);
    assert_eq!(retry.before(), &values_before);
    assert!(c.checked_owned_receipt().unwrap().is_none());
    assert!(c.ops.apply_calls.borrow().is_empty());
    assert_eq!(
        c.marker.fs.read_marker(c.marker.path()).unwrap(),
        marker_before
    );
    assert_eq!(*c.ops.dns_state.borrow(), values_before);
    let rejected = c.apply_effect(&retry, "must-not-replace-restoring");
    assert_eq!(rejected.outcome, TakeoverOutcome::RecoveryPending);
    assert_eq!(rejected.marker_id.as_deref(), Some("restore-owner"));
    assert!(rejected.cleanup_pending);
    assert!(c.ops.apply_calls.borrow().is_empty());
    assert_eq!(
        c.marker.fs.read_marker(c.marker.path()).unwrap(),
        marker_before
    );
    assert_eq!(*c.ops.dns_state.borrow(), values_before);
    assert!(c.prepare_after_restore(&retry).is_err());
    c.restore_dns();
    assert!(
        c.has_marker(),
        "silent restore cannot discharge old-owner debt"
    );
    assert_eq!(
        c.exact_marker_operation_id().as_deref(),
        Some("restore-owner")
    );
    assert_eq!(*c.ops.dns_state.borrow(), values_before);
    assert_eq!(c.marker.read().unwrap().original, prepared.before);
    c.ops.suppress_apply = false;
    c.restore_dns();
    assert!(!c.has_marker());
    assert_eq!(*c.ops.dns_state.borrow(), prepared.before);
    let next = c.prepare_after_restore(&retry).unwrap();
    assert_eq!(next.scope(), retry.scope());
    assert_eq!(next.before(), &prepared.before);
    assert!(!next.needs_prior_restore());
    assert!(next.prior_marker_operation_id().is_none());
}

#[test]
fn live_reconcile_extends_exact_marker_and_preserves_original_document_and_owner() {
    let mut c = live_dns_controller();
    let prepared = c.prepare_effect(true).unwrap();
    assert_eq!(
        c.apply_effect(&prepared, "dns-origin").outcome,
        TakeoverOutcome::Applied
    );
    let mut raw: serde_json::Value =
        serde_json::from_str(&c.marker.fs.read_marker(c.marker.path()).unwrap()).unwrap();
    raw["diagnosticExtension"] = serde_json::json!({"keep":true});
    c.marker
        .fs
        .write_marker(c.marker.path(), &raw.to_string())
        .unwrap();
    c.ops.targets.push("C".into());
    c.ops
        .dns_state
        .borrow_mut()
        .insert("C".into(), vec!["10.0.0.53".into()]);
    c.ops.apply_calls.borrow_mut().clear();
    let receipt = c.reconcile_dns_checked().unwrap();
    assert_eq!(receipt.outcome, TakeoverOutcome::Applied);
    assert_eq!(receipt.operation_id, "dns-origin");
    assert_eq!(receipt.marker_id.as_deref(), Some("dns-origin"));
    let raw: serde_json::Value =
        serde_json::from_str(&c.marker.fs.read_marker(c.marker.path()).unwrap()).unwrap();
    assert_eq!(raw["diagnosticExtension"]["keep"], true);
    assert_eq!(raw["hostEffect"]["operationId"], "dns-origin");
    assert_eq!(raw["hostEffect"]["phase"], "owned");
    assert_eq!(raw["original"]["C"], serde_json::json!(["10.0.0.53"]));
    assert_eq!(
        raw["hostEffect"]["before"]["C"],
        serde_json::json!(["10.0.0.53"])
    );
    assert_eq!(
        raw["hostEffect"]["target"]["C"],
        serde_json::json!([controlled_tun_dns_ip()])
    );
    assert_eq!(
        *c.ops.apply_calls.borrow(),
        vec![("C".into(), vec![controlled_tun_dns_ip().into()])]
    );
    c.restore_dns();
    assert!(!c.has_marker());
    assert_eq!(c.ops.dns_state.borrow()["C"], vec!["10.0.0.53"]);
    assert!(c.ops.dns_state.borrow()["B"].is_empty());
}

#[test]
fn live_reconcile_missing_old_service_admits_new_service_but_retains_partial_restore_debt() {
    let mut c = live_dns_controller();
    let prepared = c.prepare_effect(true).unwrap();
    assert_eq!(
        c.apply_effect(&prepared, "scope-owner").outcome,
        TakeoverOutcome::Applied
    );
    c.ops.targets = vec!["B".into(), "C".into()];
    c.ops.dns_state.borrow_mut().remove("A");
    c.ops.read_fail_targets.push("A".into());
    c.ops
        .dns_state
        .borrow_mut()
        .insert("C".into(), vec!["10.1.0.53".into()]);
    let receipt = c.reconcile_dns_checked().unwrap();
    assert_eq!(receipt.outcome, TakeoverOutcome::RecoveryPending);
    assert!(receipt.cleanup_pending);
    assert_eq!(c.ops.dns_state.borrow()["C"], vec![controlled_tun_dns_ip()]);
    let raw: serde_json::Value =
        serde_json::from_str(&c.marker.fs.read_marker(c.marker.path()).unwrap()).unwrap();
    assert_eq!(raw["hostEffect"]["operationId"], "scope-owner");
    assert_eq!(raw["original"]["A"], serde_json::json!(["192.168.1.2"]));
    assert!(raw["hostEffect"]["target"].get("A").is_some());
    c.restore_dns();
    assert!(c.has_marker());
    assert!(c.ops.dns_state.borrow()["B"].is_empty());
    assert_eq!(c.ops.dns_state.borrow()["C"], vec!["10.1.0.53"]);
    let restoring: serde_json::Value =
        serde_json::from_str(&c.marker.fs.read_marker(c.marker.path()).unwrap()).unwrap();
    assert_eq!(restoring["hostEffect"]["phase"], "restoring");
    c.ops.apply_calls.borrow_mut().clear();
    assert_eq!(
        c.reconcile_dns_checked().unwrap().outcome,
        TakeoverOutcome::RecoveryPending
    );
    assert!(c.ops.apply_calls.borrow().is_empty());
    c.ops.read_fail_targets.clear();
    c.ops
        .dns_state
        .borrow_mut()
        .insert("A".into(), vec!["192.168.1.2".into()]);
    c.restore_dns();
    assert!(!c.has_marker());
}

#[test]
fn live_reconcile_foreign_existing_or_unowned_controlled_new_service_never_writes_or_downgrades() {
    for foreign_existing in [false, true] {
        let mut c = live_dns_controller();
        let prepared = c.prepare_effect(true).unwrap();
        assert_eq!(
            c.apply_effect(&prepared, "kept-owner").outcome,
            TakeoverOutcome::Applied
        );
        c.ops.targets.push("C".into());
        c.ops.dns_state.borrow_mut().insert(
            "C".into(),
            vec![if foreign_existing {
                "10.0.0.53"
            } else {
                controlled_tun_dns_ip()
            }
            .into()],
        );
        if foreign_existing {
            c.ops
                .dns_state
                .borrow_mut()
                .insert("A".into(), vec!["9.9.9.9".into()]);
        }
        let before = c.marker.fs.read_marker(c.marker.path()).unwrap();
        c.ops.apply_calls.borrow_mut().clear();
        assert_eq!(
            c.reconcile_dns_checked().unwrap().outcome,
            TakeoverOutcome::RecoveryPending
        );
        assert!(c.ops.apply_calls.borrow().is_empty());
        assert_eq!(c.marker.fs.read_marker(c.marker.path()).unwrap(), before);
        assert!(c.prepare_effect(true).is_err());
    }
}

#[test]
fn live_reconcile_failed_new_service_readback_keeps_applying_and_same_owner_can_finish() {
    let mut c = live_dns_controller();
    let prepared = c.prepare_effect(true).unwrap();
    assert_eq!(
        c.apply_effect(&prepared, "readback-owner").outcome,
        TakeoverOutcome::Applied
    );
    c.ops.targets.push("C".into());
    c.ops
        .dns_state
        .borrow_mut()
        .insert("C".into(), vec!["10.0.0.53".into()]);
    c.ops.suppress_apply = true;
    assert_eq!(
        c.reconcile_dns_checked().unwrap().outcome,
        TakeoverOutcome::RecoveryPending
    );
    let raw: serde_json::Value =
        serde_json::from_str(&c.marker.fs.read_marker(c.marker.path()).unwrap()).unwrap();
    assert_eq!(raw["hostEffect"]["phase"], "applying");
    assert_eq!(raw["hostEffect"]["operationId"], "readback-owner");
    assert_eq!(raw["original"]["C"], serde_json::json!(["10.0.0.53"]));
    c.ops.suppress_apply = false;
    let receipt = c.reconcile_dns_checked().unwrap();
    assert_eq!(receipt.outcome, TakeoverOutcome::Applied);
    assert_eq!(receipt.marker_id.as_deref(), Some("readback-owner"));
    let raw: serde_json::Value =
        serde_json::from_str(&c.marker.fs.read_marker(c.marker.path()).unwrap()).unwrap();
    assert_eq!(raw["hostEffect"]["phase"], "owned");
    c.restore_dns();
    assert_eq!(c.ops.dns_state.borrow()["C"], vec!["10.0.0.53"]);
}

#[test]
fn exact_partial_dns_retry_restores_original_owner_before_replacement_and_rechecks_scope() {
    let mut c = live_dns_controller();
    let prepared = c.prepare_effect(true).unwrap();
    assert_eq!(
        c.apply_effect(&prepared, "old-owner").outcome,
        TakeoverOutcome::Applied
    );
    c.ops.targets.push("C".into());
    c.ops
        .dns_state
        .borrow_mut()
        .insert("C".into(), vec!["10.0.0.53".into()]);
    c.ops.suppress_apply = true;
    assert_eq!(
        c.reconcile_dns_checked().unwrap().outcome,
        TakeoverOutcome::RecoveryPending
    );
    c.ops.suppress_apply = false;
    let marker_before = c.marker.fs.read_marker(c.marker.path()).unwrap();
    let values_before = c.ops.dns_state.borrow().clone();
    c.ops.apply_calls.borrow_mut().clear();
    let retry = c.prepare_effect(true).unwrap();
    assert!(retry.needs_prior_restore());
    assert_eq!(
        retry.prior_marker_operation_id().as_deref(),
        Some("old-owner")
    );
    assert_eq!(retry.scope(), prepared.scope());
    assert_eq!(retry.original["A"], prepared.before["A"]);
    assert_eq!(retry.original["C"], vec!["10.0.0.53"]);
    assert!(c.checked_owned_receipt().unwrap().is_none());
    let rejected = c.apply_effect(&retry, "must-not-replace-applying");
    assert_eq!(rejected.outcome, TakeoverOutcome::RecoveryPending);
    assert_eq!(rejected.marker_id.as_deref(), Some("old-owner"));
    assert!(rejected.cleanup_pending);
    assert!(c.ops.apply_calls.borrow().is_empty());
    assert_eq!(
        c.marker.fs.read_marker(c.marker.path()).unwrap(),
        marker_before
    );
    assert_eq!(*c.ops.dns_state.borrow(), values_before);
    c.ops
        .dns_state
        .borrow_mut()
        .insert("A".into(), vec!["9.9.9.9".into()]);
    assert!(c.prepare_effect(true).is_err());
    assert_eq!(
        c.marker.fs.read_marker(c.marker.path()).unwrap(),
        marker_before
    );
    *c.ops.dns_state.borrow_mut() = values_before;
    c.restore_dns();
    assert!(!c.has_marker());
    let next = c.prepare_after_restore(&retry).unwrap();
    assert_eq!(next.before()["A"], vec!["192.168.1.2"]);
    assert_eq!(
        c.apply_effect(&next, "new-owner").outcome,
        TakeoverOutcome::Applied
    );
    assert_eq!(c.exact_marker_operation_id().as_deref(), Some("new-owner"));
    c.restore_dns();
    assert!(!c.has_marker());
    assert_eq!(*c.ops.dns_state.borrow(), retry.original);
    c.ops
        .dns_state
        .borrow_mut()
        .insert("A".into(), vec!["9.9.9.9".into()]);
    assert!(c.prepare_after_restore(&retry).is_err());
    assert!(!c.has_marker());
    *c.ops.dns_state.borrow_mut() = retry.original.clone();
    c.ops.targets.push("D".into());
    assert!(c.prepare_after_restore(&retry).is_err());
}

#[test]
fn live_reconcile_duplicate_service_identity_keeps_exact_marker_without_mutation() {
    let mut c = live_dns_controller();
    let prepared = c.prepare_effect(true).unwrap();
    assert_eq!(
        c.apply_effect(&prepared, "unique-owner").outcome,
        TakeoverOutcome::Applied
    );
    let raw = c.marker.fs.read_marker(c.marker.path()).unwrap();
    c.ops.targets.push("A".into());
    c.ops.apply_calls.borrow_mut().clear();
    assert_eq!(
        c.reconcile_dns_checked().unwrap().outcome,
        TakeoverOutcome::RecoveryPending
    );
    assert!(c.ops.apply_calls.borrow().is_empty());
    assert_eq!(c.marker.fs.read_marker(c.marker.path()).unwrap(), raw);
}

struct InventoryDriftDnsOps {
    values: std::cell::RefCell<BTreeMap<String, Vec<String>>>,
    visible: std::cell::RefCell<Vec<String>>,
    next_on_write: std::cell::RefCell<Option<String>>,
}
impl InventoryDriftDnsOps {
    fn new() -> Self {
        Self {
            values: std::cell::RefCell::new(BTreeMap::from([
                ("A".into(), vec!["192.168.1.2".into()]),
                ("C".into(), vec!["10.0.0.53".into()]),
                ("D".into(), vec!["172.16.0.53".into()]),
            ])),
            visible: std::cell::RefCell::new(vec!["A".into()]),
            next_on_write: std::cell::RefCell::new(None),
        }
    }
}
impl SystemDnsOps for InventoryDriftDnsOps {
    fn takeover_supported(&self) -> bool {
        true
    }
    fn list_targets(&self) -> Result<Vec<String>, SystemIntegrationError> {
        Ok(self.visible.borrow().clone())
    }
    fn read_dns(&self, name: &str) -> Result<Vec<String>, SystemIntegrationError> {
        self.values
            .borrow()
            .get(name)
            .cloned()
            .ok_or_else(|| SystemIntegrationError::dns("missing test service"))
    }
    fn apply_dns(&self, name: &str, values: &[String]) -> Result<(), SystemIntegrationError> {
        self.values.borrow_mut().insert(name.into(), values.into());
        if let Some(appeared) = self.next_on_write.borrow_mut().take() {
            self.visible.borrow_mut().push(appeared);
        }
        Ok(())
    }
    fn read_effective_resolvers(&self) -> Result<Vec<String>, SystemIntegrationError> {
        let mut all = Vec::new();
        for name in self.list_targets()? {
            for ip in self.read_dns(&name)? {
                if !all.contains(&ip) {
                    all.push(ip);
                }
            }
        }
        Ok(all)
    }
}

#[test]
fn exact_apply_inventory_drift_cannot_report_an_unchecked_full_scope_as_owned() {
    let ops = InventoryDriftDnsOps::new();
    *ops.next_on_write.borrow_mut() = Some("C".into());
    let mut c = SystemDnsController::new(ops, DnsMarker::new(MemFs::default(), "/inventory.json"));
    let prepared = c.prepare_effect(true).unwrap();
    assert_eq!(prepared.before().len(), 1);
    let result = c.apply_effect(&prepared, "inventory-apply");
    assert_eq!(result.outcome, TakeoverOutcome::FailedRestored);
    assert!(!c.has_marker());
    assert_eq!(c.ops.values.borrow()["A"], vec!["192.168.1.2"]);
    assert_eq!(c.ops.values.borrow()["C"], vec!["10.0.0.53"]);
}

#[test]
fn live_reconcile_inventory_drift_retains_applying_and_next_checked_scope_can_finish() {
    let ops = InventoryDriftDnsOps::new();
    let mut c = SystemDnsController::new(
        ops,
        DnsMarker::new(MemFs::default(), "/inventory-live.json"),
    );
    let prepared = c.prepare_effect(true).unwrap();
    assert_eq!(
        c.apply_effect(&prepared, "inventory-owner").outcome,
        TakeoverOutcome::Applied
    );
    c.ops.visible.borrow_mut().push("C".into());
    *c.ops.next_on_write.borrow_mut() = Some("D".into());
    assert_eq!(
        c.reconcile_dns_checked().unwrap().outcome,
        TakeoverOutcome::RecoveryPending
    );
    let pending: serde_json::Value =
        serde_json::from_str(&c.marker.fs.read_marker(c.marker.path()).unwrap()).unwrap();
    assert_eq!(pending["hostEffect"]["operationId"], "inventory-owner");
    assert_eq!(pending["hostEffect"]["phase"], "applying");
    assert_eq!(pending["original"]["C"], serde_json::json!(["10.0.0.53"]));
    assert!(
        pending["original"].get("D").is_none(),
        "not-yet-admitted service is not falsely owned"
    );
    assert_eq!(c.ops.values.borrow()["D"], vec!["172.16.0.53"]);
    assert_eq!(
        c.reconcile_dns_checked().unwrap().outcome,
        TakeoverOutcome::Applied
    );
    c.restore_dns();
    assert!(!c.has_marker());
    assert_eq!(c.ops.values.borrow()["A"], vec!["192.168.1.2"]);
    assert_eq!(c.ops.values.borrow()["C"], vec!["10.0.0.53"]);
    assert_eq!(c.ops.values.borrow()["D"], vec!["172.16.0.53"]);
}
