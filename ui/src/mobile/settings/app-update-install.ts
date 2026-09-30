/**
 * 移动端「下载 → 交系统安装器」那一跳的**纯折算**（组件只做 `t()` 与渲染）。
 *
 * # 这一跳与桌面那条腿的形状差别
 *
 * 桌面 `update_install` 会写一个安装脚本、停代理、detached 起脚本、退出应用。Android 一件都不做：
 * 它经 FileProvider 把 APK 交给**系统安装器**，然后本进程**继续活着等**用户在系统 UI 上按确认
 * （后端 `app_update.rs` 的 Android 分支有一整段写这件事）。于是这一侧要认的结局与桌面完全不同：
 *
 * | 回包 | 含义 | 界面 |
 * |---|---|---|
 * | `{ok:true, awaitingSystemInstaller:true}` | 包交出去了，系统安装器已经在前台 | 一行「已交给系统安装器」+ 签名一致性预告 |
 * | `{ok:false, awaitingSystemInstaller:false, reason}` | 交不出去，且知道为什么 | 按 `reason` 逐码取文（五个码五句话） |
 * | 抛错 | 桥本身坏了（没接线 / 超时 / Kotlin 抛异常） | 走通用失败取文 |
 *
 * # 🔴 三档失败里，只有两档是应用**观测得到**的
 *
 * 用户视角的三档是「来源被禁」「包损坏」「签名不符」。前两档有确定的观测点：
 *  · **来源被禁** —— `installApk` 在 `startActivity` **之前**显式问过
 *    `canRequestPackageInstalls()`，答案随回包带上来（`unknown-sources-*` 两个码）；
 *  · **包损坏 / 被篡改** —— 在**下载**那一步就判掉了（`update_download` 的 sha256 强校验与
 *    「清单声明体积」等值判据），码是 `digestMismatch` / `sizeMismatch` / `digestHexInvalid`，
 *    走 `appUpdateErrText` 取文，根本走不到交付这一步。
 *
 * **签名不符这一档，应用内没有观测点**（如实登记，不假装）：交付走的是 `ACTION_VIEW`，
 * 交出去之后**没有任何回调**；系统安装器自己弹「应用未安装」，那件事发生在本进程之外。
 * 要观测它得改成 `PackageInstaller` 会话式（自起 `IntentSender` 接收器、处理
 * `STATUS_PENDING_USER_ACTION`），那是 Kotlin 侧的架构改动，不在本批。
 * 故这一档只能**预告**：交付成功那一行同时讲清「若系统提示『应用未安装』，多半是这个包与你
 * 已装的那份签名不一致 —— 去发布页拿官方包重装」。预告写在
 * `mobileSettings.update.handedOffNote` 上，与「已交给系统安装器」同一行。
 */
import type { AndroidInstallReason, UpdateInstallResult } from '@/ipc/api-client';

/**
 * 交付结局。`handed-off` 是**成功**（包出去了，本进程还活着在等），不是终态 ——
 * 装没装成发生在系统安装器里，应用看不见。
 */
export type InstallHandoff =
  | { readonly kind: 'handed-off' }
  | { readonly kind: 'refused'; readonly reason: string };

/**
 * 把 `updateApi.install` 的回包折成交付结局。
 *
 * 🔴 判据是 `awaitingSystemInstaller === true`，**不是** `ok`：那个字段是这条腿独有的键
 * （后端刻意没有复用 `handedToSystem`，因为那个键在本仓的唯一语义是「形态错配、放弃安装」）。
 * 按 `ok` 判会让「字段整个不存在」的回包（桌面那条脚本腿、或后端改了形状）静默落进成功侧。
 */
export function classifyInstallHandoff(result: UpdateInstallResult): InstallHandoff {
  if (result.awaitingSystemInstaller === true) return { kind: 'handed-off' };
  return { kind: 'refused', reason: result.reason ?? '' };
}

/**
 * 五个 `REASON_*` 码各自的文案键。表外的码（含空串）落一句**不编原因**的兜底。
 *
 * 🔴 **一个码一句话**：前两个是「按一下开关就能继续，而且应用已经把你送到那一页了」，
 * 第三个是「本机装不了」，后两个是接线错误 / 包不见了 —— 折成一句「安装失败」，
 * 等于对前两种情形的用户说一句做不到的话。这正是 Kotlin 侧那五个常量分开的全部理由
 * （`PolarisVpnPlugin.kt` 的 `openUnknownSourcesSettings` 头注写着同一条）。
 */
const REASON_KEYS: Readonly<Record<AndroidInstallReason, string>> = {
  'unknown-sources-denied': 'mobileSettings.update.installUnknownSourcesDenied',
  'unknown-sources-settings-unavailable': 'mobileSettings.update.installUnknownSourcesNoSettings',
  'no-installer-activity': 'mobileSettings.update.installNoInstaller',
  'package-not-app-private': 'mobileSettings.update.installPathRejected',
  'package-missing': 'mobileSettings.update.installPackageMissing',
};

/**
 * 交付失败的文案键。**不认识的码不编原因**：回落到一句只说「没能交给系统安装器」的话，
 * 而不是把那个机器码贴进界面（开发者信息外露，元规则 #5），也不是挑一个看起来像的码去顶。
 */
export function androidInstallFailureKey(reason: string): string {
  return (
    REASON_KEYS[reason as AndroidInstallReason] ?? 'mobileSettings.update.installHandoffFailed'
  );
}

/** 判据用：五个码一个不少地登记了（表缩水时当场红）。 */
export const ANDROID_INSTALL_REASONS: readonly AndroidInstallReason[] = Object.keys(
  REASON_KEYS,
) as AndroidInstallReason[];
