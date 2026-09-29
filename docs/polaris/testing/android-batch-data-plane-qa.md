---
title: Android data-plane batch QA operator procedure
created: 2026-09-30
updated: 2026-09-30
status: prepared-not-device-tested
type: runbook
---

本脚本只准备下一次用户在场的**单批次**验收。本次实现没有执行 ADB、连接设备、创建 socket、切换 VPN、网络、策略或 PC peer。Mock 通过和 Java/dex 编译通过不代表真机通过。

入口：`scripts/android-batch-qa.mjs` 管理不可覆盖的计划、逐阶段回执和失败后恢复回执；`scripts/android-batch-witness.mjs` 提供 PC loopback witness、PC→Android 探测，以及经批准的 Android companion 参数；`scripts/qa/MeshWitness.java` 是可重新编译的 Android witness。Node 使用仓库当前 Debug/Release 包常量，不读取旧 `/tmp` interop 脚本的身份、序列号或私有会话。

## 现场前置：任一缺失即停止，先补准备条件

- 用户已批准本次具体设备和单批次范围，用户持有手机，能恢复 Wi-Fi。采用 USB ADB；经 Wi-Fi 的 ADB 会在 P8 中断。显式传序列号，不执行 `adb connect`、配对或安装 APK。
- 安装的包与本次唯一 APK 的 SHA-256、构建 source HEAD 已比对。本批次中途不换 APK。默认包为 `com.polaris2.app.debug`；Release 只支持计划和包预检，当前 companion 依赖 `run-as`，无法给 Release 真机作证。
- 手机已有 Headscale 登录身份、现有节点和分配的 IPv4，**已选中待测 Headscale 节点**，VPN 已停止、无 pending changes；Wi-Fi 已连接，移动数据已开启，飞行模式已关闭。身份缺失时不登录、不注册、不新建节点；交由另一个明确授权的准备步骤。
- PC 已有可用的同一 Headscale peer 和现有系统/TUN 路由；PC 端 loopback 服务可由其现有 mesh endpoint 的 `target:local` 入站访问。PC→Android 的普通 Node socket 必须确实通过既有 mesh 路径。只有 userspace SOCKS5 的 PC 不满足此 v1 witness 前置；不得使用直连物理地址替代，不得把 SOCKS 会话启动/改路由藏在脚本里。该情况先停止，另行实现/批准 TCP+UDP transport adapter。
- 两端拟用的高位 TCP/UDP 端口空闲，PC 已有入站授权允许本手机访问这两个端口；本流程不改 PC 策略、防火墙、peer、身份或 VPN。
- 能从 Android 本机 `dumpsys connectivity` 或已审核的 native network witness 中明确取得 **VPN 正在使用的 underlying 非 VPN 网络**的 transport 与 network handle/netId。VPN interface、通知栏图标、单纯 `wifi_on` 都不够。存在多个活跃物理网络而无法确定 underlying network 时，P8 停止并记录缺证；不得自行填入期望值。
- 本地 Node ≥24、JDK21、Android SDK d8；输入与回执在一个新建、权限 `0700` 的本地目录中。原始 config、authKey、state file、identity/key 只存在内存，禁止写入计划、事件、报告或分享内容。
- 只允许一个 operator，期间不并发编辑配置或刷新订阅。现有 `server_update` 没有 CAS/base-version 参数，读回检查不能消除两次 IPC 间的并发覆盖风险；若无法保证独占编辑，停止批次。

`server_switch` 会写后端 MRU history；因此本计划要求基线已选中待测节点，**批次内不调用它**。否则“只还原 selectedServerId”无法保证还原整个原始配置。手机号、Headscale URL、设备序列号和 node id 不写入示例或仓库。

## 1. 只读包/进程预检，随后批准基线采集

```bash
node scripts/android-batch-qa.mjs preflight "$QA_SERIAL" --read-device
```

该入口仅 `get-state`、`pm list packages`、`pidof`，不会起 Activity 或做 CDP forward，未运行的 app 将失败。需要现有 ADB daemon/授权连接；此命令不验证 VPN/Headscale、WARP、DNS 或网络数据面。

`connectAndroidWebView` 会 `am start` 和创建 ADB forward；`config_get` 首次可能执行 startup maintenance。因此应先取得用户对 attach/维护的明确授权，使用既有 `scripts/android-cdp.mjs` 连接当前 Debug 包，等待维护结束后再冻结基线，不能把这一段描述为只读原始磁盘取证。保存这次 helper 返回的 `close()`，最后只删除自己的 forward。

操作会话使用下列现有 API；每个响应都必须先验证 `success === true`，不得把错误响应投影成默认空配置。保留 `originalNode` 的完整内存副本用于最后恢复，**不落盘**。

```js
import { connectAndroidWebView, until } from './scripts/android-cdp.mjs';
import { digest, projectSnapshot, hasPendingChanges, requireNextStage,
  requireApproval, appendEvent, assessBatch, writePrivateJson } from './scripts/android-batch-qa.mjs';
const session = await connectAndroidWebView(QA_SERIAL, 'com.polaris2.app.debug');
const data = async (command, args = {}) => {
  const r = await session.invoke(command, args);
  if (r.success !== true) throw new Error(`IPC failed: ${command}`);
  return r.data;
};
const originalConfig = await data('config_get'); // stays in memory
const originalNode = structuredClone(originalConfig.servers.find(n => n.id === TARGET_ID));
if (!originalNode || originalNode.protocol !== 'tailscale' || originalConfig.selectedServerId !== TARGET_ID) {
  throw new Error('Existing selected Headscale endpoint required');
}
const capture = async () => projectSnapshot({
  config: await data('config_get'), targetId: TARGET_ID,
  identity: await readExistingIdentityInMemory(), // stable existing node ID/key/IP evidence, not backend running state
  pcState: await readExistingPcStateInMemory(), // existing config/peer/runtime/route snapshot; no witness counters
  status: await data('proxy_get_status'),
  pendingChanges: hasPendingChanges(await data('proxy_get_pending_changes')),
  appPid: Number(session.adb('shell', 'pidof', 'com.polaris2.app.debug').trim()),
  ...await readUnderlyingNetworkInMemory(),
  // returns actual physicalTransport, networkHandle, wifiEnabled, cellularEnabled, airplaneEnabled
});
```

`readExistingIdentityInMemory`、`readExistingPcStateInMemory` 和 `readUnderlyingNetworkInMemory` 是现场的**证据适配点**：来自用户当前已有状态，必须先完成它们，不能用常量/期望值填充。仓库没有跨 PC 系统和 OEM 的可靠现有通用接口；本脚本不声称自动发现这些事实。底层网络观察可只读执行 `adb -s SERIAL shell dumpsys connectivity`，在本地内存筛出 VPN underlying 的有效非 VPN network record；另外只读观察 `settings get global wifi_on/mobile_data/airplane_mode_on`。不把完整 dumpsys 输出写入回执。

用 `capture()` 创建 `baseline`。manifest 字段固定如下；SHA 一律通过代码算，`targetServerSha256 = digest(TARGET_ID)`、`serialSha256 = sha256(QA_SERIAL)`。现有 config fingerprint、身份和 PC 状态的 SHA 来自 `projectSnapshot`。

```json
{
  "schemaVersion": 1,
  "package": "com.polaris2.app.debug",
  "serialSha256": "<computed 64 lowercase hex>",
  "sourceHead": "<verified 40 lowercase hex>",
  "apkSha256": "<verified installed base APK hash>",
  "targetServerSha256": "<digest of existing selected server id>",
  "pcMeshIp": "<observed PC IPv4>",
  "androidMeshIp": "<observed Android IPv4>",
  "tcpPort": 40101,
  "udpPort": 40102,
  "attempts": 3,
  "timeoutMs": 1500,
  "baseline": "<capture() object, not a string in the real file>"
}
```

IP 和端口必须来自本次准备，例中的端口只是示例；v1 只支持 IPv4。`plan` 生成随机 192-bit nonce，并绑定完整基线和精确 allowlist。不可覆盖旧文件、复用旧 nonce 或手改计划。

```bash
node scripts/android-batch-qa.mjs plan "$QA_DIR/manifest.json" "$QA_DIR/plan.json"
```

## 2. 审批具体计划，准备 witness

用户看过 plan 中动作、两个端口、包和恢复范围并明确批准后，operator 本地写入 `approval.json`（0600），固定字段为 `schemaVersion:1`、`planSha256`、`approvedByOperator:true`、`scopes:plan.scopes`、实际 `issuedAtMs` 与 `expiresAtMs`。最长一小时，不允许未来签发时间。这个记录是操作闸门，不是签名/身份认证，也不能替代用户批准。脚本不自动生成“已批准”记录。

纯本地编译 companion，可在现场前完成；输出目录必须是新的本次目录，不构建/安装 APK：

```bash
mkdir -p "$QA_DIR/classes"
"$QA_JAVA_HOME/bin/javac" --release 8 -d "$QA_DIR/classes" scripts/qa/MeshWitness.java
"$QA_JAVA_HOME/bin/jar" --create --file "$QA_DIR/witness-classes.jar" -C "$QA_DIR/classes" polaris
JAVA_HOME="$QA_JAVA_HOME" "$QA_D8" --min-api 24 --output "$QA_DIR/witness.dex.jar" "$QA_DIR/witness-classes.jar"
sha256sum "$QA_DIR/witness.dex.jar"
```

先 `requireApproval(plan, approval, 'witness')`。再推送**本次已审核、SHA 一致**的 jar 到新的 `/data/local/tmp/polaris-qa-NONCE/witness.dex.jar`；shell-owned 暂存目录 `0755`、jar `0444`，使 Debug app UID 只读。`run-as com.polaris2.app.debug` 新建 `cache/polaris-qa-NONCE`，权限 `0700`，复制 jar 进去；严禁覆盖先前已有目录。companion 的 counter file 在 app 私有 cache 内。

Android witness 必须以 **Debug app UID** 的 `run-as` 执行，保证其出站实际受此 app 的 VPN 路由控制；shell UID 的 socket 可能被 per-app VPN 排除，不能作证。先记录当前 per-app VPN 是否包含该 app UID，若绕过或无法确认则停止。

所有 helper 命令默认 dry-run，无批准文件也不建 socket、不调用 ADB：

```bash
node scripts/android-batch-witness.mjs serve "$QA_DIR/plan.json" "$QA_DIR/approval.json" "$QA_DIR/pc-counters.json"
node scripts/android-batch-witness.mjs android-serve "$QA_DIR/plan.json" "$QA_DIR/approval.json"
```

获得批准后才加 `--execute`。PC `serve` 才绑定两个 `127.0.0.1` listener；Android `android-serve` 返回 `APPROVED_COMMAND_ONLY` 的 `argv`，**它不会执行 ADB**。以下函数在 operator 会话中执行已批准的 Android argv，避免字符串插值和 shell 注入；其中 `QA_DIR`、`plan`、`approval` 与显式设备序列号来自本次会话：

```js
import { execFile, execFileSync } from 'node:child_process';
import { androidArgs } from './scripts/android-batch-witness.mjs';
import { sha256 } from './scripts/android-batch-qa.mjs';
const shellQuote = value => "'" + String(value).replaceAll("'", "'\\''") + "'";
if (sha256(QA_SERIAL) !== plan.serialSha256) throw new Error('Wrong device');
const jar = `cache/polaris-qa-${plan.nonce}/witness.dex.jar`;
const androidCommand = (mode, options) => {
  const argv = androidArgs(plan, approval, mode, options); // rechecks scope/time/plan immediately
  return ['-s', QA_SERIAL, 'shell', 'run-as', plan.package, 'sh', '-c',
    shellQuote(`CLASSPATH=${shellQuote(jar)} exec app_process /system/bin polaris.qa.MeshWitness ${argv.map(shellQuote).join(' ')}`)];
};
// Foreground child: hold this handle; never kill by an unverified recycled PID.
const androidWitness = execFile(QA_ADB, androidCommand('serve', []), { timeout: 950000 });
// For each short probe/health call:
const androidProbe = (phase, protocol) => JSON.parse(execFileSync(QA_ADB,
  androidCommand('probe', [phase, protocol]), { encoding: 'utf8', timeout: 90000 }));
const androidHealth = protocol => JSON.parse(execFileSync(QA_ADB,
  androidCommand('health', [protocol]), { encoding: 'utf8', timeout: 15000 })).loopbackHealth;
```

ADB remote shell quoting must be retained exactly; `execFile` alone does not remove Android shell parsing. The helper array above passes one quoted command to `sh -c`. Verify `READY` with the matching plan SHA and both fresh counter objects before traffic. Read Android counters via `adb -s SERIAL exec-out run-as PACKAGE cat cache/polaris-qa-NONCE/counters.json`; read PC counters from its local private file. A partially written JSON is an observation error: retry boundedly, never replace it with zeros. Each server has a maximum 15-minute lease, additionally capped by approval expiry. Expired lease stops the batch; do not restart witness mid-batch.

## 3. 单次启动，按五个 phase 顺序执行

首先记录 `{stage:'baseline', snapshot:await capture()}`；它必须与 plan 完全相等。initial receipt 为 `{schemaVersion:1, planSha256:plan.planSha256, events:[]}`。每次修改前调用 `requireNextStage(plan, receipt, phase, approval, scope)`；任何失败都只允许进入恢复，不能继续测另一个 phase 或刷新基线掩盖失败。

策略修改使用当前 `config_get` 读回的 node，只改 `meshInboundPolicy`，先用 `projectSnapshot` 验证 `invariantsSha256`/identity/PC 状态仍匹配。`server_update({server: {...liveNode, meshInboundPolicy:plan.allowPolicy}})` 成功后，`requireApproval(...,'android-vpn')` 再 `proxy_start`。只启动现有选中节点；不 enroll、不切账号、不换节点。等待 `proxy_get_status` 为 running、不 starting，pending summary 四项全部为空/false。后续策略变更后若仍 pending，在同一批准窗口执行 `proxy_apply_pending_changes`，等待它实际 `applied` 且 pending 为空；`deferred/skipped` 不算 applied。

| phase | 策略 / 实际底层网络 | 必须执行的探测 |
|---|---|---|
| `wifi-positive` | 精确端口 allowlist / Wi-Fi | PC→Android 与 Android→PC，各 TCP、UDP，3 个独立 nonce challenge |
| `policy-negative` | `{mode:'block'}` 真正生效 / Wi-Fi | PC→Android TCP、UDP 拒绝/超时，接收端两个计数均零增长；同期 Android `health` 两协议本地回显成功；Android→PC 两协议继续正常回显 |
| `policy-restored` | 恢复 plan.allowPolicy / Wi-Fi | 四组合重新正常；与负例前的正例共同排除 listener 故障和单向断路 |
| `cellular-positive` | allowlist / 关闭 Wi-Fi 后真实 cellular underlying 网络 | 用户手动关 Wi-Fi，保持已开启的移动数据；确认新的 physical handle，再执行四组合新 nonce |
| `wifi-return` | allowlist / 重开 Wi-Fi 后真实 Wi-Fi underlying 网络 | 用户手动重开 Wi-Fi并等到实际切回；确认 handle 不再是 cellular，再执行四组合新 nonce |

每个 phase 的每个方向/协议**顺序执行**：保存 receiver `before` → 发完整 N 个 challenge → 等 settle → 保存 receiver `after`。健康探测用不同 wire token，不计入 mesh counters。四组探测都完成后重新 `capture()`；若策略/底层网络发生中途漂移，保留失败证据并停止。不能并发造成 counter 混读。

PC→Android：

```bash
node scripts/android-batch-witness.mjs probe "$QA_DIR/plan.json" "$QA_DIR/approval.json" wifi-positive tcp --execute
node scripts/android-batch-witness.mjs probe "$QA_DIR/plan.json" "$QA_DIR/approval.json" wifi-positive udp --execute
```

其余 phase 替换固定 phase 名。Android→PC 使用 `androidProbe(phase, protocol)`。assemble 每个 probe 为下列结构，来自真实 probe 输出和真实 receiver 计数；`phase` 不写入 probe 结构，event 的 `stage` 持有它：

```js
const probeReceipt = {
  direction: client.direction, protocol: client.protocol, attempted: client.attempted,
  outcomes: client.outcomes, ackSha256s: client.ackSha256s,
  before: receiverBefore[client.protocol], after: receiverAfter[client.protocol],
  loopbackHealth: phase === 'policy-negative' && client.direction === 'pc-to-android'
    ? androidHealth(client.protocol) === true : false,
};
const event = { stage: phase, snapshot: await capture(), probes: fourProbeReceipts };
receipt = appendEvent(plan, receipt, event); // throws before moving to next phase on any mismatch
writePrivateJson(newReceiptPath, receipt); // use a new file for every checkpoint
```

也可离线录入已经投影的 event，执行一次 receipt gate：

```bash
node scripts/android-batch-qa.mjs record "$QA_DIR/plan.json" "$QA_DIR/last-accepted.json" "$QA_DIR/event.json" "$QA_DIR/new-receipt.json"
```

退出码 `2` 是有效但尚未完成的 `INCOMPLETE`，不是测试失败；不要用无区分的 `set -e` 将它误当完整 batch 失败。`1` 表示字段/计数/顺序/恢复证据无效。超时只有在 block policy applied、listener alive、同期本地健康与出站正例都在、前后入站正例完整时才构成 policy 负例。

## 4. 成功与失败都执行恢复，只恢复本次拥有的内容

`finally` 恢复顺序：恢复原 Wi-Fi 状态并确认实际 underlying Wi-Fi → `proxy_stop` 并确认 stopped/not starting → 再次检查 node 除 policy 外仍等于内存 `originalNode` → 只恢复其原有 policy（原先缺席则删除字段；原先存在则原样恢复）→ 验证选中节点、完整 config hash、身份、PC baseline、app PID 和网络开关仍匹配。发现用户并发修改或身份变化时，不用原始整个 config 覆盖它；停止恢复写入，标 `RESTORE_FAILED`，保留具体差异的**字段名**和 hash 交给用户处置。

关闭自己持有的 PC server/process，Android companion 等自身 lease/expiry退出或用已验证 cmdline+nonce 的 app UID PID终止；仅中断宿主 `adb` 进程不证明远端 listener 已停。证实两个 witness 的 counter `alive:false`、进程消失、两个本次 listener 均关闭后再清理。只删除准确 nonce 路径下列已知文件和空目录，不执行广泛 rm/glob/forward --remove-all：

- app 私有 cache 内本次 `witness.dex.jar`、`counters.json`；之后 `rmdir cache/polaris-qa-NONCE`。
- 本次 shell 暂存的 `witness.dex.jar`；之后 `rmdir /data/local/tmp/polaris-qa-NONCE`。
- 在上述恢复和 witness/artifact 清理完成后，先 `const restoredSnapshot = await capture()`，再调用本次 CDP session 的 `close()`，删除它创建的单个 forward。baseline capture 时 app 已运行，因此无需停止 app。

最终 event 固定为 `{stage:'cleanup', snapshot:restoredSnapshot, witnessesStopped:true, forwardsRemoved:true, artifactsRemoved:true}`，三个布尔值只在实际观察满足后填写；CDP 关闭后不再调用 `capture()`。手机复连原 Wi-Fi可得到新的 handle，因此 cleanup 比对允许 handle 改变，但 transport、所有开关与其余基线必须完全恢复。

```bash
node scripts/android-batch-qa.mjs assess "$QA_DIR/plan.json" "$QA_DIR/final-receipt.json"
```

完整所有阶段并恢复才返回 `PASS`/退出 `0`。未完成 `INCOMPLETE`/退出 `2`。某 phase 失败后不要伪造跳过它的完整 receipt；用最后接受的 checkpoint 和真实 cleanup 输出失败后的恢复证据：

```bash
node scripts/android-batch-qa.mjs abort "$QA_DIR/plan.json" "$QA_DIR/last-accepted.json" "$QA_DIR/cleanup-event.json" "$QA_DIR/abort-receipt.json"
```

`ABORTED_RESTORED` 退出 `2`、不作通过声明；`RESTORE_FAILED` 退出 `1`。operator 正常完成审批窗口外的恢复必须具有此前用户授权的 cleanup 范围；不要为了继续测试自行延长审批。

这一批回执只证明受控 IPv4 TCP/UDP mesh 数据面、策略入站负例和 Wi-Fi/cellular/Wi-Fi 切换后的 fresh echo。WARP、DNS、IPv6、直连/DERP 路径只能记录“readiness / 未验证”；没有独立受控出口和 DNS witness 时不能声称它们数据面通过。当前提供的是准备完成的操作工具与证据闸门，现场适配点和实际设备结果尚未完成。
