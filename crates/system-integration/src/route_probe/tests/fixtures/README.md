# route_probe 真机抓取夹具

这里放的是**真机上原样跑出来的命令输出**。它存在的唯一理由写在 `route_probe.rs` 的头注里：
本模块的解析器按真实输出写，不按记忆里的格式写。一旦有人把样本"整理"得好看些，
这个目录就退化成一份自证 —— 解析器过的是自己想象中的格式。

## ⚠️ 地址是脱敏过的，不要"改回真值"

**本仓是公开仓。** 真机抓取里带着采集者家庭网络的可识别信息：运营商分配的全球 IPv6
前缀（能反查到 ISP 与大致归属）、以及全屋设备的 MAC 地址。这些一旦推上去就是永久公开的。

所以入库前跑了一遍**保形脱敏**，规则如下（改夹具或新增夹具时照同一条做）：

| 处置 | 对象 |
|---|---|
| **逐字保留** | 全部 IPv4（私网/环回/链路本地/组播，本身不可识别）、`fe80`、`ff00`–`ff0e` 组播、`fd7a:115c:a1e0`（Tailscale ULA，协议常量）、`%作用域`、`/前缀长度`、列对齐与全部非地址文本 |
| **同长度替换** | 其余每个十六进制组 —— 按 salted SHA-256 取同样字符数的十六进制，同一输入恒映射到同一输出 |

关键是**同长度 + 同映射**：

- 同长度 ⇒ 冒号位置、组宽、行宽、列对齐全不变，`netstat` 那套列式输出的形态判据一条不损；
- 同映射 ⇒ 跨行引用同一地址的关系保住（主机路由与它所属的 `/64` 仍然对得上）。

脱敏后解析器测试结果与脱敏前**逐条相同**（24 passed），这就是"形态没被破坏"的收据。

**推论：夹具里的全球 IPv6 地址是合成的，不指向任何真实网络。** 看着像随机十六进制是对的，
不是采集失误，不要"修正"。真正的判据面（classful 缩写 `192.168.10` = `/24`、
长度明写但地址仍缩写、`%作用域` 在 `/` 之前、`Netif` 列号必须从列头读）全都落在脱敏不碰的那一半上。

**新增夹具前先自问**：这份输出里有没有能定位到人的东西（公网地址、MAC、主机名、账号、
MagicDNS 域名）？有就先脱敏再入库 —— 公开仓没有"回滚"。

## 命名

```
<平台>-<机器>-<抓的什么>-<YYYY-MM-DD>.txt
```

例：`macos-p101-netstat-rn-2026-09-08.txt`

- **平台**必须是 `macos` / `windows` / `linux` 之一 —— harness 靠文件名前缀分派解析器，
  认不出前缀的文件会在扫描报告里被点名，不会被静默忽略。
- **机器**是短标签（小写、无点），日后对得上是哪台设备。
- **日期**是采集日期，不是入库日期。

## 内容形态

按**独占一行**的 `@@@<分节ID>` 切段（ID 只含 `A-Z` / `0-9` / `_`）：

```
（这里是给人读的抬头：隐私提示、采集环境说明。不属于任何分节，harness 丢弃）
@@@META
platform=macos
...
@@@V4
Routing tables

Internet:
Destination        Gateway            Flags               Netif Expire
default            192.168.10.1       UGScg                 en0
...
@@@V6
...
```

分节体**逐字**保留，包括行尾空格（`netstat` 的空 Expire 列就长这样）与原始行终止符。
本目录的 `.gitattributes` 关掉了 git 的行尾转换，正是为了让 Windows 抓取的 CRLF 活着进仓 ——
仓根规则 `* text=auto eol=lf` 会把它规范成 LF，于是夹具里再也见不到 `\r`，
而解析器在真机上读到的**就是** CRLF：夹具绿、真机红，没有任何地方会喊。

采集脚本在"这条读法在这台机器上不可用"时，会把分节体写成一行哨兵
`<<POLARIS-CAPTURE-UNAVAILABLE>> <原因>`。harness 认得它，会登记成"抓了但那台机器上没这条命令"，
不会送进解析器。

## 分节 ID → 谁解析它

单一真值在 `../fixture_harness.rs` 的 `classify()`，此表只是给人看的摘要。

| 分节 | 来源命令 | 状态 |
|---|---|---|
| `V4` / `V6` | `netstat -rn -f inet` / `-f inet6` | ✅ `parse_netstat_routes` |
| `IFCONFIG` | `ifconfig -a`（**未过滤**） | ✅ `parse_macos_tunnel_interfaces` |
| `ROUTE_PRINT_4` / `ROUTE_PRINT_6` | `route print -4` / `-6` | ✅ `parse_route_print_routes`（**要配 `GET_NETIPADDRESS` 那张对照表**） |
| `GET_NETIPADDRESS` | `Get-NetIPAddress` | ✅ `parse_get_netipaddress` —— 判据取材面，不是备查 |
| `GET_NETADAPTER` | `Get-NetAdapter -IncludeHidden` | ✅ `parse_windows_tunnel_interfaces`（判据 = `InterfaceType` 列的 IANA ifType **131 或 53**，见下） |
| `NETSH_INTERFACE` | `netsh interface ipv4 show interfaces` | ⛔ 回退读法，**结构上**给不出适配器类型，见下 |
| `META` `ENV` `NETSTAT_I` `NETWORKSETUP_ORDER` `TAILSCALE` `IPCONFIG` `GET_NETROUTE_*` `NETSH_INTERFACE_6` | 见采集脚本 | 备查，不是判据取材面 |

Windows 的生产读法定在 `route print`（原生 exe，任何 SKU 都有）。`GET_NETROUTE_*` 一列就给出
`InterfaceAlias`、好解析得多，但它依赖 NetTCPIP 模块（Server Core / 精简版可能没有），
故只作**参照**：`route_print_result_matches_the_independent_get_netroute_reading` 拿它跟
`route print` 的解析结果逐条对差（40 vs 40 全等）。那条参照解析器在测试里**另写一份**、
不复用生产的切列函数 —— 复用等于两边共用同一个错，对差就没有检出力了。

## 现有样本的覆盖边界

| 样本 | 覆盖 | **不**覆盖 |
|---|---|---|
| `macos-p101-netstat-rn-2026-09-08.txt` | classful 缩写（`/8` `/16` `/24` 与"长度明写、地址仍缩写"的 `224.0.0/4`）、v6 `%作用域`、主机路由补 `/32` `/128`、`default` 跳过 | **连接态的 utun**（断开态，见下）。`ifconfig` 那一半被 `grep` 滤掉了，完全没抓 |
| `macos-p101-routes-ts-off-2026-09-12.txt` | 上面那些 + `ifconfig -a` 全文：`POINTOPOINT` 判据、`bridge0` 的嵌套成员行（与接口头行同形，只有列位置分得开）、`stf0: flags=0<>` 的**空 flags 列表**、DOWN 且无地址的 `gif0` 仍算隧道 | **连接态的 utun**（断开态 ⇒ 噪声过滤的**负样本**，见下） |
| `macos-p101-routes-ts-on-2026-09-12.txt`（`label=routes-ts-on`） | **连接态** `utun11`：25 条逐 peer `32.0.0.x/32` 主机路由 + `100.100.100.100/32`（MagicDNS）+ `fd7a:115c:a1e0::/48` 与 `::16/128`；`32.0.0.30 32.0.0.30 UH` 这条**无 `/前缀`** 的本机主机路由；**本机自己那条 v6 `/128` 挂在 `lo0` 上**（BSD 形态，故进不了 `foreign`）；`255.255.255.255/32` 也挂在 utun11 上而它**不在**噪声四块里 | `@@@TAILSCALE` 仍是「命令不在 PATH 上」哨兵（Mac App Store 版不装 CLI）⇒ **判连接态只能看路由表**。另：exit node 那条 `default` 在 utun11 上，按生产口径跳过 |
| `windows-w207-routes-ts-off-2026-09-12.txt`（`label=routes-ts-off-v2`） | 本地化表头（zh-CN / gb2312）、v4 的 `Interface` 列是本地 IP、v6 的 `If` 列是接口索引、`route print -6` 的**折行**（18 行）、永久路由表只有 4 列、`Get-NetAdapter` 里**没有**环回伪接口（索引 1 只能从 `Get-NetIPAddress` 拿）、隧道判据的 `131` 三正两负、`Format-Table -AutoSize` 挤不下时**从右边丢列**（脚本 `Select` 了 `LinkSpeed`，输出里没有它） | 没装 Tailscale ⇒ 没有 wintun 那一行。三个隧道适配器都是 `Not Present`、路由表上一条都没有 ⇒ 它是三份 Windows 抓取里的**起点对照**（`foreign` 为空） |
| `windows-w207-wintun-present-2026-09-12.txt`（`label=wintun-present`） | **wintun 的正样本**：Tailscale 1.102.4 已装、服务已起、`Status=Up`，`InterfaceType` = **53**（`IF_TYPE_PROP_VIRTUAL`），`ComponentID` = `Wintun`（**不空** ⇒ 直接证伪「ComponentID 为空」那条土办法） | `tailscale status` 逐字 `Logged out.` ⇒ wintun 上只有 3 条 `169.254.*` 自动路由，**一条业务网段都没有**。它现在的职责是噪声过滤的**负样本** |
| `windows-w207-ts-on-2026-09-12.txt`（`label=ts-on`） | **登录态 wintun**：适配器名单与上一份逐条相同（判据不随连接态漂），路由表上 30 条**全是业务网段** —— 25 条逐 peer `32.0.0.x/32` + `32.0.0.31/32`（本机，**Windows 侧就在适配器上**）+ `100.100.100.100/32` + `fd7a:115c:a1e0::/48` 与两条 `/128`；未登录时那 3 条 `169.254.*` 噪声**全撤了** | `53` 的**假阳性面**（这台机器上没有 Hyper-V / VMware / Docker 的虚拟适配器）；TAP-Windows / OpenVPN / RAS 那两族驱动的 ifType（见下） |

**连接态边界（2026-09-12 22:00 / 22:14 补齐）**：两个平台各有一份连接态抓取了。
「隧道宣告的**业务**网段被摘出来」这条链路现在**两侧都有真样本**，且与断开态那几份配成
正负样本对：

| | 隧道名单 | `foreign` | 其中业务网段 |
|---|---|---|---|
| Windows 无 wintun | 3 | 0 | 0 |
| Windows wintun up、未登录 | 4 | 3（全噪声） | **0** |
| Windows wintun up、已登录 | 4 | 30 | **30** |

由 `the_three_windows_captures_step_from_no_wintun_to_business_prefixes`（台阶差分）、
`windows_leg_on_the_connected_capture_keeps_business_prefixes` /
`windows_leg_on_the_logged_out_wintun_capture_sees_only_noise`（Windows 正负样本）、
`macos_leg_on_the_connected_capture_keeps_business_prefixes` /
`macos_probe_sample_is_tailscale_off_and_says_so`（mac 正负样本）、
`macos_tailnet_prefixes_appear_only_in_the_connected_capture`（三份 mac 抓取的分侧登记）钉着。

🔴 **照记忆写不出来的形态**：Tailscale 装的是**逐 peer 的 `/32` / `/128` 主机路由**，
不是一条 `32.0.0.0/24` 汇总段 —— 按「找一条汇总前缀」的直觉写断言会全落空。

**Windows 隧道判据 = `InterfaceType` 的 IANA ifType `131`（`IF_TYPE_TUNNEL`）
或 `53`（`IF_TYPE_PROP_VIRTUAL`）**。实测取材面（`windows-w207-wintun-present-…` 的
`@@@GET_NETADAPTER`，**四正两负**）：

| InterfaceAlias | InterfaceType | NdisPhysicalMedium | ComponentID | DriverDescription | Status | 是隧道 |
|---|---|---|---|---|---|---|
| `Teredo Tunneling Pseudo-Interface` | **131** | 0 | （空） | 同别名 | Not Present | ✅ |
| `Microsoft IP-HTTPS Platform Interface` | **131** | 0 | （空） | 同别名 | Not Present | ✅ |
| `6to4 Adapter` | **131** | 0 | （空） | 同别名 | Not Present | ✅ |
| `Tailscale`（wintun） | **53** | 0 | `Wintun` | `Wintun Userspace Tunnel` | **Up** | ✅ |
| `以太网` | 6 | 0 | `PCI\VEN_1AF4&…` | `Red Hat VirtIO Ethernet Adapter` | Up | ❌ |
| `以太网(内核调试器)` | 6 | 14 | `root\kdnic` | `Microsoft Kernel Debug Network Adapter` | Not Present | ❌ |

**`53` 是 2026-09-12 被真机坐实的一次判据翻案**：在那之前判据只认 `131`，而 wintun 报的是
`53` —— 旧判据在它唯一要做的那件事上静默失败（装着 Tailscale 的机器会拿到一句自信的
「无冲突」）。`53` 确实比 `131` 宽（它是「厂商自有虚拟接口」这个大桶），仍然收它的理由是
**代价不对称**：假阳性的代价只是展示面多列一条网段（判定只认与 FakeIP / Mesh / TUN 地址
**相交**，不会凭空多出冲突），假阴性的代价是那句「无冲突」。完整论证在
`route_probe::IF_TYPE_PROP_VIRTUAL` 的头注里。

**只认 ifType 这一列，另外两列抓回来了也不叠**：`NdisPhysicalMedium` 在真实数据上根本不分隔
（`以太网` 与四个隧道同为 `0`）；`ComponentID` 在**装 Tailscale 之前**那份上看着能用（隧道全空、
网卡不空），正是这种「看着能用」让它危险 —— wintun 那行**直接证伪**了它（`ComponentID` = `Wintun`，
不空）。也**不**拿 `ComponentID == "Wintun"` 收窄：那是把一条**类型**判据换成一张**驱动名单**，
wintun 之外的每种隧道驱动都不在名单上，等于把一个已知的漏换成一族未知的漏。驳回理由由
`the_two_rejected_cross_criteria_are_rejected_for_reasons_visible_in_the_capture` 与
`the_tunnel_criterion_does_not_also_require_an_empty_component_id` 钉在真机数据上，前提一变就红。

**🔴 判据面还剩两个缺口**（由
`the_wintun_capture_pins_iftype_53_and_two_driver_families_are_still_unverified` 钉着）：

1. **`53` 的假阳性面无样本**：两份带 wintun 的抓取里报 `53` 的只有 wintun 一行；装了
   Hyper-V / VMware / Docker 的机器上有没有别的适配器报它，**未核实**。
2. **另外两族隧道驱动的 ifType 无样本**：TAP-Windows / OpenVPN（`tap0901`，以太网仿真驱动）
   与 Windows 内置 VPN（RAS：SSTP / L2TP / IKEv2）。前者疑报 `6`、后者疑报 `23`（PPP），
   **均未核实、需真机抓取**；若成立，当前判据漏它们 —— 但这两个值**不能盲收**：`6` 是全部
   物理网卡，`23` 与 PPPoE 拨号同型，收了就是把一批真业务网卡判成隧道。

**`@@@NETSH_INTERFACE` 顶不上来**：它的表只有 `Idx/Met/MTU/State/Name`，**结构上**没有类型列；
而且名字列在 zh-CN 控制台上是乱码 —— `以太网` 逐字打成 `浠ュお缃?`（UTF-8 字节被当 GBK 读）。
走 cmdlet 对象的 `Get-NetIPAddress` / `Get-NetAdapter` 两支则是干净的 UTF-8。
故 `parse_windows_tunnel_interfaces` 对它报 `CaptureIncomplete`，harness 把这条登记在扫描报告里。

## 怎么补样本

在现场机上跑对应脚本，把产出的 `.txt` **原样**放进本目录：

- macOS：`~/docs/polaris/scripts/polaris-collect-routes-macos.sh`
- Windows：`~/docs/polaris/scripts/polaris-collect-routes-windows.ps1`

两个脚本全只读（`~/docs/polaris/scripts/polaris-collect-readonly-gate.sh` 按拒绝清单机械核验）。
放进来之后直接 `cargo test -p polaris-system-integration route_probe`：
harness 会枚举到它、调对应平台的解析器，并在"样本已到、解析器还没写"时把这件事写进扫描报告。

## `synthetic/`

**合成**样本，不是真机抓取。目前只有一份：

| 文件 | 是什么 | 给谁用 |
|---|---|---|
| `windows-w207-route-print-en-headers.txt` | 把 `windows-w207-routes-ts-off-2026-09-12.txt` 的 `route print` **本地化表头/标签**逐条换成 en-US（接口列表→Interface List、活动路由:→Active Routes:、在链路上→On-link 等），**路由数据行一个字节没动** | 证明 `parse_route_print_routes` 的解析**不依赖表头字面量** —— `route_print_parsing_does_not_depend_on_localized_headers` 断言它与 zh-CN 真样本解析出逐条相同的结果 |

它是**派生**的，不是第二次抓取：真机那份是唯一真值，改判据时先改真的那份的解析、再重新派生。
列宽没有按 en-US 的真实排版重新对齐（解析是 token 型的，不看列宽）—— 别拿它当英文机器的形态参考。

本目录不进 harness 的正常扫描面（扫描只读 `fixtures/` 的**直接**子项，不递归）。

## `malformed/`

故意的坏样本（空文件、截断行），给反向对照用：解析器必须报错，不许静默产出一份空的 / 少一半的
`Ok` —— 那种结果到了下游就长得像"看过了，没有冲突"。harness 的正常扫描只读本目录的**直接**
子项，不递归，所以这个子目录天然不进取材面。
