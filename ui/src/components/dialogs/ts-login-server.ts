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

export type TsLoginMode = 'browser' | 'authkey';

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

/**
 * 依「有无既有 TS 节点 + 登录方式 + 控制面地址」算出提交计划。
 */
export function planTsLoginSubmit(input: TsLoginSubmitInput): TsLoginSubmitPlan {
  const { existing, mode, authKey, controlUrl, hasState, mintId } = input;
  // 绝不 mutate existing（它是 app-store.servers 里的 live 引用）——克隆基础对象 + 克隆 tailscaleSettings，
  // 提交失败不得把 authKey 写脏内存态。
  const server: ServerConfig = existing
    ? { ...existing, tailscaleSettings: { ...(existing.tailscaleSettings ?? {}) } }
    : ({
        id: mintId(),
        name: 'Tailscale',
        protocol: 'tailscale',
        // tailscale 是账号制协议，后端 sanitize 豁免 address/port 校验（crates/store/src/sanitize.rs:250）。
        address: '',
        port: 0,
        // 空设置即默认：allowInternet / alwaysRouteSubnets 缺省均为 true（contracts/types/protocol-settings.ts:179,182），
        // 显式重写一遍只会制造第二个默认值真值源。
        tailscaleSettings: {},
      } as ServerConfig);

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
    persist: keyChanged || controlChanged ? 'update' : 'none',
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
    return { phase: 'failed', reason: code === 'TAILSCALE_LOGOUT_MAIN_CORE' ? 'mainCoreInUse' : stage };
  } finally {
    if (!handedOff) await input.cancel().catch(() => {});
  }
}
