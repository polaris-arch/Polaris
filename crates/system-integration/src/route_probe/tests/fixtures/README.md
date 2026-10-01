# route_probe 真机抓取夹具

这里放的是**真机上原样跑出来的命令输出**。它存在的唯一理由写在 `route_probe.rs` 的头注里：
本模块的解析器按真实输出写，不按记忆里的格式写。一旦有人把样本"整理"得好看些，
这个目录就退化成一份自证 —— 解析器过的是自己想象中的格式。

## ⚠️ 地址是脱敏过的，不要"改回真值"

**本仓是公开仓。** 真机抓取里带着采集者家庭网络的可识别信息：运营商分配的全球 IPv6
前缀（能反查到 ISP 与大致归属）、以及全屋设备的 MAC 地址。这些一旦推上去就是永久公开的。

所以入库前跑了一遍**保形脱敏**。**规则由脚本持有，不由本文件持有**：

```
~/docs/polaris/scripts/polaris-redact-fixture.py
```

改夹具或新增夹具时跑它，别照着一张表手工做 —— 一张写在文档里的规则表对执行没有强制力，
同一份表在不同人手里会被执行成不同结果（本仓吃过这个亏，`memory` 里那条叫「判据由代码持有」）。

大意是：IPv4 全部逐字保留（私网/环回/链路本地/组播，本身不可识别），`fe80` / `ff00`–`ff0e` /
`fd7a:115c:a1e0`（Tailscale ULA，协议常量）/ `%作用域` / `/前缀长度` / 列对齐与全部非地址文本
也逐字保留；其余每个十六进制组按 **同长度 + 同映射** 替换。

### 🔴 四条踩过的坑（脚本已经处理，但改脚本的人要知道）

1. **Windows 的 MAC 是连字符形**：`route print` 的接口列表里逐字是 `bc-24-11-24-98-95`
   （PowerShell 的 `MacAddress` 列同形），**不是**冒号形。按 `([0-9a-f]{2}:){5}` 写的正则在
   Windows 夹具上**零命中** —— 而零命中与「这份文件本来就没有 MAC」长得一模一样，
   脱敏脚本会自信地报「已处理」。
2. **Python 默认的 universal newlines 会吃掉 CRLF**：`open(path)` 读进来时 `\r\n` 变成 `\n`，
   `open(path, "w")` 写出去时又按平台换。**读写都要 `newline=""`**，否则脱敏这一步顺手把
   Windows 抓取的行尾规范化掉了 —— 而那正是 `.gitattributes` 在守的东西。
3. **协议常量被当成可识别信息脱敏**（2026-09-13 修两次）。两个受害者形态不同、后果同：
   - `route -n get` 回显的 **v6 掩码** `ffff:ffff:ffff::` 被换成 `0a4f:0a4f:0a4f::`；
   - `ip -d link show` 里的**广播 MAC** `ff:ff:ff:ff:ff:ff` 与全零 MAC 被换成随机值。

   这类损坏**不会让夹具看起来坏掉**（形态完好、门照样绿），它把一路读数悄悄降级：
   掩码被换掉之后，macOS 交叉对差门的 v6 那半就只剩地址与接口在比，前缀长度不再来自内核。
   掩码判据要写在**位级**（前导连续 1 ⇔ 取反后是 `2^k-1`）——第一版写成「每组 ∈ {ffff, 0}」，
   `/60` 的 `ffff:ffff:ffff:fff0::` 仍然漏。广播 MAC 则要先换占位符再放行：
   它同时匹配 IPv6 正则（6 组、有冒号），MAC 阶段原样返回之后会被 IPv6 那一轮再吃一次。
4. **正则的尾部 `\b` 吃不到 `::`**：`ffff:ffff:ffff::` 只匹配出 `ffff:ffff:ffff`。
   掩码判据拿这个残缺串去解析必然失败 —— 补 `::` 再试一次，**不要去改正则**：
   改正则会改动全部匹配面 ⇒ 同映射失效 ⇒ 已入库夹具的每个地址都变。

**脚本自带 `--selftest`**（13 条用例，保留侧与替换侧各半）。改脱敏规则前后各跑一次：
只测「该替换的替换了」会被一个什么都不做的脚本骗过，保留侧的用例才是防这次这类损坏的那一半。

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
| `V4_NETSTAT` / `V6_NETSTAT` / `IFCONFIG_FLAGS` | 同 `V4` / `V6` / `IFCONFIG` | ✅ 新一代采集脚本给这三段改了名；`classify()` 两套都认（只认一套会让另一代夹具悄悄失去覆盖） |
| `V4_ROUTE` / `V6_ROUTE` | `ip -o route show` / `ip -o -6 route show` | ✅ `parse_ip_routes` |
| `LINK_TUN` / `LINK_WIREGUARD` / `LINK_OVPN` | `ip -o link show type {tun,wireguard,ovpn}` | ✅ `parse_ip_link_names`。**单段为空是合法的**（那台机器没装该模块）⇒「非空」只在三者**并集**上断 |
| `GET_VPNCONNECTION` / `GET_VPNCONNECTION_ALLUSER` | `Get-VpnConnection [-AllUserConnection]` | ✅ `parse_vpn_connection_names` —— **判据取材面**，不是备查。RAS 族的承载接口不在 `Get-NetAdapter` 里。**空分节是合法结果**（这个作用域里没有 VPN 连接），与 `<<POLARIS-CAPTURE-UNAVAILABLE>>`（cmdlet 不存在）分得开 |
| `GET_NETIPINTERFACE` | `Get-NetIPInterface` | 备查：它**看得见** RAS 承载接口（ifIndex 35）但不带 ifType —— 「没有任何 cmdlet 给 RAS 一个 ifType」这条结论的证据之一 |
| `V4_ROUTE_GET` / `V6_ROUTE_GET` | 逐目的地的 `route -n get` | ✅ macOS 的**第二读数**，由 `macos_netstat_reading_agrees_with_the_independent_route_get_reading` 消费（不进 `absorb_*` 那条常规路径） |
| `META` `ENV` `NETSTAT_I` `NETWORKSETUP_ORDER` `TAILSCALE` `IPCONFIG` `GET_NETROUTE_*` `NETSH_INTERFACE_6` `LINK_DETAIL` `ADDR` `END` | 见采集脚本 | 备查，不是判据取材面 |

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
| `windows-w207-ovpn-tun-connected-2026-09-13.txt` | **TAP-Windows6 的 ifType 正样本**：`OpenVPN TAP-Windows6`（`ComponentID` = `root\tap0901`、`TAP-Windows Adapter V9` 9.27.0.0、`Status=Up`、`10.8.0.2`）报 **`53`**，**不是**此前登记的疑值 `6` ⇒ 现有白名单 `{131, 53}` 本来就覆盖它，判据不用改。同一份里 Tailscale 也在（两张 `53` 同时在场）。**磁盘上带着 CRLF**（463 个 CR） | RAS（SSTP / L2TP / IKEv2）那族的 ifType 仍无样本 |
| `windows-w207-hyperv-present-2026-09-13.txt` | **`53` 假阳性面的负向对照**：装 Hyper-V 之后多出来的 `vSwitch (Default Switch)` / `vEthernet (Default Switch)` 报的是 **`6`**，没有落进 `53` 这个宽桶。同一份反向印证 **`6` 绝对不能收** —— 它在这一份里同时是物理网卡、Hyper-V 虚拟交换机、内核调试适配器。另带 TAP 的 `Disconnected` 态（ifType 仍是 `53` ⇒ 不随链路状态漂）。**磁盘上带着 CRLF**（335 个 CR） | VMware / Docker 的虚拟适配器仍无样本 |
| `linux-vm185-ovpn-dco-2026-09-13.txt` | **Linux 侧第一份真机夹具**，证的是 `ip -o link show type tun` **看不见 OpenVPN 2.7**：OpenVPN 2.7 默认走 ovpn-dco，设备 link type 是 `ovpn`（`ip -d link show tun0` 第三行 `ovpn addrgenmode random …`），而设备名仍叫 `tun0`/`tun1`。`type tun` 在这台机器上只回 `tap0`。`tun1` 上有 `198.18.42.0/24`，**落在 FakeIP 段 `198.18.0.0/15` 里** —— 补 ovpn 这条腿之前它进不了 `tunnel_interfaces`，`detect_tunnel_conflicts` 返回 0 条冲突 | `gre` / `sit` / `ipip` / `vti` / `xfrm` / `ip6tnl` 那几种 link type 仍无样本（判据不盲收，见下） |
| `windows-w207-ras-l2tp-connected-2026-09-13.txt` | **Windows RAS 族的第一份真机样本，坐实一个真缺陷**：L2TP 连上之后承载流量的接口以 VPN 连接名为别名（`PolarisProbeL2TP`，ifIndex 35，宣告 `0.0.0.0/0` metric 1），而 `Get-NetAdapter -IncludeHidden` 对该 index **返回 0 条** ⇒ ifType 白名单那条腿整个取材面看不到它。另：`WAN Miniport (L2TP/IKEv2/SSTP/PPTP)` 实测 **131**、`(PPPOE)` 实测 **23**、`(IP/IPv6/Network Monitor)` 实测 **6**；那批 131 是恒 `Disconnected` 的协议模板，路由表上一条都没有。`@@@GET_VPNCONNECTION_ALLUSER` **是空的**（这台机器上没有全局连接）—— 空 ≠ 没查。**磁盘上带着 CRLF**（410 个 CR） | WireGuard NT / Zscaler / GlobalProtect 那族仍无样本；没有 `VpnClient` 模块的 SKU（Server Core）上 `Get-VpnConnection` 会怎样，**未核实** |
| `macos-p101-route-get-ts-on-2026-09-13.txt` | **macOS 的第二读数**：逐目的地的 `route -n get`（PF_ROUTE 的 `RTM_GET`，与 `netstat -rn` 的路由表 dump 是**不同的内核接口**）⇒ mac 侧第一次有了能把解析器顶红的独立读数。三种必须处理的真形态：① `route: bad address`（`255.255.255.255/32`，**没有 `interface:` 行**）；② 本机地址回 `lo0` 而 dump 里同一前缀还挂在物理口上；③ `### dest=Destination` 是采集脚本 awk 把列头当数据取的**噪声**。🔴 **v4 必须先补零到四段**：`route -n get 192.168.10` 问到的是 `192.168.0.10`。前缀长度**两族都由内核回显的 `mask:` 数出来**（v4 33 条 / v6 42 条），与生产解析器一行代码不共用 | `route -n get` 只回内核会选的**那一条**路由（`RTM_GET` 的语义），而 `netstat -rn` dump 打印全部 ⇒ 多宿主前缀（`ff00::/8` 挂 13 个接口）上两读数只能做**子集**比较，不能做相等 |
| `macos-p101-parallels-running-2026-09-13.txt` | **虚拟化是常规场景**的 macOS 输入面：Parallels Desktop 运行中 + Tailscale 连接态**同时在场**。PD 18+ 建的是 `vmenet0/1/2` + `bridge100/101/102`（不是老版本的 `vnic*`），六个**全是 `BROADCAST` 型、零 `POINTOPOINT`** ⇒ 判据一个都不收。「不误报」（六个虚拟接口不进名单）与「不漏报」（12 个 utun 仍全进）能从同一份输入上取，由 `parallels_virtual_nics_are_not_mistaken_for_tunnels` 钉住 | 老版本 PD 的 `vnic0`/`vnic1` 与 VMware Fusion 的 `vmnet*` **没有样本**。判据不按名字走，所以那两族理论上同样不收 —— 推论，不是实测 |

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
或 `53`（`IF_TYPE_PROP_VIRTUAL`）**。2026-09-12 的实测取材面（`windows-w207-wintun-present-…` 的
`@@@GET_NETADAPTER`，**四正两负**）：

| InterfaceAlias | InterfaceType | NdisPhysicalMedium | ComponentID | DriverDescription | Status | 是隧道 |
|---|---|---|---|---|---|---|
| `Teredo Tunneling Pseudo-Interface` | **131** | 0 | （空） | 同别名 | Not Present | ✅ |
| `Microsoft IP-HTTPS Platform Interface` | **131** | 0 | （空） | 同别名 | Not Present | ✅ |
| `6to4 Adapter` | **131** | 0 | （空） | 同别名 | Not Present | ✅ |
| `Tailscale`（wintun） | **53** | 0 | `Wintun` | `Wintun Userspace Tunnel` | **Up** | ✅ |
| `以太网` | 6 | 0 | `PCI\VEN_1AF4&…` | `Red Hat VirtIO Ethernet Adapter` | Up | ❌ |
| `以太网(内核调试器)` | 6 | 14 | `root\kdnic` | `Microsoft Kernel Debug Network Adapter` | Not Present | ❌ |

2026-09-13 那两份又补了**三正两负**，都是此前没有输入的形态：

| 抓取 | InterfaceAlias | InterfaceType | ComponentID | DriverDescription | Status | 是隧道 |
|---|---|---|---|---|---|---|
| `…-ovpn-tun-connected-…` | `OpenVPN TAP-Windows6` | **53** | `root\tap0901` | `TAP-Windows Adapter V9`（9.27.0.0） | **Up**（`10.8.0.2`） | ✅ |
| `…-hyperv-present-…` | `OpenVPN TAP-Windows6` | **53** | `root\tap0901` | 同上 | Disconnected | ✅ |
| 两份都有 | `Tailscale`（wintun） | **53** | `Wintun` | `Wintun Userspace Tunnel` | Up | ✅ |
| `…-hyperv-present-…` | `vSwitch (Default Switch)` | **6** | `vms_vsmp` | `Hyper-V Virtual Switch Extension Adapter` | Up | ❌ |
| `…-hyperv-present-…` | `vEthernet (Default Switch)` | **6** | （空） | `Hyper-V Virtual Ethernet Adapter` | Up | ❌ |

🔴 **`6` 在真机数据上同时是三种东西**：真业务网卡（`Red Hat VirtIO Ethernet Adapter`）、
Hyper-V 虚拟交换机、内核调试适配器。「不盲收 `6`」此前只是推理，现在有实测。

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

**🔴 判据面剩下的缺口**（由 `the_captures_pin_iftype_53_for_wintun_and_tap_windows6` 与
`iftype_53_admits_the_two_vpn_drivers_but_not_the_hyperv_switches` 钉着）：

1. ~~`53` 的假阳性面无样本~~ —— **Hyper-V 那一半 2026-09-13 证否了**（见上表的两个 `6`）。
   **VMware / Docker 仍未核实**。
2. ~~TAP-Windows / OpenVPN（`tap0901`）的 ifType 无样本，疑报 `6`~~ —— **实测 `53`，那条疑值
   是错的**。推论值得记一笔：「以太网仿真」是数据链路层的形态，与 `InterfaceType` 这个用途
   分类不是一回事，从前者推后者本身就不成立。
3. ~~RAS（SSTP / L2TP / IKEv2）疑报 `23`~~ —— **2026-09-13 实测订正**：
   `WAN Miniport (L2TP/IKEv2/SSTP/PPTP)` 四张全报 **131**，报 `23` 的是 `WAN Miniport (PPPOE)`
   （**接入协议不是隧道**）⇒「不收 `23`」的结论对，理由此前写反了。
   **仍未核实**：WireGuard NT / 各家企业 VPN 客户端（Zscaler / GlobalProtect 之类）。

**🔴 ifType 白名单有一条结构性天花板：RAS 族它判不出来，也补不上。** 连接建立时承载流量的
接口以 VPN 连接名为别名，**不在 `Get-NetAdapter` 的枚举里**；`Get-NetAdapter` 里那批
`WAN Miniport (...)` 是恒 `Disconnected` 的协议模板，它们的 `131` 说明不了任何正在跑的连接。
逐个 cmdlet 问过：

| 来源 | 看得到 ifIndex 35 | 带类型信息 |
|---|---|---|
| `Get-NetAdapter -IncludeHidden` / `MSFT_NetAdapter` / `Win32_NetworkAdapter` | ❌ | — |
| `Get-NetIPInterface` / `netsh interface ipv4 show interfaces` / `Get-NetRoute` | ✅ | 无 ifType |
| `Get-VpnConnection` | ✅（按名） | **`TunnelType=L2tp`** |

**没有任何 cmdlet 给 RAS 接口一个 IANA ifType**，故这一族改由 `parse_vpn_connection_names`
承担：并入 `ConnectionStatus -eq 'Connected'` 的连接 `Name`（路由表按 `InterfaceAlias` 对齐，
别名就是连接名）。选它而不是「集合差」（`Get-NetIPInterface` 减 `Get-NetAdapter`）的理由是
**别重实现引擎、去问它** —— `Get-VpnConnection` 是 Windows 自己对「哪些是 VPN」的回答，
还附带 `TunnelType`；集合差要自己排掉 `Loopback Pseudo-Interface 1` 之类，假阳性面未知。
正负三面钉在 `windows_leg_sees_the_connected_ras_tunnel` /
`a_disconnected_vpn_connection_is_not_a_tunnel` /
`ras_protocol_template_miniports_announce_nothing`。

**Linux 隧道判据 = `ip -o link show type <T>` 的三种 link type：`tun` / `wireguard` / `ovpn`**，
三种各有真机正样本（前两种 2026-09-11 本机，`ovpn` 是 VM185 那份）。
`gre` / `sit` / `ipip` / `vti` / `xfrm` / `ip6tnl` 同样是隧道 link type，**判据里没有它们** ——
仓里一份样本都没有，与 Windows 侧同一条「不盲收」纪律。缺口由
`linux_tunnel_link_types_are_exactly_the_three_with_samples` 钉着。

## 行尾：`.gitattributes` 守的东西，与真正该钉住的不变量

本目录的 `.gitattributes`（`*.txt -text`）声称在保护 Windows 抓取的 CRLF。**但三份 2026-09-12 的
Windows 夹具在磁盘上一个 `\r` 都没有** —— 上一轮入库时就丢了，而没有任何地方红过：
**门在，但没牙**。

把行尾修回去只是提高保真度，治不了这个形状：下一次谁再用一个规范化行尾的工具过一遍夹具
（比如没写 `newline=""` 的 Python 脚本），同样的事会再发生一次，同样没人喊。真正该钉住的
不变量是**「解析器不因行尾而分叉」** —— 那条由
`windows_parsers_do_not_fork_on_line_endings` 钉住（每份夹具在内存里规整出 LF 与 CRLF 两版，
四支解析器逐条对差），钉住之后夹具的行尾丢没丢就降级成保真度问题。

那道门另带一条**磁盘普查**：至少得有一份夹具真的带着 CRLF（当前是 2026-09-13 那两份，
463 / 335 个 CR），否则「CRLF 是真机形态」这件事在仓里没有任何实物依据，上面那组比对
比的就是两个合成串。

**`@@@NETSH_INTERFACE` 顶不上来**：它的表只有 `Idx/Met/MTU/State/Name`，**结构上**没有类型列；
而且名字列在 zh-CN 控制台上是乱码 —— `以太网` 逐字打成 `浠ュお缃?`（UTF-8 字节被当 GBK 读）。
走 cmdlet 对象的 `Get-NetIPAddress` / `Get-NetAdapter` 两支则是干净的 UTF-8。
故 `parse_windows_tunnel_interfaces` 对它报 `CaptureIncomplete`，harness 把这条登记在扫描报告里。

## 怎么补样本

在现场机上跑对应脚本，把产出的 `.txt` **原样**放进本目录：

- macOS：`~/docs/polaris/scripts/polaris-collect-routes-macos.sh`
- Windows：`~/docs/polaris/scripts/polaris-collect-routes-windows.ps1`
- Linux：**暂无脚本**（如实登记，不是忘了）。VM185 那份是手抓的：`ip -o route show` /
  `ip -o -6 route show` / `ip -o link show type {tun,wireguard,ovpn}` / `ip -d link show` /
  `ip -br addr`，分节名见上面的分节表。

脱敏：`~/docs/polaris/scripts/polaris-redact-fixture.py`（**读写都要 `newline=""`**，否则
Windows 抓取的 CRLF 会在这一步被吃掉）。

**第三条坑（2026-09-13 修）**：「同长度替换」会把 `route -n get` 回显的 **v6 掩码**
（`ffff:ffff:ffff::`）当成普通十六进制组换掉，于是内核算出来的那个掩码变成了乱码
（`0a4f:0a4f:0a4f::`）—— 表面上夹具没坏、解析器也照常绿，实际是把一路**独立读数**悄悄降级成了
抄另一路的数字。脚本现在按**位级判据**（前导连续 1）认出合法 v6 掩码并逐字保留，全球 IPv6
仍全部脱敏。判据侧的收据在 `macos_netstat_reading_agrees_with_the_independent_route_get_reading`
里：v4 33 条 / v6 42 条前缀长度由内核回显的 `mask:` 数出来，条数钉死。

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
