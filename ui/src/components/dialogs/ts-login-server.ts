/**
 * TsLoginDialog 提交前的「节点落盘决策」纯逻辑（与组件分离，走 node 环境单测）。
 *
 * 为何登录前必须先落盘节点：后端把登录产物按 `server.id` 分键存——登录配置写
 * `tailscale-login-<id>.json`（runtime/tailscale_login_core.rs:348），state 目录同样按 id 分
 * （crates/mesh/src/tailscale_login.rs）。拿 `id: ''` 直接发起登录，登录成果会落在空串键下，与日后
 * 任何真实节点 id 永不匹配 = 成果孤儿化；且 config 里从头到尾不会出现 Tailscale 节点。
 *
 * 为何 id 由渲染端 mint 而不是取 `add` 的返回值：`server_add`（commands/server.rs:65）返回
 * `ApiResponse<()>`，**不回传新建节点**（api-client 的 `Promise<ServerConfig>` 签名与后端不符，见
 * 交付说明）；而 `ensure_server_id`（同文件 :40）只在 id 缺失/空串时才 mint，非空 id 原样保留 →
 * 「渲染端自带 id 落盘」是后端明确支持的契约（其单测名即 `..._keeps_existing`）。
 */
import type { ServerConfig } from '@/contracts/types';
import type { TailscaleStatusSnapshot } from '@/contracts/tailscale-status';

export type TsLoginMode = 'browser' | 'authkey';

/**
 * 「等登录地址」的放弃时限，对齐后端瞬态登录核的超时臂
 * （`src-tauri/src/runtime/tailscale_login_core.rs:54 DEFAULT_LOGIN_TIMEOUT = 300s`）。
 *
 * 到点只代表仍未收到地址，不代表授权失败或核已停止。取消的结果以 awaited 后端回执为准。
 * 移动端只把这条时限用于本面板启动的瞬态核；主核 STATUS/URL 等待不能触发它。
 *
 * 🔴 住在这里而不是各自的表单里：桌面 `TsLoginDialog` 与 `mobile/forms/TsLoginPanel` 都要用它，
 * 两份常量必然在某天分叉，而分叉的表现是「一端说超时了、另一端还在转」。
 */
export const TS_LOGIN_TIMEOUT_MS = 300_000;

/**
 * 瞬态浏览器登录此刻该画哪一档 —— 两端共用。主核活态另见 tsLoginMainCoreView。
 *
 * # 为什么这条也要收进共用 `.ts`
 *
 * 移动端面板此前在这条链的最前面自己加了一档 `loggedIn`：
 * `tailscaleLoginStates[pendingServerId] === true` ⇒ 画「已提交，正在连接…」。那张表答的是
 * **「这个节点历史上登录成过没有」**（初值来自 localStorage 缓存 `loadTailscaleLoginStatesFromCache`，
 * `reset` 时刻意不清），不是「本次登录成没成」。于是对一个**此前登录过**的 Tailscale 节点重新
 * 登录（接入面 / 设置表末尾的「切换账号」，`planTsLoginSubmit` 复用既有节点 ⇒ id 不变），
 * `setPendingServerId(server.id)` 那一刻这一档当场命中，登录地址、复制/打开两颗重试键、
 * 以及超时那一臂**全部不可达**；而没有任何东西会把它翻回 false（瞬态登录核只发
 * `EVENT_TAILSCALE_AUTH_URL`，`EVENT_TAILSCALE_STATUS` 只有主核发，主核里没有这个 endpoint）。
 *
 * ⇒ 「本次登录走到哪一步」的真值只有三个：本次提交拿到的 `pendingServerId`、该 id 下的
 * `authUrl`、以及本地超时臂。历史登录态不参与这条判决（它驱动的是节点屏/首页卡片的角标）。
 * 判据落在这里而不是各自的组件里：桌面 `TsLoginDialog` 与 `mobile/forms/TsLoginPanel` 的四档
 * 顺序必须逐字一致，两份嵌套三元必然在某天分叉，而分叉的表现正是上面那一档。
 */
export type TsLoginBrowserView = 'url' | 'timeout' | 'awaiting' | 'hint';

/**
 * @param pendingServerId 本次提交起的那个登录核对应的节点 id（`null` = 还没提交过）。
 * @param authUrl 该 id 下已到达的登录地址（`null` = 还没到）。
 * @param timedOut 本地等地址超时臂是否已到点。
 */
export function tsLoginBrowserView(
  pendingServerId: string | null,
  authUrl: string | null,
  timedOut: boolean
): TsLoginBrowserView {
  // 地址优先：拿到了就先把地址给用户（自动开浏览器会失败，那两颗重试键是唯一补救）。
  if (authUrl !== null) return 'url';
  if (timedOut) return 'timeout';
  if (pendingServerId !== null) return 'awaiting';
  return 'hint';
}

/** 主核存在不等于已授权；仅消费本次主动读取/订阅得到的活态，禁止用历史登录缓存。 */
export function tsLoginMainCoreView(
  serverId: string,
  snapshot: TailscaleStatusSnapshot | null,
  receivedUrl: string | null,
): { state: 'unknown' | 'authorized' | 'needs-login' | 'url'; authUrl: string | null } {
  const frame = snapshot?.connected ? snapshot.statuses.find((s) => s.serverId === serverId) : undefined;
  if (frame?.loggedIn && !frame.expired && frame.backendState === 'Running') {
    return { state: 'authorized', authUrl: null };
  }
  const authUrl = frame?.authURL?.trim() || receivedUrl;
  if (authUrl) return { state: 'url', authUrl };
  if (frame && (frame.expired || ['NeedsLogin', 'NeedsMachineAuth'].includes(frame.backendState))) {
    return { state: 'needs-login', authUrl: null };
  }
  return { state: 'unknown', authUrl: null };
}

/** 错误只用稳定码分类；message 可能包含 auth key、控制面 URL 或本地路径。 */
export function tsLoginFailureKey(error: unknown): 'ts.loginStartFailed' | 'ts.loginAttemptFailed' | 'errors.androidNativeCapacityClosed' {
  if (typeof error === 'object' && error !== null && 'code' in error && error.code === 'ANDROID_NATIVE_LEDGER_CAPACITY_CLOSED') return 'errors.androidNativeCapacityClosed';
  return typeof error === 'object' && error !== null && 'code' in error && error.code === 'TAILSCALE_LOGIN_FAILED'
    ? 'ts.loginStartFailed'
    : 'ts.loginAttemptFailed';
}

export interface TsLoginSubmitPlan {
  /** 发给 `tailscaleLogin` 的节点（id 恒为真实 id，绝不为空串）。 */
  server: ServerConfig;
  /** 发起登录前需要的落盘动作。 */
  persist: 'add' | 'update' | 'none';
  /**
   * 提交前是否必须先退出登录（清 state 目录）。
   *
   * **这条是「auth_key 换不上」的根因**：tsnet 手上只要有有效 node key 就不会去用 `auth_key`，
   * 于是用户填了新 key、提交也成功，登录身份却一动不动。清 state 目录是唯一的解
   * （`commands/server.rs` 的 `tailscale_logout` 注释：「清 state 目录；保留节点配置/authKey」）。
   *
   * 只在「切到 authkey 且该节点确实有 state」时为真：browser 模式本来就是交互登录，
   * 而没有 state 的节点没什么可退。
   */
  requiresLogout: boolean;
}

/** 提交计划的输入。用对象而非位置参数：五个入参里三个是字符串，位置写反了类型系统看不出来。 */
export interface TsLoginSubmitInput {
  /** 既有的 Tailscale 节点（首次接入时缺席）。 */
  existing?: ServerConfig;
  /** 用户填写的节点名称；桌面旧调用方未传时保留原名称或默认名称。 */
  name?: string;
  mode: TsLoginMode;
  authKey: string;
  /**
   * 自建控制面地址（headscale 等）。空串 = 官方 controlplane。
   *
   * **必须能在登录弹窗里填**：登录核确实透传它（`crates/mesh/src/tailscale_login.rs`），
   * 但此前只有「TS 设置」弹窗有这个控件，而那个弹窗要求节点**已存在** ——
   * 于是自建控制面用户的第一次登录必然打向官方 controlplane，只能先登错一次再改。
   */
  controlUrl: string;
  /** 该节点当前是否已有登录 state（`tailscaleStateExists`）。无既有节点时传 false。 */
  hasState: boolean;
  /** 注入而非直接调 `crypto.randomUUID`，使单测可断言 id 去向。 */
  mintId: () => string;
}

/** 为组网 TAB 中的新 Tailscale 节点找一个未占用的默认显示名。 */
export function nextTsNodeName(names: readonly string[]): string {
  const used = new Set(names.map((name) => name.trim().toLowerCase()));
  if (!used.has('tailscale')) return 'Tailscale';
  let suffix = 2;
  while (used.has(`tailscale ${suffix}`)) suffix++;
  return `Tailscale ${suffix}`;
}

/**
 * 依「有无既有 TS 节点 + 登录方式 + 控制面地址」算出提交计划。
 */
export function planTsLoginSubmit(input: TsLoginSubmitInput): TsLoginSubmitPlan {
  const { existing, name, mode, authKey, controlUrl, hasState, mintId } = input;
  // 绝不 mutate existing（它是 app-store.servers 里的 live 引用）——克隆基础对象 + 克隆 tailscaleSettings，
  // 提交失败不得把 authKey 写脏内存态。
  const server: ServerConfig = existing
    ? { ...existing, tailscaleSettings: { ...(existing.tailscaleSettings ?? {}) } }
    : ({
        id: mintId(),
        name: name?.trim() || 'Tailscale',
        protocol: 'tailscale',
        // tailscale 是账号制协议，后端 sanitize 豁免 address/port 校验（crates/store/src/sanitize.rs:250）。
        address: '',
        port: 0,
        // 空设置即默认：allowInternet / alwaysRouteSubnets 缺省均为 true（contracts/types/protocol-settings.ts:179,182），
        // 显式重写一遍只会制造第二个默认值真值源。
        tailscaleSettings: {},
      } as ServerConfig);
  if (existing && name !== undefined) server.name = name.trim();

  if (mode === 'authkey') {
    server.tailscaleSettings = {
      ...(server.tailscaleSettings ?? {}),
      authKey: authKey.trim(),
    };
  } else {
    delete server.tailscaleSettings?.authKey;
  }

  // controlUrl 与登录方式无关（两种方式都要打向同一个控制面），故不放在 authkey 分支里。
  // 缺省即删键：空串写进配置只会让「用官方控制面」这件事在磁盘上多一个等价噪声键。
  const trimmedControl = controlUrl.trim();
  const nextSettings = { ...(server.tailscaleSettings ?? {}) };
  if (trimmedControl) nextSettings.controlUrl = trimmedControl;
  else delete nextSettings.controlUrl;
  server.tailscaleSettings = nextSettings;

  const requiresLogout = mode === 'authkey' && hasState;

  if (!existing) return { server, persist: 'add', requiresLogout };
  // 已有节点：只有**真的变了**才写盘。browser 模式不改任何字段时不碰配置 → 免掉一次无谓的
  // CONFIG_CHANGED 广播（后端据此重启代理）。
  const keyChanged =
    server.tailscaleSettings?.authKey !== existing.tailscaleSettings?.authKey;
  const controlChanged =
    server.tailscaleSettings?.controlUrl !== existing.tailscaleSettings?.controlUrl;
  return {
    server,
    persist: keyChanged || controlChanged || server.name !== existing.name ? 'update' : 'none',
    requiresLogout,
  };
}

export interface TsLoginExecution {
  isActive: () => boolean;
  prepare: () => Promise<void>;
  verifyState?: () => Promise<boolean>;
  logout: () => Promise<unknown>;
  save: () => Promise<void>;
  onSaved: () => void;
  refresh: () => Promise<unknown>;
  start: () => Promise<{ started: boolean; reason?: string }>;
  cancel: () => Promise<void>;
}

/** Save and authorize are separate transactions. Cancellation is checked at every IPC boundary. */
export async function executeTsLogin(input: TsLoginExecution): Promise<{ phase: 'handedOff' | 'cancelled' | 'failed'; reason?: string }> {
  let handedOff = false;
  let stage = 'authorizationRequestFailed';
  try {
    await input.prepare();
    if (!input.isActive()) return { phase: 'cancelled' };
    if (input.verifyState) {
      stage = 'stateQueryFailed';
      const exists = await input.verifyState();
      if (!input.isActive()) return { phase: 'cancelled' };
      if (exists) {
        stage = 'logoutFailed';
        await input.logout();
        if (!input.isActive()) return { phase: 'cancelled' };
      }
    }
    stage = 'saveFailed';
    await input.save();
    input.onSaved();
    if (!input.isActive()) return { phase: 'cancelled' };
    stage = 'configurationRefreshFailed';
    await input.refresh();
    if (!input.isActive()) return { phase: 'cancelled' };
    stage = 'authorizationRequestFailed';
    const result = await input.start();
    if (!input.isActive()) return { phase: 'cancelled' };
    handedOff = result.started || result.reason === 'inMainCore';
    return { phase: handedOff ? 'handedOff' : 'cancelled' };
  } catch (error) {
    const code = error && typeof error === 'object' && 'code' in error ? error.code : null;
    if (code === 'ANDROID_NATIVE_LEDGER_CAPACITY_CLOSED') return { phase: 'failed', reason: code };
    return { phase: 'failed', reason: code === 'TAILSCALE_LOGOUT_MAIN_CORE' ? 'mainCoreInUse' : stage };
  } finally {
    if (!handedOff) await input.cancel().catch(() => {});
  }
}
