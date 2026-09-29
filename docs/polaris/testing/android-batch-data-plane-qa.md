---
title: Android batch QA prepared-only evidence foundation
area: polaris
created: 2026-09-30
updated: 2026-09-30
status: active
type: runbook
---

此版本是 **prepared-only，不能执行真机批次或签收 P5/P8**。受限 runtime-policy、per-app VPN、VPN underlying netId/transport、MIUI run-as/app_process、独立进程/listener/forward 清理观察器尚未完成。即使所有输入 fixture 完整且一致，`assess` 也只返回 `NOT_READY`、`claims:[]`、退出码 `2`；没有 PASS 分支。用户在场或填写批准记录均不能解除该限制。

`scripts/android-batch-witness.mjs` 不包含 socket 实现。`--execute`、`serve/probe/androidArgs` 和 `requireNextStage` 从代码层拒绝执行；不会生成可执行 Android argv。Java companion 已收口为**无 socket 的私有 artifact verifier**，没有 echo server/probe/VPN/policy 操作，拒绝 `--execute`。当前只有本地编译、文件 hash、离线 receipt 检查和受限包/进程观察入口。

本轮未执行真实 ADB、app_process、设备或网络操作。Mock 检查与 Java/dex 编译只证明此准备代码的局部性质，不证明 Android、MIUI 或 mesh 数据面。

## 已保留的安全基础

- 每个 phase 必须有 `beforeSnapshot` 和 `afterSnapshot`。两次观察除 host `observedAtMs` 外完全相等，覆盖配置、实际 runtime policy、per-app VPN、物理 transport/handle、identity、PC baseline、selection、pending changes、开关和 app PID。任一漂移拒绝记录。
- 四组探测按顺序执行；时间关系必须是 `phase.before < receiver.before < probe.start <= probe.end < receiver.after < phase.after`。不能用结束时的好状态替代发包前证据。所有时间必须由将来的同一个受限 host observer 记录，caller 自填时间不具有观察权威。
- 双向 TCP/UDP 独立 nonce challenge、发送端精确回显、接收端 unique counters、跨 phase 连续性。负例还需要已生效的 block policy、同期本地健康、出站正例，以及前后入站正例；路由/权限/解析错误不能算拒绝成功。
- immutable plan 固定 source/编译工具/platform/class jar/DEX hash 和 build receipt hash。每个 receipt/counter 固定 DEX、private manifest、build receipt 三个 hash。复制后的 manifest/DEX 必须用 app UID 的 run-as hash 对拍。
- private manifest 是**不含外层 planSha256 的 canonical plan body 原始字节**，无换行；它的 SHA 就是 `planSha256`。Java verifier 从 app-owned `0700` nonce cache 读取两个 app-owned 只读文件，自核完整 manifest hash、nonce、Debug package、build receipt hash 和实际 jar hash，不能仅回显传入 digest。
- 批准 scope 分为 read-only、mutation、Wi-Fi、cleanup；含独立 presence checkpoint，超过 60 秒拒绝。该记录仍是 operator 自述，不证明连续用户在场，因此相关 observer 仍在 missing 列表中。
- PC counter 路径只能是当前 operator 拥有的真实 `0700` QA root 内、新的 `pc-counters-NONCE.json`；拒绝 symlink root、任意路径和旧计数文件。当前没有创建 counter/socket 的运行实现。
- 清理不接受 `witnessesStopped/forwardsRemoved/artifactsRemoved` 这类 caller 布尔值。缺少独立 nonce-owned PID/start token、listener、counter、artifact 和本次 ADB forward 观察时，`cleanupEvidence` 只能为 `NOT_OBSERVED`；`abort` 不会声明 restored。

## 本地静态构建与 artifact binding

在新建、权限 `0700` 的本次 QA root 中编译。显式指定已审核的 JDK21、Android platform jar 和 d8；不构建或安装 APK。

```bash
mkdir -p "$QA_DIR/classes"
"$QA_JAVA_HOME/bin/javac" --release 8 -classpath "$QA_ANDROID_JAR" -d "$QA_DIR/classes" scripts/qa/MeshWitness.java
"$QA_JAVA_HOME/bin/jar" --create --file "$QA_DIR/witness-classes.jar" -C "$QA_DIR/classes" polaris
JAVA_HOME="$QA_JAVA_HOME" "$QA_D8" --min-api 24 --lib "$QA_ANDROID_JAR" --output "$QA_DIR/witness.dex.jar" "$QA_DIR/witness-classes.jar"
node scripts/android-batch-qa.mjs build-receipt scripts/qa/MeshWitness.java "$QA_JAVA_HOME/bin/javac" "$QA_D8" "$QA_ANDROID_JAR" "$QA_DIR/witness-classes.jar" "$QA_DIR/witness.dex.jar" "$QA_DIR/witness-build.json"
```

build receipt 是具体文件的 hash 记录，不是可重复/密闭构建的认证。本轮本机静态编译的 hash 收据保存在同目录 `android-batch-witness-static-build-2026-09-30.json`；它不包含真机执行证据。所有输出不得覆盖；只保留审核需要的小产物和 receipt。本准备脚本不上传源码、jar、配置或身份。

plan input 固定为 `schemaVersion:1`、明确 Debug/Release package、serial SHA、已核 APK SHA/source HEAD、既有 selected target 的 ID SHA、两端观察到的 IPv4、两个高位 TCP/UDP 端口、`attempts`、`timeoutMs`、absolute `qaRoot`、完整 `witnessBuild` receipt 和基线 snapshot。不存在可用的现场基线适配器，因此当前 plan 只能由明确标识的离线 fixture 准备；不得把 fixture 当现场状态。

```bash
node scripts/android-batch-qa.mjs plan "$QA_DIR/offline-input.json" "$QA_DIR/plan.json"
node scripts/android-batch-qa.mjs private-manifest "$QA_DIR/plan.json" "$QA_DIR/manifest.json"
node scripts/android-batch-witness.mjs verify-artifacts "$QA_DIR/plan.json" "$QA_DIR/manifest.json" "$QA_DIR/witness.dex.jar"
```

private manifest 输出为 `0444`，无换行；修改 whitespace 也会破坏 hash。`verify-artifacts` 在本地比对精确字节与 DEX，只返回 `NOT_READY`，不使真机执行可用。

未来复制适配器必须在批准的 artifact-copy scope 内，将二者复制到 `run-as com.polaris2.app.debug` 的 `cache/polaris-qa-NONCE/{manifest.json,witness.dex.jar}`，root `0700`、文件无写权限；从**同一 app UID**读取两条 SHA 后调用 `verifyCopiedHashOutput`。该函数要求精确两个 nonce 路径和两个 pinned hash。Java `verify NONCE PLAN_SHA MANIFEST_PATH JAR_PATH` 只核 artifact；它不证明 per-app VPN/MIUI/socket 能力。当前没有执行它的 device adapter/命令生成入口，不能靠手工绕过 prepared-only 闸门。

## 当前可用的受限只读观察

如另有用户对具体设备观察的授权，批准文件必须绑定本计划，`scopes:['device-observe']`，实际 `issuedAtMs/expiresAtMs/presenceConfirmedAtMs` 与 `approvedByOperator:true`。最长一小时，presence checkpoint 最长 60 秒；不自动生成批准记录。

```bash
node scripts/android-batch-qa.mjs preflight "$QA_DIR/plan.json" "$QA_DIR/read-only-approval.json" "$QA_SERIAL" --read-device
```

该入口只允许 `get-state`、`pm list packages`、`pidof`，要求 app 已在运行；不连接/配对、不起 Activity、不 forward、不 `config_get`、不创建 socket。它始终返回 `NOT_READY`，只能证明包和进程存在。`config_get` 的 startup maintenance 与现有 CDP helper 的 Activity/forward 副作用不属于这个只读入口。

不能用通知栏图标、`wifi_on`、活跃的某条 WIFI network 或 VPN interface 推断 VPN 实际 underlying network。不能用保存的 policy hash推断实际 runtime policy，也不能用 app UID 的预期值代替实际 per-app VPN 作用域。对应受限观察器完成、真机无 socket preflight验证并独立复审之前，不增加 `executable:true` 配置开关。

## 离线 receipt 格式与未来单批次顺序

顺序固定为 `baseline → wifi-positive → policy-negative → policy-restored → cellular-positive → wifi-return → cleanup`。每个 phase 的 `beforeSnapshot/afterSnapshot` 含相同事实字段和各自 host `observedAtMs`；每个 probe 包含 receiver `before/after`、`startedAtMs/finishedAtMs`、四组方向/协议之一的 ACK/outcomes。完整 receipt 顶层还必须含 `{dexSha256,manifestSha256,buildReceiptSha256}` 的 `witness` 对象。

`baseline` 与 plan 的不可变事实一致。活动 phase 的 actual runtime policy 必须与对应 allow/block policy一致，per-app VPN/identity/PC/selection/pending等保持基线；cellular phase 必须是不同 physical handle，Wi-Fi return 不再使用 cellular handle。任何双快照漂移、计数越界、counter重置、旧nonce、缺失或重复阶段会拒绝。

每个 counter 必须记录 pinned 三个 artifact hash和由未来同一 host observer捕获的 `observedAtMs`。时间戳和 hash本身不认证数据来源；目前 `WitnessLedger` 明确只用于离线 mock，不能承担 receiver实测。

`cleanup` 只接受结构化 `cleanupEvidence:{verdict:'NOT_OBSERVED',missingObservers:plan.readiness.missingObservers}`。即使 snapshot 的配置/身份/hash与基线一致，也不会认证 PID/listener/forward 清理。未来的清理观察必须区分 nonce-owned PID与PID复用，独立核实进程退出、端口listener、counter状态、精确artifact缺席和本次forward缺席，不依赖caller填布尔值。

```bash
node scripts/android-batch-qa.mjs assess "$QA_DIR/plan.json" "$QA_DIR/offline-receipt.json"
node scripts/android-batch-qa.mjs record "$QA_DIR/plan.json" "$QA_DIR/prior-offline-receipt.json" "$QA_DIR/offline-event.json" "$QA_DIR/new-offline-receipt.json"
node scripts/android-batch-qa.mjs abort "$QA_DIR/plan.json" "$QA_DIR/last-accepted-offline.json" "$QA_DIR/offline-cleanup.json" "$QA_DIR/offline-abort.json"
```

结构合法也只返回 `NOT_READY`/退出 `2`；结构/hash/顺序错误退出 `1`。不能将 `2` 当实际通过，不能在 receipt/report 中写 P5/P8、WARP、DNS、IPv6 或 restored 通过。

后续真实批次仍需用户持有手机、USB ADB、已登录且已选中的现有 Headscale 节点、手机初始VPN停止、Wi-Fi已连接/移动数据已开、现有可观察 PC mesh route/入站授权、两个空闲端口和独占配置编辑。现有 `server_switch`会改MRU，`server_update`无CAS，userspace-only SOCKS PC不能用普通Node direct socket作证；这些条件需真实adapter验证，当前不处理身份、PC route/policy或账号变更。

下一步是独立的可信观察器与设备无 socket readiness任务；完成后重新设计执行runner、cleanup receipt和用户在场控制，并再次审查。当前准备提交不能因为用户到场而自动升级。
