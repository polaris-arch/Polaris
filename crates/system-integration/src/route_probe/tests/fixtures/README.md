# route_probe 现场格式衍生夹具

解析器按真实命令输出的**格式**写；公开仓保存的是保形脱敏衍生物，不能把其中的地址、
主机或账号当成现场身份原文。私有原件与生成收据留在审查材料中，不是仓库构建依赖。

## 内容边界与生成工具

本批覆盖七份 Windows 抓取、一份 macOS route-get 抓取及其英文表头派生物：
未覆盖的本机 IPv4、主机/用户/tailnet 标签已一致虚构化。`host=win1` 与 ipconfig/status
中的同一主机联动；其它节点/用户使用定宽 `f…` 标识。文件名中的机器短标签是匿名 case ID。
这不是其余素材、Git 历史或所有二进制的零私密声明。

历史 tailnet `32.0.0.*` 段已有脱敏说明，本批保留该既有映射；它不是协议常量。
现有 IPv6/MAC 映射也保留。历史 IPv6/MAC 生成器**只覆盖这两类字段**，不处理 IPv4 或
主机/账号文本，而且再次处理衍生物会改变既有映射，不能拿它当幂等清理器。
本批任务级变换工具和指纹随私有审查材料保存；仓内测试不读取外置脚本或私人目录。

新采集必须先在私有环境生成衍生物，再入库。每个身份在多张表、各分节、注释和精确期望中
同值同映射，不同身份不得碰撞；CIDR 包含/相交、网络/广播地址、掩码、host 路由与
本机 IP → alias、ifIndex → alias 的关联必须保留。只改一张表会让独立对差失去意义。

协议结构逐字保留：默认路由、环回、广播/组播类别常量、合法 v4/v6 掩码、MagicDNS 地址、
Tailscale ULA 前缀、接口类型/驱动类别、连接状态和表头语言。真实主机的 host bits 与
类别常量分开处理；不要把私网业务路由当噪声，也不要改生产解析器去适应错误夹具。

### 保形要求

- 同长度字段替换，列起点、分隔符、行尾空格、空行、scope zone、CRLF/LF 不变；读取和
  写入均须显式保留换行，不能依赖 Python 默认 universal newlines。
- `.gitattributes` 的 `*.txt -text` 只禁止 Git 转换，拦不住生成工具自己规整内容。
- MAC 的冒号/连字符两种形态都要识别；全零和广播 MAC 仍是协议常量。
- IPv6 掩码用位级连续 1 判据识别，不能按普通十六进制组替换；否则 route-get 的
  `mask:` 不再是独立读数。保留历史掩码保护，不再次变换现有 IPv6/MAC。
- Mac classful 缩写与完整地址从同一虚构前缀派生，保留补零、显式 prefix、`Netif`
  列位置和 `%scope` 反例；名字有空格、Windows drive-colon 等格式也不得缩减。

仓内回归包括七份 Windows text/native parity、各表 join、Mac route-get 独立掩码、
CRLF/LF 对拍、zh/en/遮挡表头、RAS 与 Hyper-V 正反对照，以及合成主机跨元数据/status
关联。英文样本必须从**同版脱敏 zh-CN 来源**派生，唯一差异是本地化表头/标签；测试
同时核字节派生关系与解析结果。旧历史通过数不能替代新映射后的整组验证。

## 命名

```
<平台>-<机器>-<抓的什么>-<YYYY-MM-DD>.txt
```

例：`macos-p101-netstat-rn-2026-09-08.txt`

- **平台**必须是 `macos` / `windows` / `linux` 之一 —— harness 靠文件名前缀分派解析器，
  认不出前缀的文件会在扫描报告里被点名，不会被静默忽略。
- **机器**是匿名 case 短标签（小写、无点），不写真实 hostname 或账号。
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

相对已完成身份映射的来源，分节体**逐字**保留，包括行尾空格（`netstat` 的空 Expire 列就长这样）与原始行终止符。
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

在获授权的现场机上按上面的来源命令采集，只读保存私有原件；先生成身份已虚构化的
衍生物和内容/格式收据，再提交。Linux 仍须如实标记未具统一采集器；手抓命令与分节
契约见覆盖表。采集与脱敏工具不能修改网络状态，也不能把原始身份写回公开夹具。

保留独立 `route-get mask:` 的回归，不能用 netstat 的 prefix 抄成第二读数。
任何新变换均须同时验证“应替换”和“应保留”两侧，并核换行/列宽/关联关系。
放进来之后直接 `cargo test -p polaris-system-integration route_probe`：
harness 会枚举到它、调对应平台的解析器，并在"样本已到、解析器还没写"时把这件事写进扫描报告。

## `synthetic/`

**合成**样本，不是真机抓取。目前只有一份：

| 文件 | 是什么 | 给谁用 |
|---|---|---|
| `windows-w207-route-print-en-headers.txt` | 把 `windows-w207-routes-ts-off-2026-09-12.txt` 的 `route print` **本地化表头/标签**逐条换成 en-US（接口列表→Interface List、活动路由:→Active Routes:、在链路上→On-link 等），**相对同版脱敏来源的数据行不变** | 证明 `parse_route_print_routes` 的解析**不依赖表头字面量** —— `route_print_parsing_does_not_depend_on_localized_headers` 断言它与 zh-CN 脱敏来源解析出逐条相同的结果 |

它是**派生**的，不是第二次抓取：同版脱敏来源是派生基准，先核来源映射与解析，再重新派生。
列宽没有按 en-US 的真实排版重新对齐（解析是 token 型的，不看列宽）—— 别拿它当英文机器的形态参考。

本目录不进 harness 的正常扫描面（扫描只读 `fixtures/` 的**直接**子项，不递归）。

## `malformed/`

故意的坏样本（空文件、截断行），给反向对照用：解析器必须报错，不许静默产出一份空的 / 少一半的
`Ok` —— 那种结果到了下游就长得像"看过了，没有冲突"。harness 的正常扫描只读本目录的**直接**
子项，不递归，所以这个子目录天然不进取材面。
