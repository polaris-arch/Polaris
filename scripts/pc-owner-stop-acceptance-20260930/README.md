# PC owner-stop：一次设备窗口的离线准备

此目录只生成准备计划、读取选定文件的哈希，并检查手填证据包的形状与绑定一致性。候选必须通过 `--candidate` / `-Candidate` 显式输入最终统一消费源码的完整 40 位 SHA；工具没有旧来源的默认候选，也不反写自身提交 SHA。源码 SHA 不是 App/helper/core 二进制哈希或构建来源认证。

**没有设备 transition 执行器、helper IPC 探针、可信原生回执导出器、四 producer drain 观察器、helper 尾部观察器、独立后检或 restore 执行器。** 文件名中的 acceptance 表示待准备的验收工作，不表示此工具可以验收设备。prepare 成功仍为准备；即使完整、手填、哈希一致的 witness，也只得到 `NOT_READY`，永不输出设备 PASS、ReleasedExact、global NoOwner、managed 或发布放行。生产发布阻断脚本保持原样。

计划沿用会话已经授权的 PC 独立测试和配套升级范围，不要求重复审批；准备本身不增加授权。故障注入尚未实现，属于后续窗口依赖，必须处于已有授权范围内。

## 只读预检

只需 Python 3.9+ 标准库；Windows 预检用 PowerShell 5+。预检只读操作者选定的 App/helper/core 文件和 OS 元数据，不运行这些文件，不枚举 PID，不发 signal，不访问 helper IPC，不起停服务，不改变路由、DNS、防火墙，不安装、重启或下载。

device ID 使用非秘密资产标签 `[A-Za-z0-9._-]{1,96}`。nonce 为本窗口新建的 32–64 位小写十六进制串；本工具校验形状与一致性，不证明随机性或新鲜性。版本标签可含 `+`，由操作者声明，**不是运行时版本或 helper 能力探测结果**。

Linux 示例（路径仅输入本地工具，不进入输出）：

```sh
sh scripts/pc-owner-stop-acceptance-20260930/preflight-linux.sh \
  --candidate FINAL_UNIFIED_SOURCE_SHA \
  --device-id PC-LAB-01 --nonce YOUR_FRESH_HEX_NONCE \
  --app /selected/app --helper /selected/helper --core /selected/core \
  --app-version APP_VERSION --helper-version HELPER_VERSION --core-version CORE_VERSION \
  > preflight.json
```

macOS 使用相同参数，将 wrapper 换为 `preflight-macos.sh`。wrapper 拒绝与实际 OS 不符的运行。

Windows 示例：

```powershell
powershell.exe -NoProfile -NonInteractive -File scripts\pc-owner-stop-acceptance-20260930\preflight-windows.ps1 `
  -Candidate FINAL_UNIFIED_SOURCE_SHA `
  -DeviceId PC-LAB-01 -Nonce YOUR_FRESH_HEX_NONCE `
  -App C:\selected\app.exe -Helper C:\selected\helper.exe -Core C:\selected\core.exe `
  -AppVersion APP_VERSION -HelperVersion HELPER_VERSION -CoreVersion CORE_VERSION |
  Out-File -Encoding utf8 preflight.json
```

Windows 脚本先验证 Win32NT；32/64bit-os 标签表示 OS 位数，不宣称 CPU 架构。三平台输出只有 path-role 对应的 SHA-256/字节数、设备标签、nonce、必要版本及 OS 信息；不输出绝对路径、主机名、环境、配置、凭据或通用进程命令行。主机名只形成 hash，不是设备认证。文件哈希使用流式读取，前后大小/修改时间复核仅防常见读中变化，不证明运行进程使用了该文件、构建来自 candidate 或文件不可被并发替换。

示例的 `FINAL_UNIFIED_SOURCE_SHA` 必须替换为最终统一消费提交的 SHA，不能用其 source parent 或旧 PC 来源代替。缺 candidate 参数立即拒绝；工具不自动读取 git HEAD、认证构建来源或把版本标签当来源证据。

预检退出 `0` 只表示这些只读输入通过校验；它不证明设备能力或验收完成。macOS/Windows 设备上的实际脚本运行仍待窗口执行，host fixture 不代替平台实测。

## 计划与窗口矩阵

```sh
python3 scripts/pc-owner-stop-acceptance-20260930/acceptance.py prepare preflight.json > plan.json
```

退出 `2` 是固定的 prepared-only 结果。计划摘要绑定 candidate、device ID、nonce、选定三类文件哈希和声明版本。计划要求同一窗口留存原基线的哈希/配套关系与脱敏状态，执行已授权变更后恢复，并做独立后检；本工具不执行这些步骤。

| 阶段 | 后续必须取得的事实；当前均未实现采集 |
| --- | --- |
| baseline / old-helper-upgrade | 脱敏独立基线；配套 helper 能力与旧版本拒绝、升级影响。旧 helper 的 Stop ACK/NotRunning 不替代本代原生退出。 |
| main direct / helper Stop→Restart | 同代 owned Child/native birth 的停止结果；helper 尾部日志 writer 退休；新代准入。切模式/配置替换须纳同一受控功能检查。 |
| login / temp / check | 三条辅助 producer 的停止、取消后有界重试；原 Child/config/claim 保管；Unknown 不准新 spawn。 |
| Quit / App restart / update | 四 producer 全量 drain、本代 claim 提交与 helper 尾部；更新必须等待旧 App 的真实 OS 退出。 |
| 负控 | wait error、取消/迟到旧 birth、ACK-only；原 Unknown/Pending 必须保留，失败不得被后续成功或 PID 列表抹平。故障执行器尚缺。 |
| restore / independent-postcheck | 恢复配套基线，独立比较网络/功能与脱敏状态；恢复失败或缺后检维持 Unknown。 |

四 producer 指 `main / login / temp / check`。此计划不删减产品功能；原有 direct/helper、登录、测速、检查、Stop→Restart、Quit、重启和更新路径的设备行为仍待验证。原生进程退出仅是本地事实，不能证明 Go constructor/Start/PostStart/internal rollback/stale cleanup 的平台资源为 NoOwner；平台资源合同与发布冻结不由本工具解除。

## Witness 一致性检查，始终不能验收

```sh
python3 scripts/pc-owner-stop-acceptance-20260930/acceptance.py verify plan.json witness.json --bundle receipt-directory
```

退出语义：`1` 拒绝缺失/多余字段、错绑/旧 nonce、错哈希、synthetic、类型错误或越界证据；`2` 形状一致但 **NOT_READY**。没有验收通过的退出码。JSON 输入（计划、预检、witness、capture）限 1 MiB，文件字节哈希流式读取。

`OFFLINE_EVIDENCE_BUNDLE` 的 `records` 必须与 plan phases 数量、顺序完全一致，每条只有 `caseId / state / capture`。state 为 `OBSERVED / UNKNOWN / PENDING / NOT_EXECUTED`；后面三类允许 capture 缺失并原样列入 retainedStates。OBSERVED 必须有 capture 文件引用。每个引用只允许 bundle 内的相对路径、SHA-256、正字节数，拒绝 symlink 和 `..`。

capture envelope 要求 `synthetic / binding / planSha256 / caseId / state / source / identities / original`。identities 按本阶段 producer 列出 opaque `generation / nativeIdentity`；original 指向原始字节文件。这里的 source 名称是**未来观察器要求**，不是当前 helper wire 格式或已经存在的 exporter。工具只比较声明形状与文件哈希，不解析/认证原生回执、没有权威信任链，无法防止手填内容伪称 native wait。因此完整 forged envelope 也保持 NOT_READY；synthetic 标记为 true 的材料直接拒绝，不进入设备证据评审。shape/witnessConsistent 不等于 native exit 已发生。

原捕获的 UNKNOWN/PENDING 不可被外层 record 改判为 OBSERVED。PID 清单、文件哈希、Stop ACK、手填 identity 均不能补成原生退出；缺观察器、helper tail、四 producer drain 或独立后检时不能接受设备。计划/证据中的 claims 固定空数组。请勿将完整配置、secret state、环境或原始敏感 argv 放入 evidence；此工具不提供日志脱敏器，也不打印原始 capture 内容。

## 无害离线回归

```sh
PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover \
  -s scripts/pc-owner-stop-acceptance-20260930 -p 'test_*.py' -v
sh -n scripts/pc-owner-stop-acceptance-20260930/preflight-linux.sh
sh -n scripts/pc-owner-stop-acceptance-20260930/preflight-macos.sh
```

fixture 只创建临时文本/JSON 文件，不起 App/helper/core，不使用真实服务或设备。测试覆盖错绑/缺回执/超限输入/旧 Unknown/伪 source、完整手填仍 NOT_READY、权限范围不扩张和能力矩阵不被推断改写。测试通过只证明这些离线拒绝逻辑。

这些离线工具不被 packaging/runtime/release workflow 消费，位于现有 CI 分类器的非 registry roots 下；不新增 package、kernel、Android 或构建腿。纯脚本测试命令由准备批次统一执行，不声称已接入 CI 自动运行。
