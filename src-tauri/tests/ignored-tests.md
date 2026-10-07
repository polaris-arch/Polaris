# 默认不跑的测试（`#[ignore]`）——它们是什么、怎么跑

本仓有一批测试标了 `#[ignore]`：`cargo test` 默认**不执行**它们，报告里显式打 `ignored`。

先说清楚为什么是这个形态。另一种常见写法是「前置条件不满足就 `return`」——那样 cargo 报的是
**ok**：一条从没跑过的测试冒充通过，是假绿。`#[ignore]` 不冒充：它在报告里是独立的一档，
且带理由串。而且被 ignore 的测试**仍然参与编译**，不会烂成编不过的死代码。

代价是另一面：**默认不跑 = 默认没有覆盖**。所以这份文档存在，并且由
`src-tauri/tests/ignored_tests_registry.rs` 强制：每一处 `#[ignore]` 必须登记在那道门的
`REGISTRY` 里、必须带理由、理由必须与类别一致、条目过期即红。**不许有人靠随手加一个
`#[ignore]` 把碍事的测试变哑而全仓仍绿。**

---

## 六个类别

| 类别 | 为什么默认不跑 | 谁来跑 |
|---|---|---|
| **RealCore** | 需要 `POLARIS_SINGBOX_PATH` 指向**真实的 sing-box 二进制**，会真起进程、真占端口 | 真机验收环节，人在场 |
| **PublicNetwork** | 需要公网连通性。CI 与本机开发环境都不保证，且本仓禁止在默认门里碰网络 | 手动，确认网络可用时 |
| **LiveHostState** | 会读或**改写宿主机的真实状态**（路由表 / 系统代理）。跑错机器会把开发机的网络配置改掉 | 专用测试机，人在场 |
| **PrivilegedPrivateFs** | 需要 root，只写测试独占的临时目录与子进程 UID，拒绝操作真实 claims 或网络 | 隔离 Linux 验证环境，按父门过滤器执行 |
| **PrivateChildFixture** | 由父测试供给私有输入与身份，不能作为独立门运行 | 仅由父门以精确过滤器调用并收尾 |
| **NotAGate** | 根本不是测试，是打印工具（如逃生门清单）。用 `#[ignore]` 只是为了不进默认门 | 需要那份清单时随手跑 |

---

## 怎么跑

### RealCore

```bash
# 1) 准备一个真实 sing-box 二进制（随包核即可）
export POLARIS_SINGBOX_PATH=/绝对路径/sing-box
# 2) speedtest 那条还需要一个真实网卡名
export POLARIS_TEST_INTERFACE=eth0

# 3) 跑（--ignored 只跑被忽略的那些；--test-threads=1 因为它们抢同一把跨模块真核锁）
cargo test --bin polaris -- --ignored --test-threads=1 real_core_
```

> `real_core_crash_loop_gives_up_without_infinite_restart` 内含 2+5+15s 的退避，单条就要半分钟以上，
> 这是它被单独标注的原因。

**这些测试会真的起停 sing-box 进程。** 不要在正在用代理的机器上跑。

### PublicNetwork

```bash
cargo test --bin polaris -- --ignored real_https_get_handshakes_and_returns_body
```

### LiveHostState

**跑之前先读这一段。** 这一类里 macOS 的两条会**改写宿主机的系统代理设置**；虽然它们自称
「事务后恢复」，但那正是被测对象本身——它坏了就不会恢复。只在可以随手重置网络配置的专用
测试机上跑，且人在场。

```bash
# 路由表（只读，安全）
cargo test --bin polaris -- --ignored live_route_planner_returns_a_real_interface

# Windows 路由（只读，安全；只在 Windows 上编译）
cargo test -p polaris-helper -- --ignored live_best_route_interface_alias_supports_both_families

# macOS 系统代理（**会改写宿主机状态**；只在 macOS 上编译）
cargo test -p polaris-system-integration -- --ignored production_macos_native_proxy_
```

### NotAGate

```bash
cargo test -p polaris --test release_escape_hatches -- --ignored --nocapture inventory
```

---

## 完整清单（35 条）

`src-tauri/tests/ignored_tests_registry.rs` 的 `REGISTRY` 是真值源；本表由它逐条对应，
**每个测试名都必须在本文档里逐字出现**（那道门会逐条核对，前缀兜底已被去掉——它会让 2/3 的条目失守）。

| 测试 | 文件 | 类别 |
|---|---|---|
| `real_core_full_lifecycle` | `src-tauri/src/runtime/proxy/tests/lifecycle.rs` | RealCore |
| `real_core_lifecycle_race_start_then_immediate_stop` | `src-tauri/src/runtime/proxy/tests/lifecycle.rs` | RealCore |
| `real_core_hot_switch_keeps_pid` | `src-tauri/src/runtime/proxy/tests/hot_switch.rs` | RealCore |
| `real_core_auto_failover_attests_without_applying_saved_debt` | `src-tauri/src/runtime/proxy/tests/hot_switch.rs` | RealCore |
| `real_core_hot_switch_failure_falls_back_to_restart` | `src-tauri/src/runtime/proxy/tests/hot_switch.rs` | RealCore |
| `real_core_crash_triggers_auto_restart` | `src-tauri/src/runtime/proxy/tests/recovery.rs` | RealCore |
| `real_core_crash_feeds_diagnostic_restart_axis` | `src-tauri/src/runtime/proxy/tests/recovery.rs` | RealCore |
| `real_core_intentional_stop_does_not_restart` | `src-tauri/src/runtime/proxy/tests/recovery.rs` | RealCore |
| `real_core_crash_loop_gives_up_without_infinite_restart` | `src-tauri/src/runtime/proxy/tests/recovery.rs` | RealCore（含 2+5+15s 退避） |
| `real_core_stale_cleanup_kills_own_orphan_spares_foreign` | `src-tauri/src/runtime/proxy/tests/process_supervision.rs` | RealCore |
| `real_core_accepts_bound_shadow_tls_temp_config` | `src-tauri/src/runtime/speedtest/tests/mod.rs` | RealCore（另需 `POLARIS_TEST_INTERFACE`） |
| `real_core_aggregate_relay_emits_real_frames` | `src-tauri/src/runtime/stats/tests/real_core_tests.rs` | RealCore |
| `real_https_get_handshakes_and_returns_body` | `src-tauri/src/runtime/http/tests/mod.rs` | PublicNetwork |
| `live_route_planner_returns_a_real_interface` | `src-tauri/src/runtime/route_binding/tests/mod.rs` | LiveHostState（只读） |
| `live_best_route_interface_alias_supports_both_families` | `crates/helper/src/platform/windows/wintun/tests/mod.rs` | LiveHostState（只读，Windows-only） |
| `production_macos_native_proxy_transaction_restores_after_takeover` | `crates/system-integration/src/tests/mod.rs` | LiveHostState（**写宿主机**，macOS-only） |
| `production_macos_native_proxy_recovers_across_process_sessions` | 同上 | LiveHostState（**写宿主机**，macOS-only） |
| `inventory` | `src-tauri/tests/release_escape_hatches.rs` | NotAGate |
| `no_network_fixture` | `crates/core-supervisor/src/exact_spawn/tests/mod.rs` | PrivateChildFixture |
| `unrelated_no_network_fixture` | `crates/core-supervisor/src/exact_spawn/tests/mod.rs` | PrivateChildFixture |
| `root_provision_publishes_exact_v2_without_rewriting_existing_claims` | `crates/helper/src/platform/linux/claims/tests/mod.rs` | PrivilegedPrivateFs |
| `root_old_private_or_partial_layout_is_never_repaired` | `crates/helper/src/platform/linux/claims/tests/mod.rs` | PrivilegedPrivateFs |
| `root_concurrent_initializers_only_accept_the_completed_layout` | `crates/helper/src/platform/linux/claims/tests/mod.rs` | PrivilegedPrivateFs |
| `root_existing_wrong_base_modes_and_owner_are_refused` | `crates/helper/src/platform/linux/claims/tests/mod.rs` | PrivilegedPrivateFs |
| `root_symlink_and_untrusted_parent_are_refused` | `crates/helper/src/platform/linux/claims/tests/mod.rs` | PrivilegedPrivateFs |
| `root_marker_bytes_links_mode_owner_and_allocator_size_are_strict` | `crates/helper/src/platform/linux/claims/tests/mod.rs` | PrivilegedPrivateFs |
| `root_marker_and_allocator_replacements_do_not_rebind_old_fds` | `crates/helper/src/platform/linux/claims/tests/mod.rs` | PrivilegedPrivateFs |
| `root_base_and_parent_replacements_are_detected` | `crates/helper/src/platform/linux/claims/tests/mod.rs` | PrivilegedPrivateFs |
| `root_symlink_and_fifo_marker_never_block_or_publish` | `crates/helper/src/platform/linux/claims/tests/mod.rs` | PrivilegedPrivateFs |
| `root_publication_sync_errors_survive_statically_valid_reopen` | `crates/helper/src/platform/linux/claims/tests/mod.rs` | PrivilegedPrivateFs |
| `root_successful_handoff_keeps_the_original_deployment_fds` | `crates/helper/src/platform/linux/claims/tests/mod.rs` | PrivilegedPrivateFs |
| `root_fresh_held_base_replacement_with_moved_files_is_not_published` | `crates/helper/src/platform/linux/claims/tests/mod.rs` | PrivilegedPrivateFs |
| `root_peer_uids_with_original_ambient_caps_share_the_sticky_store` | `crates/helper/src/platform/linux/claims/tests/mod.rs` | PrivilegedPrivateFs |
| `uid_worker` | `crates/helper/src/platform/linux/claims/tests/mod.rs` | PrivateChildFixture |
| `native_core_lists_and_switches_the_two_compiled_modes` | `crates/singbox-grpc/tests/native_clash_mode.rs` | RealCore |

---

## 私有夹具的跑法与边界

`no_network_fixture` 只由 `exact_spawn::tests` 的私有 probe 执行；父门供给 sealed stdin、管理子进程身份与管道并收尾。不能把子夹具的成功当成独立能力验收：

```bash
POLARIS_NO_KERNEL_RUN=1 cargo test -p polaris-core-supervisor exact_spawn::tests -- --test-threads=1
```

`unrelated_no_network_fixture` 是普通 `Command` 启动的独立子可执行夹具，不打开或关闭受保护 FD。父门以 `--exact --ignored --nocapture exact_spawn::tests::unrelated_no_network_fixture` 调用它，在两秒内收到真实用户态 READY 后扫描其 FD，并在观察或断言失败前 kill/wait。
默认运行的 `unrelated_child_does_not_inherit_protected_images` 保留 FD 隔离断言；`deliberately_passed_protected_stdin_is_detected_after_userland_ready` 保留真实继承 FD 的反向对照。不要独立调用这个会等待父门收尾的子夹具，也不要把它的 ignored 数当成安全门退出默认覆盖。

Linux claims 的 13 条 `root_*` 门需要 root，创建 `/var/tmp/polaris-claims-h-*` 独占目录；不写 `/run/polaris-sing-tun-claims`，不改网络。只在隔离 Linux 验证环境中用 root 身份执行父门：

```bash
POLARIS_NO_KERNEL_RUN=1 cargo test -p polaris-helper platform::linux::claims::tests::root_ -- --ignored --test-threads=1
```

`uid_worker` 只由 `root_peer_uids_with_original_ambient_caps_share_the_sticky_store` 起出的子测试进程执行；父门负责私有目录、环境、UID 与收尾，不单独运行 worker。

`native_core_lists_and_switches_the_two_compiled_modes` 是真实核 gRPC 契约，使用相同的 `kernel_run_or_skip` / `with_run` 双重守卫。经授权的真核验证才执行下面命令；本次源码修复未执行真核验收：

```bash
POLARIS_SINGBOX_PATH=/绝对路径/sing-box cargo test -p polaris-singbox-grpc --test native_clash_mode -- --ignored --exact native_core_lists_and_switches_the_two_compiled_modes
```

## 平台差异：`ignored` 的数字是平台相关的

源码登记共 35 条。Linux claims 与 exact-spawn 私有夹具、Windows 路由和 macOS 系统代理门各有平台 cfg；运行时 `ignored` 数量按编译目标与过滤器变化。源码登记数是平台无关的事实，不用某个平台的运行时报告数充当其它平台的验收收据。
