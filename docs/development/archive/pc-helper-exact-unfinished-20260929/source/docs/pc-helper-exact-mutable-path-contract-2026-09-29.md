---
status: draft
updated: 2026-09-29
scope: pc-helper-exact-start
authority: review-only; shared design SoT remains ~/docs/polaris/design/polaris-multi-ts-apply-contract-2026-09-29.md
---

# PC exact-start：可写路径所有权与最小分批边界

本稿是可执行接口草案，不授权启用 `StartExact`。目前 helper 的私有快照只闭合配置、plan、manifest、本地 rule-set、可执行文件及 Cronet sidecar 的**只读字节**；`Started` 将来只证明这些启动输入与一个真实进程实例。平台接管仍须独立 ready/OS ACK。正常生成配置目前恒带 cache path，TS 配置再带 state/Taildrop path，所以现有 wire 继续返回 `exact-unsupported`。

## 两类对象，不混用摘要

| 类别 | 当前生成源 | 归属与证据 |
|---|---|---|
| 只读配置/plan/rule-set | `builder/generate.rs`、`builder/route.rs`，经 `mesh_apply/artifact.rs` manifest | helper 从同一受检对象复制到私有目录，重写路径；回执含源配置 SHA、实际启动配置 SHA、版本化闭包 digest。源文件在复制后变化不影响启动字节。 |
| 核与 Cronet sidecar | 受保护 coreDir；Linux `libcronet.so`、Windows `libcronet.dll`，macOS 静态集成 | helper 从已验证的受保护对象复制并记录存在性与摘要；损坏 symlink/不可读为 Unsupported，不折成 absent。实际 spawn 仅指向副本。Linux/Windows 还须证明动态加载器取**这个副本**（搜索路径、环境、已加载模块对象），仅把 DLL/so 放在核旁不足以证明。 |
| `experimental.cache_file.path` | `builder/generate.rs:549` 恒发射 `<userData>/cache.db` | bbolt 可读写、清理时可能 Remove+reopen（固定核 `experimental/cachefile/cache.go:173-201,281-289`）。必须有可写文件**和目录**，不能只给文件写权限。其内容是运行态，不进入 immutable bytes digest。 |
| `endpoints[].state_directory`/`taildrop_directory` | `builder/endpoints.rs:355,383` | TS 身份和收件目录，按 `serverId+identityEpoch` 持久归属；绝不每次起核建空状态。主核与临时登录核共享旧状态的既有 gate 必须一并转移。 |
| 运行期热更的 local rule-set | `proxy/startup.rs::sync_custom_rule_files`、`proxy/tailnet_rules.rs::sync_tailnet_rule_files` | custom 规则可由 app 热改，tailnet 规则随 TS STATUS 帧改变。把起核时字节复制为只读快照会切断这两条现有更新腿；需将动态文件单列 helper 私有可写 owner/lease，并把每次更新绑定 runRef/plan generation，或显式改为停核后新 plan 重启。不能以启动快照声称运行期规则固定。 |
| 其它本地输入 | `dns.servers[type=hosts].path`、`outbounds[].private_key_path`、dashboard 目录、Tor/plugin 等 | 未进入 manifest 并无受检变换前均 Unsupported；HTTP/DoH `path` 是 URL 路径，不能当文件。未知字段、custom passthrough、未知类型同样 Unsupported。 |

语义路径检查不能只找显式 `path` 键：`services[].dashboard` 即使 `path=None`，核也可能按 CWD 取得或下载资源，当前 helper 一律拒绝 dashboard；`log.output` 常随 TUN 配置出现，是可写日志路径，当前同样拒绝。后续接受它们前要先定义默认路径、CWD、owner 和落盘副作用。H1 只允许这组逐项审过的空状态子集，**普通生成配置覆盖率仍为 0%**；它不是 managed TS 的可用入口。

新 manifest 需显式区分 `readOnlyFiles` 与 `mutablePaths`，每条可写 claim 至少带 `role`、规范化源路径、受保护目标相对路径、`ownerRef`（cache 为安装级 owner；TS 为 `serverId+identityEpoch`；动态规则为 plan/run owner）、`runRef`、请求 `generation`、迁移来源摘要或「源确实缺席」证据、`disposition`。plan/manifest digest 绑定**路径和预期 owner**，不冒充运行中内容摘要；helper 回执另外报告每条目标对象身份/已领取的 lease。`StartRequested` 持久成功及旧实例确证退出后，helper 才能领取新 lease。Stop/崩溃恢复按真实 runRef 释放，不能按 PID 或新请求 generation 猜。

任何平台开放 `StartExact` 前，helper 都必须在路径重写和可写 owner 绑定**之后**，从最终私有路径启动同一份 pinned core 副本做严格 check；check 的输入 SHA/closure identity 与 spawn/Started 一致。`check` 只验证核接受配置，不能代替进程出生证明、ready 或 OS ACK。失败、超时或无法证明同一对象均返回 Unsupported/明确失败，零 spawn。当前代码仅 stage，未接此 check，wire 因而保持拒绝。

## Cache 首个可用闭环（先覆盖正常非 TS）

1. 在同一 lifecycle/TS gate 预检只读输入，旧核尚在时不迁移 bbolt。新增 manifest claim 记录旧 `cache.db` 位置、安装级 cache owner 与候选 runRef；配置源 SHA 仍对应**未重写**的 config。缓存内容的 SHA 独立记录。
2. 持 gate 确认旧主核和任何持该 cache 的孤儿均已退出，阻止 app 的 cache 删除/重置入口。helper 以 no-follow、受限路径读取旧 cache：存在则复制相同字节、核对对象身份/长度/修改时间及前后 SHA；缺席则显式记录 absent。任何读/竞争/超预算/不确定都保持 stopped+Unsupported，不能退回空缓存。源文件留存供故障恢复。
3. helper 在受保护父目录下建立稳定 cache owner 目录及本次 lease。Linux：父目录 root-owned、给降权 UID 仅 search ACL；bbolt 目录由核 UID 可写，且 app 正常代码从不使用该私有路径。同 UID 恶意进程不在既定信任边界。macOS root、Windows SYSTEM 用各自保护目录/DACL。bbolt 的 Remove+reopen 要求这个目录可写，不能只预建 `cache.db` 并给它 `rw`。
4. 重写 `cache_file.path` 到私有目标，使用**同一份受保护的最终配置和同一受保护核副本**执行严格 `sing-box check -c <launched-config>`；仅零退出、未超时才可进入真正 spawn。记录 check 所用的 launched-config SHA、核 SHA、closure digest，与随后 `Started` 的三者逐项相等；源配置 preflight 成功不能替代重写后 check，当前 `core-supervisor` 的 fail-open verdict 也不能直接当 helper 严格判定。回执里区分 cache lease 与只读输入。复用后续世代的同一私有 cache，不每轮重置 FakeIP/DNS/selector 持久态。首次迁移与 lease 交接必须耐崩溃：迁移标记持久化优先于启动；恢复时先核旧/新 run 是否存活，不得二次复制覆盖已经被新核写过的 cache。
5. `Started` 后仍需 core-ready、selector 校正及平台 ACK；cache 旧 `selected` bucket 可覆盖配置默认出口，见 `runtime/proxy/hot_switch.rs:1901-1907`。迁移成功只证明旧字节被保存，不证明路由正确。失败分支保留旧 cache 和私有候选，清理仅在确切实例退出后执行。

### H2a 离线账本切片（本工作树）

`helper/src/exact_cache.rs` 只实现可序列化、严格 canonical 的 **v3 状态值与纯转移**，未接 helper wire、没有磁盘写入/核检查/进程启动。`CacheClaim` 携带安装级 `localId+ownerRef`、本轮 `intentRevision+bootId+lifecycleGeneration`、作为 `candidateRunId` 的 `runRef`、必填且不同于候选的 `stopTargetRunRef`、源路径/配置 SHA、planId/plan digest、manifest 引用及原字节 SHA。初始 NoOldCore 路径仍 Unsupported；不能把空旧 runRef 当成无旧核证明。H1 `exact_snapshot::stage` 对 `manifest.json` 原始 bytes 求 SHA 并比对 `ExactBinding.artifact_digest`，H2 显式要求 `artifactManifestSha256 == artifactDigest`；ref 必须是 B3 的 `mesh-routes/plans/<planId>/manifest.json`。这只固定字段语义，**不认证 claim** 或证明 manifest 对象。`localId` 在 journal 顶层持久且转移不变，per-intent 字段仅随本轮 phase claim 保存；Idle 不代表某个候选。配置 SHA 改变不重建 cache owner。私有 `FreshCacheOwner` 在初迁时还绑定经未来认证 claim 确认的 `ownerUid`，账本持久保存该 UID 且所有后续阶段不变；旧 v1/v2 JSON 直接拒绝，不隐式迁移。源缓存观察值只能是 `Present{sha256,bytes,objectKey}` 或经过父目录对象核验的 `Absent{parentKey}`。`Absent` 与读取失败是不同类型，不能互换。

| 持久阶段 | 允许的下一步 | 崩溃重读时 |
|---|---|---|
| `migration_pending` | 持匹配的新停核 fence，复核源前后同一对象/摘要、候选副本同字节或双侧确实缺席，再转 `idle` | 阻断自动重试；不得覆盖已有私有 owner。 |
| `idle` | 复核私有 owner 目录对象与本轮停核 fence，写 `reserved` | 仅此阶段可在复核后复用旧 cache；旧文件的字节摘要是迁移历史，不是运行期不变量。 |
| `reserved` | 用 pinned 核/重写后 config 做严格 check，成功后写 `checked` | 不知道 check/核是否已触碰 cache，阻断自动恢复。 |
| `checked` | 真正 spawn；helper 内部 `ObservedStarted` 须把 lifecycleGeneration、独立 OS 观察的 daemon birth/PID/process birth 与双 SHA/closure 回执逐项对齐，才写 `active` | 可能已经 spawn 但回执未持久，阻断自动恢复。 |
| `active` | 只凭匹配 daemon birth、PID、process birth、runRef/generation 的确切退出证明回 `idle` | 未证实退出时阻断；PID 或新请求世代不构成释放证据。 |

公开 `ExactReceipt` 不含 lifecycleGeneration，且目前没有持久证据证明 runRef 全局唯一。账本因此持久记录**全部已领取 runRef**，A→B→A 即使换 generation 也拒绝；有界容量为 4096，达到上限返回 `RunRefCapacity`，未来 wire 必须映射为 Unsupported，不能删历史或静默轮转。这只是离线防回放上限，**不是可长期运行的容量契约**：生产开放前需由核心裁决 runRef 的持久唯一性证明、跨安装/恢复边界与安全轮转方案，并把它绑定 journal。`record_started` 仅 crate 内可见且只收无生产构造器的 `ObservedStarted`；公开 DTO 不能直接激活 lease。

纯模块中的「journal 与私有 owner 均不存在」首迁证明、停核 fence、退出证明、私有 owner 观察和严格 check 成功对象类型没有生产构造器，防止当前代码凭 caller 的布尔值直接越过闸门。未来平台 adapter 必须持 lifecycle/TS gate，在所有旧 cache 使用者确证退出后签发 fence，并实现 no-follow 对象检查、迁移复制、受保护父目录、按上一份 canonical journal 字节做 CAS 的原子写新、文件及父目录 fsync、失败注入和重读。`migration_pending` 必须先持久化才可复制；`reserved` 必须先持久化才可运行最终 check；`checked` 必须先持久化才可 spawn。若旧 app cache 的并发删除/写入入口仍可能穿过 gate，就不能签发 fence。当前只实现这些转移的确定性验证，**不宣称已经完成安全迁移或严格 check**。

### H2b POSIX 持久化切片（本工作树，仍未接线）

`helper/src/exact_cache/store.rs` 为 Linux/macOS 提供独立的 `CacheJournalStore`；它只持久化 H2a 值，不签发任何生命周期、核检查或进程证明。入口要求 helper 为 root，从 `/` 起逐级 `openat(O_DIRECTORY|O_NOFOLLOW|O_CLOEXEC)` 固定 root-owned 且无非 root 写权限的目录链；最终目录要求 root 可读写搜索，其他主体最多只有搜索权，以便 Linux 降权核将来访问其下独立的 UID 可写 `cache-owner`。账本和 lock 文件仍为单链接、root-owned、0600 等价的私有普通文件。这个目录必须在 app 可写树外由平台适配器预置；本切片不创建它，也不决定 UID ACL。

`CacheJournal` 对外只可序列化已取得的状态值，**不实现公开 `Deserialize`**；磁盘重读经模块私有 DTO 和私有 canonical parser，随后逐字段验证。外部 crate 不能用 JSON 合成 `checked`/`active`，也不能直接调用 parser 绕开 Fresh/Stopped/ObservedStarted 等证据类型；编译拒绝用例覆盖这两条 API。store 的 `compare_and_replace` 也仅 helper crate 可见；持久 CAS 仍只核验状态形状和字节，不签发这些证据。`load_diagnostic` 只返回 ownerRef、持久 ownerUid 与阶段名，不返回 Journal 或对象证明。

跨进程 CAS 准入是固定名称且**永不删除**的 `cache-journal-v1.lock`：每个 writer 以 `O_NOFOLLOW|O_CLOEXEC` 打开、核验打开对象与目录项同一 dev/inode，持有非阻塞独占 `flock` 直到 store 释放；第二个 helper 进程拿不到锁即拒绝。Rust 写接口要求 `&mut self`，同一个 store 也不能并发写。这里依赖所有拥有该 root 私有目录写权限的 helper 适配器都遵守此锁，且目录位于本地、支持预期 `flock`/rename/fsync 语义的文件系统；网络文件系统或语义未验证的 volume 继续 Unsupported。若未来把目录移到 app 可写位置、清理/替换 lock 或引入绕锁 writer，POSIX rename 不再构成 CAS，必须继续 Unsupported。子进程 `exec` 不继承锁 fd；daemon 异常退出由 OS 释放锁。

每次写先在持锁状态下重读并逐字节比对上一份 canonical journal；初始写仅允许 `migration_pending` 且固定 `cache-owner` 项不存在。随后只接受 H2a 邻接阶段和稳定 owner/source/history 的**形状**，证据本身仍须由未来平台 adapter 签发。写新固定 `.next` 时使用 `O_CREAT|O_EXCL|O_NOFOLLOW`，完整写入并 `fsync` 文件，再同目录 `renameat` 覆盖并 `fsync` 目录；任何 rename 前失败留下 `.next`，下次 open/load 必须拒绝，不自动删除或重放。rename 后目录 fsync 失败返回 `DurabilityUnknown`，调用方不得做下一项 effect；恢复可能看到旧或新 canonical 阶段，只有 `idle` 可进入后续 owner+停核复核，其余阶段阻断自动恢复。坏 JSON、非 canonical、超预算、symlink/hardlink、私有 owner 存在但账本缺失均拒绝。`load_idle_for_recovery` 还要求固定 owner 项是实际目录并与 journal 中 owner_key 匹配；它本身**不授予 lease**，仍需 H2a 的停核 fence。

### H2c 真实 cache-owner 目录观察（本工作树，仍未接线）

在 H2b 已持有的 root-owned 父目录 fd 和独占 `flock` 下，store 对固定 `cache-owner` 做 `fstatat(AT_SYMLINK_NOFOLLOW)`、`openat(O_DIRECTORY|O_NOFOLLOW)`、已开 fd 的 `fstat` 和目录项再查；前后 dev/ino 必须相同，实际对象必须是期望 UID 拥有的 `0700` 目录。`OwnerDirectory<'store>` 保持 fd 打开，记住父目录 dev/ino，其生命周期借用持锁 store，不能先释放锁再使用旧观察值；恢复阶段再次核对目录项与打开对象。`owner_key=unix-uid-<uid>-dev-<dev>-ino-<ino>` 来自该真实目录而非 marker：账本持久的 `ownerUid`、该 key 内 UID、真实 inode UID 与未来认证 claim 的 UID 必须一致。任何可进入 phase 的 `load_for_phase`、publish、idle recovery 都显式要求字段私有的 `AuthenticatedOwnerUid{ownerRef,uid}`；当前没有生产构造器，不能从现时 `stat.st_uid` 自证。写入任何非 `migration_pending` journal 前、phase 重读含 owner_key 的 journal 时均用该 UID 核对 key；`idle` 恢复再核对权限和对象身份。普通文件、符号链接、权限/UID 错误、同 inode chown、旧目录被替换、journal 中的伪造 key 即使 JSON canonical 也拒绝；定向测试包含这些负例。

此观察仅证明**当前目录对象**。期望 UID 目前没有可信生产来源：未来 auth adapter 必须在 lifecycle gate 下把已认证 peer 与计划 mutable claim 绑定后才可签发不透明 token；所有会改动 `cache-owner` 目录项的 helper 操作也必须遵守同一稳定 lock。H2c 没有创建 owner、迁移旧缓存、签发 `StoppedCacheFence`、授权 lease 或运行 strict final check。跨进程/崩溃后的 dev/ino 复用还依赖 root-owned 父目录的生命周期规则：helper 不得在 journal 存活时删除并重建 owner；若要支持清理/重建，需额外持久 owner 身份代，而不能只凭 inode 相等启用。这些缺口未裁决前全平台 `StartExact` 保持 Unsupported。

此切片把固定 `cache-owner` 视为**未来私有 cache 目录的占位路径契约**；若平台最终布局不是该目录项，必须先改为同一受保护对象的检查，不能以 marker 文件替代真实 owner 存在性。崩溃后 `.next` 的安全清理/人工隔离流程、真实旧 cache 文件迁移和首拷贝校验、旧实例与孤儿核确证退出、app 的 reset/delete gate、最终私有 config 对同一 pinned core 的 strict check、OS child birth/ACK、runRef 持久唯一性与轮转、Mac 实机 fsync/权限验证都未实现。全平台 `StartExact` 继续 `exact-unsupported`；H2b 不改变普通配置覆盖率。

### H2d 旧核 Stop 与迁移围栏审计（仅契约；尚无生产准入）

现有路径中，`mesh_apply.rs::StopReservationReceipt` 只证明持久 `StopRequested` 指向当时的 `stopTargetRunRef`，其 `matches_current_direct_run` 只认 app 手中的同一个 `DirectCoreRun`；`PhaseEvent::OldStopped { exited, owners_released }` 仍由调用方给布尔值。`proxy/lifecycle.rs::stop_inner` 持 app 进程内的 TS state gate、检查 lifecycle generation，并等待 `kill_core`；直起核的 `Child::wait` 能收割该句柄，helper 腿的既有 `stop_managed_core(pid)` 只给普通 stop 回执。三平台 `StopExact` 均只返回 `StopRequested`：Linux 在 handler 中摘 child 后发 terminate，macOS 的 `do_terminate` 后台收割，Windows 的 `reap_exact_child` 持进程句柄后台终止/等待。摘除受管槽或随后 `StatusExact=Unknown` **不是**同一进程的退出证明。启动前 `cleanup_stale_cores` 的字面 binary+`run` 扫描和 PID 存活复查用于减少遗留孤儿，但解析失败会跳过、Windows 扫描为空，PID 还可能复用；它不是「所有旧 cache 使用者已退出」的全集证明。上述任一回执都不能构造 `StoppedCacheFence`。

旧 `<userData>/cache.db` 的当前明确写入口是：`config-engine/builder/generate.rs` 与 `proxy/startup.rs` 把同一路径发给普通主核，核运行期间写 bbolt 且可 Remove+reopen；macOS legacy helper 的 `prepare_cache_for_user` 在起核前 `create`/`fchown` 该文件；`runtime/uninstall.rs::remove_user_config` 会删除包含它的整个用户配置目录。检索当前 Rust app 未发现独立的 `cache.db` reset/delete 命令，不能据此认为用户数据目录受保护。Tailscale 登录临时核使用自己的 `login-cache-<port>.db`，但它的 state owner 仍须由 TS gate 结算；测速临时核及任何自定义配置是否引用主 cache 必须以实际生成配置/启动登记核对，不能从进程名推断。未来所有主核 start/restart/recovery/autostart、legacy helper start、macOS cache 准备、卸载目录删除及新增 cache 写/删入口均须经过相同的持久迁移准入；仍存活的旧核本身也必须先确证退出。仅在迁移时锁住 helper journal，挡不住 app 随后重开旧路径。

跨进程不传递 Rust `MutexGuard`，而传递**持久代际授权**：app 协调器持现有 TS async gate，按共享 SoT 的锁序短暂取 lifecycle/config 锁，先在 B3 的同一 `config.json` 账本持久记录 `StopRequested`、确切旧 runRef 与请求 generation，并使 managed marker/legacy-start admission 阻断旧入口；CAS 成功后仍持 TS gate 发 exact Stop。cache owner/source path、manifest 摘要与 former-user 全集仍需从同一受检产物和受管运行事实取得，不能假称现有 `MeshTransaction` 已存下这些字段。helper 用认证 peer、重读的受管 claim 与同代 token 准入，独占 H2b 的 root-owned journal lock，持续记账 `migration_pending`；同步锁内不 await/IPC。app 在 Stop、退出确认、源观察与迁移交接完成前不放行任何指向旧 cache 的新 start 或删除。进程崩溃时内存 TS gate 会消失，因此新 app 必须先从持久 marker/账本和 helper journal 做 reconcile：未结算的 Stop/迁移一律拒绝旧路径新 start、源删除和再次拷贝。`LegacyStartLease` 目前只是 app 内存计数与 marker 发布门，不能独自充当跨进程迁移锁；`StopRequested` CAS 失败或代被新 Stop 抢占均不得给 helper 授权。

退出与全集证据须拆开：在 stop **之前**按确切 runRef 记录并钉住旧主核的 OS process birth/句柄及其实际 cache path；直起 `Child`、helper-managed child、可能引用主 cache 的临时核逐个登记为 former user。helper 的 StopRequested 仅触发动作，保留原进程对象到 reaper/OS 确认退出，再给带 daemon birth、PID、process birth、runRef、原 generation 的退出回执；app 也须持 gate 核对全集没有在飞 spawn/未释放 owner，并对账原 Stop 预留、当下 generation 与账本 revision，最后才允许 helper 构造 `StoppedCacheFence`。对非 helper 直起核，app 的 `Arc<RunIdentity>`/`Child::wait` 是本进程证据，helper 若要迁移还须在 stop 前独立钉住并核对 OS 对象；仅传 PID、runRef 或 app 的 `exited:true` 不够。无法枚举/钉住的 crash orphan、旧版本未登记实例、未知 cache path、迟到 stop、超时、PID 复用、helper 重启后丢失 reaper 身份，均保持账本未结算并返回 Unsupported，不把 `NotRunning`、`Unknown` 或扫描空集当作退出证明。

平台证明各自收口：Linux 可在同一 child slot/reaper 上绑定 `/proc` starttime 或 pidfd，并保持 stop 前对象引用到退出，降权/孤儿枚举失败拒绝；macOS 既有 `done` 可等待 daemon 自己的 child，但 `ps` 路径/`lstart` 不能单独证明所有 root 孤儿且需真实设备核验对象生死；Windows 必须保留 `OwnedHandle` 和 creation time 到 `WaitForSingleObject` 确认退出，当前 exact stop 的后台线程丢弃等待结果，且 WMI 孤儿扫描尚未闭合。每个平台的旧 direct/helper 双路径与重启恢复都要独立验证；某平台不具备对象级证明时只让该平台 Unsupported。

此前审计限定的**最小观察对象**是 Linux 单一 helper 受管 child 的「stop 前钉住对象 → stop 请求 → reaper 确认同一对象退出」，加共享协调器只读 former-user 枚举/准入审计；它本身不产生 cache fence、迁移或 lease。下述裁决固定账本分工与更早的 join/旧入口门前置；此观察不能越过这些前置，也不能把 Linux 成功推及 macOS/Windows。

**核心方向裁决：双账本、单一语义作者。** B3 的 `meshRoutePolicy`、`meshRouteState.intent/transaction/identityEffects` 仍由 app 的同一 `config.json` 原子写者独占，作为用户意图、身份和旧入口准入的唯一真值；`mesh-route-state.required` 只负责 preparing/enabled 禁回 legacy，不承载 owner 或运行证据。helper 的 root 私有 H2 journal 只记受保护 cache 对象、迁移、lease 和确切退出效果，绝不另造 `desiredRun`、路由 owner 或可启动的 plan。这里的两份账本不是两个作者竞争同一份路由 journal，也没有跨两次 rename 的虚构原子提交。单用 app 账本无法独立保存/验证 helper 私有对象和崩溃中断的迁移效果；单用 helper 账本会拆开 B3 同文档的意图/身份事务，并使 app 在 helper 不可达时无法据本机受管状态拒绝旧起核。

两账本的 join 必须逐项绑定 `localId, intentRevision, bootId, lifecycleGeneration, planId/planDigest, artifactManifestRef+manifestDigest, sourceConfigSha256/sourcePathSha256, cacheOwnerRef+ownerUid, stopTargetRunRef, candidateRunId`。旧核退出前仅允许持久 `StopRequested` 和精确 Stop；确证全部 former user 退出且 app 写成 `OldStopped` 后，再持久 `StartRequested(candidateRunId)`，helper 才可为同一候选进入 `migration_pending → idle → reserved → checked → active`。初始本就无旧核时，`stopTargetRunRef` 的显式缺席必须来自可信 NoOldCore/全集证据，不能把字段缺失当证明。每项 helper 效果前后都复核当前 app claim、helper canonical journal 与真实对象；app 所有可撤销该 claim 的 writer 还须受同一跨进程准入围栏约束，不能把两次重读当成锁。任一方缺失、字段不符、`commitUncertain`、helper 不可达或重读失败，均不得启动、复拷或自动清理。app 账本旧阶段与 helper 较新阶段之间不回滚字节，只留现场并做 fail-closed reconcile；新 Stop/Apply 抢代后，旧候选不能凭仍在 helper journal 的 lease 继续 spawn。

**当前字段差距（未接线）**：B3 marker 只有 `phase/localId/legacyConfigDigest`，没有请求代；`MeshTransaction` 已有 `intentRevision/bootId/lifecycleGeneration/planId/planDigest/stopTargetRunRef/candidateRunId/artifactManifestRef`，却无 manifest 内容 digest、cache owner/source path/UID 或 former-user 全集证明；`stopTargetRunRef` 在 Stop 预留后存在，`candidateRunId` 到 Start 预留才写入。H2 `CacheClaim` 现已静态持有上述 typed join 字段，journal v3 可做离线逐字段相等检查，但**没有可信来源或生产适配器**；它不能自行验证 B3 原子文档、manifest 原始对象或旧进程退出。`ExactReceipt` 也无 lifecycleGeneration，不能单凭公开 DTO 连到 B3。manifest 尚无完整 mutable claim。join 字段须由共享作者扩展受检 manifest/typed claim，并由 helper 私有账本持久关联；不得在 PC 另造平行路由字段。受管 app 的 `StartRequested` 尚无生产协调器，现有任何 phase 值均非 helper 启动权。

helper 私有 journal/owner **一旦存续**，Linux/macOS/Windows legacy `START` 以及 macOS `prepare_cache_for_user` 都必须在实际副作用前查询并拒绝该安装身份，即使 app marker 丢失；app 的直接 legacy spawn、自动恢复、卸载旧目录也须有同等持久阻断。当前 helper 旧命令不查询 H2 journal，app 的 legacy admission 只核 marker/raw managed 文档，两边均未闭环。尤其 marker 与原 managed 文档一同丢失时，app 不能仅从空盘推断从未迁移；若无可用的 helper/独立持久墓碑来排除旧私有 owner，直接旧入口仍不可安全准入。preparing marker 的显式取消只在尚未有任何 helper effect 时成立，不能以 legacy digest 一致覆盖已发生的迁移。

**首切片边界**：先由共享作者冻结上述 join 的 typed claim/manifest 映射与 `StartRequested` 前置顺序，并在旧 app/helper 入口加持久拒绝门和错账恢复门；helper 只做 Linux 单个由自身受管、在 stop 前已持对象的 child 退出观察，回执不升格为 cache fence。旧 direct Child 今天仅在 app 内以 `Arc<RunIdentity>` 关联 `tokio::process::Child`，其 UUID/PID/`Child::wait` 没有跨进程对象转交；helper 不能从 `stopTargetRunRef`、app 布尔或重启后的 PID 证明同一进程退出。Linux 的 pidfd 转交、macOS/Windows 的等价对象绑定和 crash orphan 全集证明尚未实现，旧 direct 或无法钉住的旧核继续 Unsupported。此切片不创建 owner、不迁移旧 cache、不开放任何平台 `StartExact`，且不改 B3 现有路由所有权真值。

这一步还**不足以启用 managed Apply**：上游 `mesh_apply/closure.rs::config_rule_paths` 目前把 builtin SRS 和 remote rule-set 判为 `RuleSetUnsupported`，只有 materializer 纳入受检资源并使 D1 emitter 引用同一 manifest 后，helper 的 rule-set 快照才有完整候选可消费。remote rule-set、custom rule 文件、tailnet rule 文件的运行期更新也须明确冻结/禁用或单列动态 owner，不能把启动时副本称为整个运行期不变。

## TS 状态的独立交接

TS claim 以 `serverId+identityEpoch` 为稳定身份，不随请求 generation 重建。旧主核、临时登录核、迟到 stop/close 都释放后，才将旧 `state_directory` **整树**迁入受保护 owner；逐项拒绝 symlink/reparse、越界、特殊文件和超预算，记录来源树 digest 与持久迁移标记。已有私有 owner 时优先复核并复用，不从旧 app 树再复制覆盖。Taildrop 作为同一身份下的可写子目录，文件保留策略需单独核定；既有入站文件不能被当作可丢弃缓存。身份 epoch 删除/退出须先证明所有相关 run 和临时实例已释放，再按 epoch effect 清理；旧 serverId-only 延迟删除不可碰新 epoch。Linux 私有可写目录需要 UID ACL/属主策略，macOS root、Windows SYSTEM 需对象与 DACL 证明。此批需改共享状态协调，不能只改 helper。

## 最小分批与验证

| 批次 | 可交付证据 | 启用边界 |
|---|---|---|
| H1 当前 helper 基础 | typed 配置全字段 roundtrip；manifest/source hash；rule 路径重写；核/sidecar 私有复制；双 SHA/closure digest；三平台**不可达的 exact `handle_start` 旧 pathname 分支**删除（legacy `START` 保留） | wire 全平台 Unsupported；最终配置尚未经同一 pinned core 严格 check。 |
| H2 非 TS cache | 持久 mutable claim/lease、旧核退出后迁移、bbolt 可写目录、崩溃恢复；缺席/被替换/竞争/损坏/磁盘失败故障注入 | 仅在源 manifest 对所有只读资源闭合、cache lease 与真实 run 状态可核、平台启动对象可证明时开放非 TS exact-start。 |
| H3 TS state | identityEpoch owner 迁移与主核/临时核共同 gate、Taildrop 保留、迟到 stop/删除防越代 | 仅此后考虑 TS exact-start；仍需最终 core-ready/OS ACK 才可 Commit/称保护。 |
| H4 平台 | Linux 降权 UID 的只读快照 ACL + 私有可写目录；macOS root 私有路径；Windows SYSTEM ACL/reparse/句柄到 CreateProcess | 每平台独立开放，缺设备/对象证明继续 Unsupported。 |

验收以确定性故障注入为主：source/manifest/rule/sidecar 修改、symlink/reparse、空/破损 cache、旧 run 未退出、迁移中崩溃、重试、Stop 抢代、错误 epoch、PID 重用和磁盘写失败。观察指标至少记录拒绝原因、闭包 digest、cache/TS lease owner、启动实例出生凭据与最终 OS ACK；日志不得记录 authKey、TS state 或缓存字节。真实 Mac/Win 和双 Tailnet 数据面仍由各设备 owner 验收，本设计与本机单测不替代。
