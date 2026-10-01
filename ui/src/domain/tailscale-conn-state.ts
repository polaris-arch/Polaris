/**
 * Tailscale 单例「连接」卡的状态派生（纯函数，便于单测）。
 *
 * 按认证形态分流（见 docs/design/tailscale-connection-redesign.md）：
 *  - authKey 形态 = 静态凭据，等同 WireGuard，不参与登录态 → 恒 'key-ready'；
 *  - 交互登录形态 = 有登录态，按 loggedIn / authUrl 派生。
 *
 * loggedIn 来源是 app-store.tailscaleLoginStates[id]——该表是三条 feed 汇合后的综合登录态，故本函数无需
 * 单独处理缓存/state。三条 feed 的写入方（缺一即该表恒为初值，卡片会永远显示「未登录」）：
 *  ① 缓存初值：store 初始化时 loadTailscaleLoginStatesFromCache()（启动秒显）；
 *  ② state 文件兜底：api.server.tailscaleStateExists → applyTailscaleStateExists（挂载时拉一次，不起核）；
 *  ③ STATUS 实时校正：api.proxy.onTailscaleStatus → **经 [`isDefinitiveTsLoginFrame`] 过滤后**
 *     setTailscaleLoginState（启动过渡帧不许翻转已知登录态，理由见该函数）。
 * ②③ 是全局事件/挂载接线，归 App.tsx，不在本 domain 层。
 * 代理开关不改变状态机（loggedIn 已含两态真值），
 * 仅影响卡片副标题（已连接·实时 IP vs 已登录·上次），由组件层据 proxyRunning 决定文案，不进本派生。
 */
import type { ServerConfig } from '../contracts/types';
import type { TailscaleStatusDetails } from '../contracts/tailscale-status';

export type TsCardState =
  | 'no-node' // 无 TS 节点 → 显示「连接 Tailscale」入口
  | 'key-ready' // 有 authKey（静态就绪，等同 WG，不显登录态）
  | 'logging-in' // 交互登录进行中（有 authUrl 且尚未登录）
  | 'connected' // 已登录（loggedIn=true，来源缓存/state/STATUS 任一）
  | 'needs-login'; // 交互型未登录（无 loggedIn、无 authUrl）

/**
 * 这一帧 STATUS 是否**足以裁定登录态**（W1 判决门）。只有 definitive 帧才许写
 * `setTailscaleLoginState`；非 definitive 帧一律丢弃、保留上一次已知值。
 *
 * # 为什么需要这道门（不加会把「已登录」翻成「未登录」并写穿缓存）
 *
 * 后端的 `loggedIn` 是**折叠值**：`backendState ∈ {Running, Starting} 且 self 未过期`
 * （`runtime/tailscale_status.rs:140`）。核起来的头几帧 backendState 是 `NoState` / `Starting` 之前的
 * 过渡态，折叠后 `loggedIn=false` —— 但那说的是「后端还没启完」，**不是**「这份凭据无效」。
 *
 * 无条件写下去有两层后果，第二层比第一层严重：
 *  ① 组网卡角标在每次连接时闪一下「未登录」；
 *  ② `setTailscaleLoginState` 是**双写**（内存 + localStorage 缓存，见 `app-store.ts` 该函数注释），
 *     于是这个假的 false 被**写穿进缓存**。而缓存正是代理关着时唯一能秒显登录态的来源
 *     （那时没有常驻核、没有 STATUS 流）⇒ 下次冷启动一进来就显示「需登录」，直到用户再连一次核。
 *
 * 判决口径（对齐 上游 `use-native-events.ts:309-318` 的 W1 门）：
 *  - **definitive-in**：`loggedIn === true` —— 后端已确认 Running/Starting 且未过期；
 *  - **definitive-out**：控制面明说这份凭据不能用 —— `NeedsLogin` / `NeedsMachineAuth` / key 已过期。
 *  - 其余（`NoState` / `Stopped` / `Starting` 折叠出的 false / 未知态）= 不知道，不写。
 *
 * 这与本仓既有语义自洽：**核停 ≠ 登录失效**（登录经 state 目录持久，见 `App.tsx` 该订阅上方注释）。
 */
export function isDefinitiveTsLoginFrame(frame: {
  backendState: string;
  loggedIn: boolean;
  expired: boolean;
}): boolean {
  if (frame.loggedIn) return true;
  return (
    frame.expired === true ||
    frame.backendState === 'NeedsLogin' ||
    frame.backendState === 'NeedsMachineAuth'
  );
}

/**
 * 该节点**存没存** authKey —— 全仓唯一的这条判据（`key-ready` 与设置弹窗的状态行同源）。
 *
 * # 为什么返回布尔而不是 key
 *
 * pre-auth key 是长期凭据。凡是「要不要显示 / 要不要走静态认证」这类问题，需要的只有存在性；
 * 把 key 本体递给调用方，泄露面就从「写它的那一个弹窗」扩到「每一个想知道有没有 key 的地方」——
 * 截图、录屏、演示、日志任取其一就带出去了。故本函数**只输出存在性**，渲染层拿到的永远是布尔。
 *
 * 口径与此前内联在 [`deriveTsCardState`] 里的那一句逐字相同（`?.trim()` 非空）：空白串不算凭据，
 * 它喂给 tsnet 等于没填。第二处若另写一遍 `!!ts?.authKey`，「全是空格的 key」就会在两处判出两个答案。
 */
export function hasTsAuthKey(node: ServerConfig | undefined): boolean {
  return !!node?.tailscaleSettings?.authKey?.trim();
}

export function deriveTsCardState(
  tsNode: ServerConfig | undefined,
  loggedIn: boolean | undefined,
  hasAuthUrl: boolean,
  loginActive = false
): TsCardState {
  if (!tsNode) return 'no-node';
  // authKey 形态优先：静态凭据，起核即认证，不进登录态/检测态（与 WG 同质）。
  if (hasTsAuthKey(tsNode)) return 'key-ready';
  // 交互登录中：登录【正在进行】(loginActive) 且有 URL 且尚未登录成功。loginActive = 用户显式发起(loginInitiated)
  // OR 该节点是当前选中出口（app 自动连接它=登录进行中，非被动 always-emit）。1.14 主核 always-emit 会为未选中/
  // 未就绪节点持续 emit AUTH_URL——若仅凭 hasAuthUrl 判 'logging-in'，卡片会被这些非活跃 URL 误推进「连接中」。
  // 故门控后：非选中且未手动发起时有 URL 只显 'needs-login'（可点角标登录）；选中出口自动连接 或 用户手点后进
  // 'logging-in'（修真机：首页弹登录时选中出口卡片应显「连接中」而非初始态）。loggedIn 一旦转 true 即落 'connected'。
  if (loginActive && hasAuthUrl && loggedIn !== true) return 'logging-in';
  if (loggedIn === true) return 'connected';
  return 'needs-login';
}

/**
 * Tailscale 账号标识文案：登录名 + tailnet（MagicDNS 后缀兜底），供节点卡片区分「同为已登录，
 * 但登录的是哪个账号 / 哪个 tailnet」——多账号场景下 `key-ready`/已登录这类登录态文案分不清是哪一个。
 *
 * tailnet 段（`networkName`，为空退 `magicDNSSuffix`）恒无歧义，直接取；两者都是控制面下发的
 * 同帧真值，非默认值。
 *
 * **登录名段只在无歧义时取，不猜**：只有 `userGroups.length === 1` 才用那一组的 `loginName`——
 * 该帧的对端按归属用户分组，恰好一组即代表本节点登录所属账号唯一，无歧义可言。
 *
 * `userGroups.length > 1` 时**不显示登录名**（不取 `[0]` 了事）：本节点自身（`details.self`）
 * 不落进任何一组——`TailscaleUserGroup` 分组的是「对端节点」（proto `started_service.proto`
 * `TailscaleUserGroup` 头注「对端节点按归属用户(owner/sharee)分组」），而 `self` 是
 * `TailscaleEndpointStatus` 的独立字段（f7，与 `userGroups` f8 平级），不算「对端」。
 * 且 `TailscaleStatusPeer`/`TailscalePeerDetails`（本文件 Rust 镜像 `tailscale_status.rs:30-65`）
 * 都不带 `userID`，无法反过来拿 self 的某个字段去匹配某一组。当前 wire 面没有任何字段能判定
 * self 归属哪一组——取 `[0]` 是**猜**，猜错的后果是把别的账号的登录名显示成用户自己的（比不显示
 * 更坏，用户会拿它做换 key / 删节点这类操作判断）。故 `>1` 组时只保留 tailnet 段，不摆一个
 * 可能猜错的登录名。
 *
 * `details` 缺席（未收到 STATUS 帧 / 该节点非 Tailscale）或两段都拿不到值 → `undefined`，
 * 调用方不渲染任何东西——空态纪律：没有真值就不画，不摆占位符、不摆可能错的近似值。
 */
export function tsAccountLabel(details: TailscaleStatusDetails | undefined): string | undefined {
  if (!details) return undefined;
  const tailnet = details.networkName.trim() || details.magicDNSSuffix.trim();
  const login = details.userGroups.length === 1 ? details.userGroups[0].loginName.trim() : '';
  if (login && tailnet) return `${login} · ${tailnet}`;
  return login || tailnet || undefined;
}
