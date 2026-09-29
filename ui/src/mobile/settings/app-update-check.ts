/**
 * 移动端**应用更新检查**的会话态（W-19 / W-20）。
 *
 * # 为什么是一份会话态，而不是各页各查一次
 *
 * 有两个消费者：设置**根页**那行「有新版本」入口（W-20）与**更新页**那张卡（W-19）。
 * 各自 `useState` + 各自查一次的下场是：进一次设置发两次 GitHub 请求，且两处可能显示不同的答案
 * （一次成功一次失败）。故快照住在模块级，两处读同一份 —— 与桌面 `app-update-banner.ts` 的
 * 「会话级已跳过集合」同一套形态（那份也是被横幅与设置页更新卡共用的模块态）。
 *
 * 它是**会话态**、不是第二真值源：进程退出即丢；后端 `update_check` 仍是唯一的事实来源
 * （持久化的 `skippedVersion` 由它过滤，见 `commands/updater.rs:248`）。
 *
 * # 🔴 快照里为什么要带**清单**（2026-09-13 加）
 *
 * 下载那条腿的对象是一份 `UpdateProgressManifest`（下载地址 / 文件名 / 体积 / 期望 sha256），
 * 而这些只在**这一次检查**的回包里出现过。此前这里只留了 `{hasUpdate, version}`，
 * 于是「下载」这条腿在前端根本没有目标可交 —— 所以它与检查腿必须同批落地。
 *
 * [`appUpdateDownloadTarget`] 是那份清单的**唯一**出口，且它有牙：三个资产字段任一为空就返
 * `null`。Android 上「有新版本但那个 release 没发 APK」是一档**真实存在**的结果
 * （2026-09-13 之前的每一个 release 都是），后端如实回空资产字段，前端据此**不画**下载按钮，
 * 退回「打开发布页」。少了这道判据，那一档会得到一颗点下去必然失败的下载按钮。
 *
 * # 判据怎么驱动它（**测试里不许打真网**）
 *
 * [`runAppUpdateCheck`] 吃一个 `check` 函数而不是直接 import `updateApi`：判据喂一个假的即可
 * 逐支驱动（有更新 / 没更新 / 抛错），**一个真实请求都不发**。生产那一跳由
 * [`checkAppUpdateViaIpc`] 绑定，`UpdatePage` 把它喂进去；「容器把参数丢掉、恒传一个假的」
 * 这一档由判据对那一跳单独钉（`MobileSettings.test.tsx` ⑱ 与 `privacy-lock.test.tsx` 都按这条
 * 口径写）。
 */

import type { UserConfig } from '@/contracts/types';
import { updateApi, type UpdateProgressManifest } from '@/ipc/api-client';
import { appUpdateIncludePrerelease } from '@/domain/app-update-channel';
import {
  appUpdateBannerState,
  type AppUpdateSnapshot,
} from '@/components/layout/app-update-banner';

/** 一次检查的四态。`idle` = 本会话还没查过。 */
export type AppUpdatePhase = 'idle' | 'checking' | 'done' | 'error';

export interface AppUpdateCheckState {
  readonly phase: AppUpdatePhase;
  /** `phase === 'done'` 时的结果；其余为 `null`。 */
  readonly snapshot: AppUpdateSnapshot | null;
  /**
   * 这次检查查到的那份**可下载清单**；`null` = 没有可下载的资产。
   *
   * `null` 有两种成因，对用户是同一句话（「去发布页拿」），故这里不分：
   * 没有更新，或者有更新但那个 release 没发本平台的包（Android 上真实存在，见头注）。
   */
  readonly target: UpdateProgressManifest | null;
  /** `phase === 'error'` 时后端/传输给的原始原因（可能为空串）。 */
  readonly reason: string;
}

const IDLE: AppUpdateCheckState = { phase: 'idle', snapshot: null, target: null, reason: '' };

let state: AppUpdateCheckState = IDLE;
const listeners = new Set<() => void>();

/**
 * 本会话有没有跑过**自动**检查（设置根页那条后台腿）。
 *
 * 🔴 **必须是模块态，不能是组件里的 `useRef`**（2026-09-06 复审 minor，已修）。
 * `MobileSettingsScreen` 随底部导航切换整个卸载重挂（`Screens.tsx` 的五个目的地是五个**不同的**
 * 组件函数，`MobileApp` 取 `SCREENS[active]` 渲染 ⇒ 元素类型变化 = 卸载 + 挂载），组件里的 ref
 * 随之复位。此前那把闸装在组件上，实际语义是「每次进设置查一次」而不是注释声称的「每会话一次」：
 * 「首页→设置」来回三次就发三次 GitHub 请求（未认证 API 限额 60 次/小时/IP）。
 * 更实的一档是**已查到的结果会被抹掉**：每次重查开头 `publish({phase:'checking', snapshot:null})`,
 * 若这次被限流就落 `error`、`snapshot` 恒 `null` ⇒ 根页那行「有新版本」在本会话余下时间再也不出现，
 * 而新版本确实存在。
 */
let autoChecked = false;

function publish(next: AppUpdateCheckState): void {
  state = next;
  for (const listen of [...listeners]) listen();
}

/** 订阅快照变化；返回退订闭包。 */
export function subscribeAppUpdateCheck(onChange: () => void): () => void {
  listeners.add(onChange);
  return () => {
    listeners.delete(onChange);
  };
}

/** 读当前快照。`useSyncExternalStore` 的 live 与 server 两侧共用它（同 `MobileShell.usePushedPage`）。 */
export function readAppUpdateCheck(): AppUpdateCheckState {
  return state;
}

/** 判据用：把会话态复位（模块态跨用例存活，不复位会让上一条用例的结果污染下一条）。 */
export function resetAppUpdateCheck(): void {
  autoChecked = false;
  publish(IDLE);
}

/**
 * 跑一次**自动**检查（设置根页那条后台腿）。本会话只真的发一次请求，之后恒早退。
 *
 * 与 [`runAppUpdateCheck`]（手动那颗按钮走的那条）分开：手动那次是用户按下去、正等着回执的，
 * 它**必须**每次都真的去查（否则「检查更新」这颗按钮在第二次点下去时什么都不做）。
 * 两条腿共用同一份会话快照，故自动那次查到的结果手动那侧也读得到。
 *
 * 返回值只给判据用：`true` = 这一拍真的发了一次请求。
 */
export async function runAutoAppUpdateCheck(
  check: AppUpdateChecker,
  config: Pick<UserConfig, 'appUpdateChannel'> | null,
): Promise<boolean> {
  if (autoChecked) return false;
  autoChecked = true;
  await runAppUpdateCheck(check, config);
  return true;
}

/** [`runAppUpdateCheck`] 需要的最小后端面。生产是 [`checkAppUpdateViaIpc`]，判据喂假的。 */
export type AppUpdateChecker = (options: {
  includePrerelease: boolean;
  /** 「重装当前版本」那条腿才传：放宽「必须更高版本」这一道发现策略。 */
  includeCurrent?: boolean;
}) => Promise<{
  hasUpdate: boolean;
  /** `includeCurrent` 显式请求命中与已装版本相同的 release；**不是**「有新版本」。 */
  isCurrentVersion?: boolean;
  /**
   * 形状是「清单的任意子集 + 必有 `version`」而不是整个 `UpdateProgressManifest`：
   * 后端在**没有适配资产**那一档如实回空的资产字段（`downloadUrl: ''` / `fileSize: 0`），
   * 而判据喂的假回包也只关心自己那几格。要不要当成可下载目标由
   * [`appUpdateDownloadTarget`] 一处判，不靠这个类型去挡。
   */
  updateInfo?: (Partial<UpdateProgressManifest> & { version: string }) | undefined;
  error?: string;
}>;

/**
 * 「这次查到的东西能不能下载」的**唯一**判据。`null` = 不能，界面据此不画下载按钮。
 *
 * 三格全要，且各有各的承重：
 *  · `downloadUrl` 空 ⇒ 后端明写「调用方**不得**发起下载」（`check_app_update_release_only`
 *    的头注）—— 那一档没有选中的资产；
 *  · `fileName` 空 ⇒ 下载腿会落到一个兜底名 `polaris-update` 上，而安装腿按后缀分流
 *    （`classify_installer` 只认 `.apk/.exe/.dmg/.appimage/.deb`）⇒ 下得到、装不了；
 *  · `fileSize` 为 0 ⇒ 写入闸会退成 `APP_UPDATE_MAX_BYTES`，且「清单声明体积」这一级
 *    完整性校验整条失效（无摘要腿的主防线就是它）。
 *
 * 🔴 判据是**正面**的（三格都得有值），不是「不许是空串」：一个字段整个缺席
 * （`undefined`）在否定式判据下会溜过去，而它恰恰是后端契约破损时最可能的形态。
 */
export function appUpdateDownloadTarget(
  info: (Partial<UpdateProgressManifest> & { version: string }) | undefined,
): UpdateProgressManifest | null {
  if (!info) return null;
  const { version, downloadUrl, fileName, fileSize } = info;
  if (!version || !downloadUrl || !fileName) return null;
  if (typeof fileSize !== 'number' || fileSize <= 0) return null;
  return {
    version,
    downloadUrl,
    fileName,
    fileSize,
    publishedAt: info.publishedAt ?? '',
    isPrerelease: info.isPrerelease ?? false,
    ...(info.sha256 === undefined ? {} : { sha256: info.sha256 }),
  };
}

/**
 * 生产那一跳：真的那条 IPC。**全仓只在这里出现一次**，别处一律经参数拿。
 *
 * 通道字段**显式重铺**（不是把 `options` 整个转手）：`settings-logic.test.ts` 那道
 * 「全仓前端检查入口必须显式携带持久化通道」的门按调用点的**实参文本**扫，转手一个变量会让
 * 这条入口在它眼里不带通道 —— 而那道门守的正是「某个入口偷偷按缺省通道查」这件事。
 */
export function checkAppUpdateViaIpc(options: {
  includePrerelease: boolean;
  includeCurrent?: boolean;
}): ReturnType<AppUpdateChecker> {
  return updateApi.check({
    includePrerelease: options.includePrerelease,
    includeCurrent: options.includeCurrent ?? false,
  });
}

/**
 * 「重装当前版本」（修复损坏安装）的解析腿：返回**当前版本**那份可下载清单。
 *
 * 三条早退，一条都不许省：
 *  · `hasUpdate` 为真 ⇒ **不替用户去下另一个目标**（逐字同桌面 `reinstallCurrent` 的取向：
 *    真有新版时只展示新版，让用户自己决定装哪个）；
 *  · `isCurrentVersion` 不为真 ⇒ 这个通道的最新 release 与已装版本对不上（比它旧或比它新），
 *    继续下去就是一次**静默降级**；
 *  · 清单不可下载（三个资产字段任一为空）⇒ 这一档没有对象，见 [`appUpdateDownloadTarget`]。
 *
 * `null` 的三种成因对用户是同一句话（`settings.update.reinstallUnavailable`，五语齐，
 * 与桌面共用），故这里不分档 —— 分了也给不出不同的下一步。
 */
export function reinstallTarget(
  result: Awaited<ReturnType<AppUpdateChecker>>,
): UpdateProgressManifest | null {
  if (result.hasUpdate) return null;
  if (result.isCurrentVersion !== true) return null;
  return appUpdateDownloadTarget(result.updateInfo);
}

/**
 * 跑一次检查。返回的 promise **在失败时 reject** —— 调用方（更新页）把它交给 `commit`，
 * 于是失败既进会话态、也落成那一行红字。
 *
 * 并发保护：已经在 `checking` 时直接返回，不发第二次请求（用户连点「检查更新」是常态）。
 *
 * 通道（正式版 / 测试版）走**共享**的 `appUpdateIncludePrerelease`，与桌面横幅、后端启动检查
 * 同一份判定 —— 移动端另写一遍就会在「用户切到测试版通道」时与别处分叉。
 */
export async function runAppUpdateCheck(
  check: AppUpdateChecker,
  config: Pick<UserConfig, 'appUpdateChannel'> | null,
): Promise<void> {
  if (state.phase === 'checking') return;
  publish({ phase: 'checking', snapshot: null, target: null, reason: '' });
  try {
    const result = await check({
      includePrerelease: appUpdateIncludePrerelease(config),
    });
    /* 后端把「查了但没查成」放在 `error` 字段里而不是抛 —— 折成 `done` 会把一次失败显示成
       「已是最新」，那是本模块最不能犯的错（用户据此认为自己在最新版上）。 */
    if (result.error) {
      publish({ phase: 'error', snapshot: null, target: null, reason: result.error });
      throw new Error(result.error);
    }
    publish({
      phase: 'done',
      snapshot: { hasUpdate: result.hasUpdate, version: result.updateInfo?.version ?? null },
      /* `hasUpdate` 为假时**不留目标**：`includeCurrent` 那条支路（桌面「重新下载当前版本」）
         也会带回一份清单，而这一屏没有那颗按钮 —— 留着它就等于给「已是最新」配一颗下载按钮。 */
      target: result.hasUpdate ? appUpdateDownloadTarget(result.updateInfo) : null,
      reason: '',
    });
  } catch (err) {
    if (state.phase !== 'error') {
      publish({
        phase: 'error',
        snapshot: null,
        target: null,
        reason: err instanceof Error ? err.message : '',
      });
    }
    throw err;
  }
}

/* ── 两个消费点各自的**纯**折算（组件只做 `t()` 与渲染）───────────────────────── */

/** 更新行那句提示：`null` = 这一态不该说任何话。 */
export interface AppUpdateHint {
  readonly key: string;
  /** 带版本号的那一支才有。 */
  readonly version?: string;
}

/**
 * 更新行的提示（W-19）。四态各说各的，**一条都不许折叠**：
 *  · `idle` / `checking` —— 不说。按钮自己已经在说「正在检查…」，再来一句是同一事实的第二份副本；
 *  · `done` 且有新版本 —— 说版本号（拿不到版本号就只说「发现新版本」：`hasUpdate:true` 却缺
 *    version 是后端契约破损，宁可少说也不显示一个空的 `v`，同桌面 `appUpdateBannerState` 的口径）；
 *  · `done` 且没有 —— 说「已是最新」；
 *  · `error` —— **不说**。失败那句由 `commit` 落成同一行下面的红字，两处各说一遍会得到
 *    「检查更新失败」＋「已是最新」这种自相矛盾的组合。
 */
export function appUpdateHint(state: AppUpdateCheckState): AppUpdateHint | null {
  if (state.phase !== 'done' || state.snapshot === null) return null;
  if (!state.snapshot.hasUpdate) return { key: 'settings.update.upToDate' };
  const version = state.snapshot.version;
  return version === null
    ? { key: 'settings.update.foundNew' }
    : { key: 'settings.update.bannerTitle', version };
}

/**
 * 根页那行「有新版本」入口该不该在、显示哪个版本（W-20）。`null` = 不画那一行。
 *
 * 判定**整个借用桌面 `appUpdateBannerState`**（不重写）：那四道闸（查到了 / 版本号非空 /
 * 本会话没跳过 / 本会话没关掉）连同「失败不算有更新」的取向都已经在那边由单测锁死，
 * 移动端再写一遍必然与它漂移 —— 而漂移的表现是「桌面不提示了、手机还在提示同一个版本」。
 */
export function appUpdateEntryVersion(
  state: AppUpdateCheckState,
  skipped: ReadonlySet<string>,
  dismissed: ReadonlySet<string>,
): string | null {
  return appUpdateBannerState({ snapshot: state.snapshot, skipped, dismissed }).version;
}
