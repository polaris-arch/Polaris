/**
 * 设置 → 更新（移动端）。桌面对照：`components/screens/settings/SettingsUpdate.tsx`。
 *
 * # 应用更新卡按平台分叉（IA §4.5）
 *
 * 桌面有两张独立的卡：应用（`AppUpdateCard`）与内核（`CoreUpdateCard`，带分级更新、回滚、恢复出厂）。
 * 移动端只显示应用更新。内核更新没有独立于 APK 的对象：
 *
 *  · **内核**：移动端内核不是 sidecar 子进程，而是随应用进程载入的 libbox。
 *
 *    🔴 **形态不是「未定」，是「随应用一起更新」（2026-09-06 改正）**：libbox 以 `libs/libbox.aar`
 *    打进 APK（`src-tauri/gen/android/app/build.gradle.kts:226 implementation(files("libs/libbox.aar"))`），
 *    核在 Android 上是**进程内 `.so`** 而非可执行文件（`src-tauri/src/lib.rs:635-641` 与
 *    `runtime/proxy/process_supervision.rs:128 if cfg!(target_os = "android")`），
 *    `runtime/proxy/android_bridge.rs` 整模块 `#[cfg(target_os = "android")]`。
 *    ⇒ 桌面那套 `core_swap`（`<core>.bak` 原子替换可执行文件）在这个形态下**没有对象**：
 *    换内核 = 装一个新版本的应用。独立内核更新行即使只显示“内置”，也在可操作的更新卡里
 *    暗示另一条能力；移动端直接不显示它，应用 APK 更新保持原样。
 *  · **应用**：Android 走 GitHub Releases 的 APK 分发。**2026-09-06（W-19）接上检查 + 显示 +
 *    打开发布页；2026-09-13（批 15）接上下载 → 交系统安装器那一跳**，四件事齐：
 *    检查 / 下载 / 交系统安装器 / 重装当前版本，外加「自动下载新版本」那一格。
 *
 *    🔴 **这一跳的语义是「下载官方 release 的签名包并交给系统安装器」，不是「下载任何 APK」**。
 *    Android 只接受与已装应用**同一把签名**的升级包，故：
 *     · 下载侧照桌面同一条腿做完整性校验（`update_download` 的 sha256 强校验 +「清单声明体积」
 *       等值判据，见 `updateApi.download` 的 JSDoc），校验不过**绝不落位**、更不交出去；
 *     · 交付侧只认应用私有 cache 目录下的包（Kotlin `handOffToSystemInstaller` 的前缀判据）；
 *     · **签名不符这一档应用内观测不到**（`ACTION_VIEW` 交出去之后没有回调），只能预告 ——
 *       逐条登记在 `app-update-install.ts` 的头注里，别把这一行读成「三档都测得出来」。
 *
 *    🔴 **「没有可下载的资产」是一档真实存在的结果，不是错误**：APK 作为 release 资产是
 *    2026-09-13 才开始发的（`.github/workflows/android.yml` 的 `release-apk` job，且它还等着
 *    仓外的签名 secret），在那之前的每一个 release 都没有 APK。后端那一档如实回空的资产字段，
 *    本页据此**不画**下载按钮，退回「打开发布页」——判据是 `appUpdateDownloadTarget`，
 *    不是「试着下一下看看」。
 *
 *    检查这条腿的会话态住在 `app-update-check.ts` —— 根页那行「有新版本」入口（W-20）与本页
 *    共用一份，否则进一次设置发两次 GitHub 请求，两处还可能显示不同的答案。
 *    下载腿的进度不走那份会话态：它由后端 `update:progress` 广播推动（**后台自动下载腿也发
 *    同一个事件**），接线复用桌面那份 `wireUpdateProgress`（先订阅、后回读快照，回读被事件
 *    一票否决）——移动端另写一份必然与它漂，而漂出来的形态是「同一次下载两处显示成两个样子」。
 *
 *    🔴 **同一条形状债咬到了应用版本那一格**（顺手订正）：`getInfo()` 的真实载荷是
 *    `{ appVersion, coreVersion, coreBaseline }`（`commands/updater/app_update.rs:428-436`），
 *    `appVersion` 两侧同名 ⇒ 那一格是真值。拿不到时显示 `—`（与 `AboutPage.tsx:155` 同一个占位），
 *    **不是**「待接线」：版本号读不到是一个**运行期**状态，不是一条没接的腿。此前那颗
 *    `mobileSettings.pending` 芯片把两件事说成了一件，那条键随之无消费点，已整条删掉。
 *
 * 未受影响的是 GitHub 代理与自定义域名（`SettingsUpdate.tsx:81-128`）：它们服务的是**规则资源下载**，
 * 不是内核，两个平台都照常保留。订阅与规则资源的自动更新档同理。
 */

import { useEffect, useState, useSyncExternalStore, type ReactElement } from 'react';
import type { TFunction } from 'i18next';
import { useTranslation } from 'react-i18next';
import { GH_PROXY_PRESETS } from '@/domain/gh-proxy';
import {
  appUpdateIncludePrerelease,
  type AppUpdateChannel,
} from '@/domain/app-update-channel';
import {
  systemApi,
  updateApi,
  versionApi,
  type UpdateProgressManifest,
  type VersionInfo,
} from '@/ipc/api-client';
import {
  appUpdateErrText,
  backgroundIntervalSelectValue,
  ruleResourceAutoUpdateChecked,
  wireUpdateProgress,
  type AppDownloadIntegrity,
  type ProgressDrivenState,
} from '@/components/screens/settings/settings-logic';
import {
  markAppVersionSkipped,
  skippedAppVersions,
  subscribeAppVersionSkipped,
} from '@/components/layout/app-update-banner';
import {
  MobileButton,
  MobileSelect,
  MobileSwitch,
  MobileTextInput,
  SettingsGroup,
  SettingsNote,
  SettingsRow,
} from './SettingsChrome';
import {
  appUpdateHint,
  checkAppUpdateViaIpc,
  readAppUpdateCheck,
  reinstallTarget,
  runAppUpdateCheck,
  subscribeAppUpdateCheck,
  type AppUpdateCheckState,
} from './app-update-check';
import { androidInstallFailureKey, classifyInstallHandoff } from './app-update-install';
import { failureText } from './write-feedback';
import type { MobileSettingsPageProps } from './settings-pages';

type SubProxyPolicy = 'follow' | 'proxy' | 'direct';

/**
 * 发布页。与 `AboutPage.tsx:34` 那条**同一个字面量** —— 两处各写一份的话，仓迁走时一处改了
 * 另一处不改，用户会被送到一个 404。这里不 import 那边：`AboutPage` 没有导出它，
 * 而为了共享一个常量把它提成模块导出属另一次重构；判据按字面量对拍两处相等。
 */
const RELEASES_URL = 'https://github.com/polaris-arch/Polaris/releases';

/*
 * ⚠️ 行 id 写字面量（不抽常量），与本屏其余六页同一条口径：⑩ 组那条写腿对差按
 * `commit('<id>'` 的**字面**形态扫，抽成常量会让这两条写腿整条离开取材面。
 */

/**
 * 下载腿的本地态。**不并进 `app-update-check.ts` 那份会话态**：那一份回答「有没有新版本」，
 * 由本页与设置根页共用；这一份回答「那份包下到哪一步了」，只有本页消费，且它的真值源是后端
 * 广播的 `update:progress`（后台自动下载腿也发同一个事件），不是本页发起的那次调用。
 */
interface DownloadState {
  readonly phase: ProgressDrivenState | 'idle';
  /** 这一帧描述的那份包（版本号从它取；后台腿下的包与本页上次查到的可能不是同一个）。 */
  readonly info: UpdateProgressManifest | null;
  /** 已落位的包路径；「安装」那颗按钮拿的就是它。 */
  readonly path: string | null;
  readonly percentage: number;
  /** 失败机器码；正文由 `appUpdateErrText` 按码取键（后端不产正文，U1）。 */
  readonly errorCode: string | null;
  /**
   * 摘要这一级校验的结论。**承重**：`verified` 特指「有期望 sha256 且逐字相符」，
   * 而旧 release 根本不给摘要（GitHub 的 `digest` 字段是后加的）—— 那一档后端如实回
   * `verified:false`，包仍然可装，但**不许把它说成「已校验」**。
   * 少了这一格，「下载完成」那句话在无摘要的 release 上就是一句假话。
   */
  readonly integrity: AppDownloadIntegrity;
}

const DOWNLOAD_IDLE: DownloadState = {
  phase: 'idle',
  info: null,
  path: null,
  percentage: 0,
  errorCode: null,
  integrity: 'unknown',
};

/**
 * 「重装当前版本」在这一档没有对象时抛的哨兵。
 *
 * 用一个**类型**而不是一句约定好的 message：message 会被 `failureText` 当成「原因」原样贴进
 * 那一行红字，于是一个内部标记串就成了用户可见文案（元规则 #5）。
 */
class ReinstallUnavailable extends Error {}

/** 下载腿这一行的**进度**说明；`undefined` = 这一档由检查腿那句话来说。失败不走这里，见下。 */
function downloadLine(t: TFunction, dl: DownloadState): string | undefined {
  switch (dl.phase) {
    case 'downloading':
      /* 0% 是「服务端没给 Content-Length、算不出分母」那一档的真值（后端刻意不拿已收字节
         瞎凑一个分母）。如实显示 0%，不改成「正在下载…」把两件事说成一件。 */
      return t('mobileSettings.update.downloadingPercent', { percent: dl.percentage });
    case 'downloaded':
      /* 🔴 **没有摘要就不许说「已校验」**：`unverified` = 这个 release 一条 sha256 都没给
         （旧 release 的正常形态，后端不因此拒装），此时只过了「清单声明体积」与
         Content-Length 两级弱校验。桌面同一档说的也是这句 `digestMissingAfter`，
         两端共用一条文案 —— 移动端另写一句必然与它漂。 */
      return dl.integrity === 'unverified'
        ? `${t('mobileSettings.update.downloadReady')} ${t('settings.update.digestMissingAfter')}`
        : t('mobileSettings.update.downloadReady');
    default:
      return undefined;
  }
}

/**
 * 「后端跑到了，并且它已经自己报过这次失败」。
 *
 * `update_download` 的**每一条**失败早退都先发一帧 `ProgressStage::Failed` 再返带码的失败信封
 * （Rust 侧 `every_failure_path_emits_an_error_progress_event` 按计数锁死这条不变式）。
 * 于是这一档的原因已经落在本行的红字上（下面那个 `problem`）—— 再从 `commit` 抛一次，
 * 用户会在同一行看见同一句话两遍。
 *
 * 反过来，**没有码**就意味着这次调用根本没走到命令体（命令未注册、序列化失败、传输腿坏了）：
 * 那一档一帧都不会来，只有 `commit` 的红字兜得住，必须原样抛上去。
 *
 * 判据是「信封带码」而不是 `instanceof IpcError`：`unwrap` 只在拆信封失败那一支造 `IpcError`，
 * 而它恰好就是带码的那一支 —— 两个判据今天等价，但前者直说了为什么。
 */
function backendAlreadyReported(err: unknown): boolean {
  return typeof (err as { code?: unknown } | null)?.code === 'string';
}

/** 会话级检查快照。根页那行入口读的是同一份（`app-update-check.ts`）。 */
export function useAppUpdateCheck(): AppUpdateCheckState {
  return useSyncExternalStore(subscribeAppUpdateCheck, readAppUpdateCheck, readAppUpdateCheck);
}

/**
 * 提示行取文。折算住在 `app-update-check.ts`（纯函数、有单测），这里只把它翻成人话 ——
 * `undefined` 而不是空串：`SettingsRow` 按 `!== undefined` 决定那一行画不画，空串会多出一个空行。
 */
function hintText(t: TFunction, state: AppUpdateCheckState): string | undefined {
  const hint = appUpdateHint(state);
  if (hint === null) return undefined;
  return hint.version === undefined ? t(hint.key) : t(hint.key, { version: hint.version });
}

export function UpdatePage({ config, update, commit }: MobileSettingsPageProps): ReactElement {
  const { t } = useTranslation();
  const [info, setInfo] = useState<VersionInfo | null>(null);
  const [ghCustom, setGhCustom] = useState(false);
  const check = useAppUpdateCheck();

  useEffect(() => {
    void versionApi
      .getInfo()
      .then(setInfo)
      .catch(() => undefined);
  }, []);

  /*
   * 会话级「已跳过的版本」。**必须订阅**（而不是渲染时读一次）：同一个集合也被设置根页那行
   * 「有新版本」入口读（`MobileSettingsScreen` 的 `subscribeAppVersionSkipped`），
   * 少订这一条会得到「这页上按了跳过、根页那行还在提示同一个版本」。
   *
   * 它与后端持久化的 `skippedVersion` 是两层：后端那层让**下一次**检查不再报这个版本，
   * 本层让**这一次**已经查到的结果立刻收声 —— 少了它，跳过之后按钮还挂在那儿。
   */
  const [skipped, setSkipped] = useState<ReadonlySet<string>>(() => skippedAppVersions());
  useEffect(() => subscribeAppVersionSkipped(() => setSkipped(skippedAppVersions())), []);

  /*
   * 下载腿：接后端 `update:progress`。**先订阅、后回读快照**，回读被「本次订阅已收到过事件」
   * 一票否决 —— 整条编排复用桌面那份 `wireUpdateProgress`（它跑得进单测，这里跑不进）。
   *
   * 为什么必须回读：切页 / 隐私锁遮罩 / 系统回收 WebView 之后本组件重挂载，而下载跑在后端，
   * 窗口没了照跑。只订阅的话，**下载已经下完**时那一帧永不再来 ⇒ 界面永远停在「下载」按钮上，
   * 而包已经在盘上了。后台自动下载腿（`autoDownloadUpdate` 打开时）下的那份同样只能靠回读看见。
   */
  const [dl, setDl] = useState<DownloadState>(DOWNLOAD_IDLE);
  useEffect(
    () =>
      wireUpdateProgress({
        subscribe: (onFrame) => updateApi.onProgress(onFrame),
        readSnapshot: () => updateApi.getProgress(),
        /* 新一轮下载开始 ⇒ 上一份包的校验结论作废（判据是 `progressResetsIntegrity`）。
           不清的话，「上次校验过、这次没有摘要」会让界面继续说「已校验」。 */
        resetIntegrity: () => setDl((prev) => ({ ...prev, integrity: 'unknown' })),
        applyPatch: (patch) =>
          setDl((prev) => ({
            phase: patch.us,
            info: patch.info,
            path: patch.path,
            percentage: patch.percentage,
            /* `null` = **本帧不表态**（不是「没有错误」/「没有结论」）：非失败帧取走 `errorCode`
               会把上一条失败原因擦掉，而那一行还在显示 `error` 态；`integrity` 同理，它只在
               落位帧上存在。判据同桌面 `updateCardPatch` 的头注。 */
            errorCode: patch.errorCode ?? prev.errorCode,
            integrity: patch.integrity ?? prev.integrity,
          })),
      }),
    [],
  );

  /*
   * 「包已交给系统安装器」。**不是终态**：装没装成发生在系统安装器里，本进程看不见
   * （`ACTION_VIEW` 交出去之后没有回调，逐条登记在 `app-update-install.ts` 的头注）。
   * 故这一行说的是「交出去了」+ 一句签名一致性的预告，不是「装好了」。
   */
  const [handedOff, setHandedOff] = useState(false);

  /* 查到的新版本号。`hasUpdate` 为真却拿不到版本号是后端契约破损：那一档不画「跳过此版本」
     （跳过一个叫不出名字的版本落到后端就是一条空串），但「打开发布页」照旧 —— 发布页不需要版本号。 */
  const foundVersion = check.snapshot?.hasUpdate === true ? check.snapshot.version : null;
  const versionSkipped = foundVersion !== null && skipped.has(foundVersion);

  /* 可下载目标。`null` 的两种成因（没更新 / 有更新但那个 release 没发 APK）对用户是同一句话
     ——「去发布页拿」，故这里不分。跳过之后一并收声：留着就等于用户刚说「这个版本不用管」、
     界面还在推着他下载它。 */
  const target = versionSkipped ? null : check.target;
  /* 已落位的包（本页下的，或后台自动下载腿下的）。安装那颗按钮的对象就是它。 */
  const staged = dl.phase === 'downloaded' ? dl.path : null;
  const busy = dl.phase === 'downloading';

  /**
   * 交付失败的取文：把 Kotlin 侧那五个 `REASON_*` 码各翻成一句话，**不把码贴进界面**。
   * 「重装当前版本」那条腿的「这一档没有对象」共用同一个取文出口（它也是这一行的失败）。
   */
  const handoffText = (err: unknown): string => {
    if (err instanceof ReinstallUnavailable) return t('settings.update.reinstallUnavailable');
    const reason = err instanceof Error ? err.message : '';
    return t(androidInstallFailureKey(reason));
  };

  /* 下载腿的失败原因：由 `error` 帧带来（后台自动下载腿的失败**只有**这条通道兜得住 ——
     那一档一次 `commit` 都没在飞）。用户亲手点的那次不在 `commit` 里重说一遍，见
     `backendAlreadyReported`。 */
  const downloadProblem =
    dl.phase === 'error' ? appUpdateErrText(dl.errorCode, null, t) : undefined;

  const interval = backgroundIntervalSelectValue(config);
  const ghProxy = ghCustom
    ? 'custom'
    : !config.ghProxyPrefix
      ? 'none'
      : (GH_PROXY_PRESETS as readonly string[]).includes(config.ghProxyPrefix)
        ? config.ghProxyPrefix
        : 'custom';

  return (
    <>
      <SettingsGroup header={t('mobileSettings.update.channelBlock')}>
        {/* 版本号是真值（`about-info`），拿不到就显示 `—` —— 不编一个「最新」，也不说成「待接线」
            （那是一条腿的状态，不是一个读不到的值；见头注那条 🔴）。
            `hint` 而不是 `problem`：检查结果不是这一行「出错了」，它就是这一行要报告的东西。 */}
        <SettingsRow
          id="app-update"
          label={t('mobileSettings.update.appTitle')}
          desc={t('mobileHelp.appUpdate')}
          descDetails={t('mobileSettings.update.appDesc')}
          /* 下载腿在跑的时候由它说话（进度 / 已就绪 / 已交给系统安装器），否则让检查腿说。
             两件事不并排显示：它们描述的是同一条链路的不同阶段，同时说会互相打架
             （「已是最新」＋「正在下载 40%」）。 */
          hint={
            handedOff
              ? t('mobileSettings.update.handedOffNote')
              : (downloadLine(t, dl) ?? hintText(t, check))
          }
          problem={downloadProblem}
          control={
            <div style={{ display: 'flex', gap: '8px', flexWrap: 'wrap', justifyContent: 'flex-end' }}>
              <span
                data-status="app-update"
                style={{
                  alignSelf: 'center',
                  fontSize: '0.75rem',
                  color: 'hsl(var(--fg-dim))',
                  border: '1px solid hsl(var(--line))',
                  borderRadius: 'var(--r-xs)',
                  padding: '4px 8px',
                }}
              >
                {info?.appVersion ? `v${info.appVersion}` : '—'}
              </span>
              <MobileButton
                disabled={check.phase === 'checking'}
                onClick={() => {
                  commit(
                    'app-update',
                    runAppUpdateCheck(checkAppUpdateViaIpc, config),
                    (err) =>
                      failureText(t, err, {
                        withReason: 'mobileSettings.update.checkFailed',
                        plain: 'mobileSettings.update.checkFailedPlain',
                      }),
                  );
                }}
              >
                {check.phase === 'checking'
                  ? t('mobileSettings.update.checking')
                  : t('mobileSettings.update.checkAction')}
              </MobileButton>
              {/* ── 下载 → 交系统安装器那一跳（批 15）──────────────────────────────
                  三颗按钮**互斥**，按下载腿的态出一颗：
                   · 有可下载目标且还没下 → 「下载」；
                   · 下完了（本页下的，或后台自动下载腿下的）→ 「安装」；
                   · 上一次下载失败 → 「重试」（桌面那颗 `common.retry` 重试的也是**下载**，
                     不是检查 —— 检查的重试是同一颗检查按钮再点一次）。
                  「没有可下载的资产」那一档一颗都不出：那时用户出口是「打开发布页」。 */}
              {staged !== null && (
                <MobileButton
                  tone="primary"
                  onClick={() => {
                    /* IPC 调用**内联在 `commit` 的实参里**（裁定 #14 的辖区判据按词法算）。
                       `awaitingSystemInstaller` 为真 = 包交出去了、系统安装器已在前台，而本进程
                       还活着 —— 它**不是**「装好了」。交不出去时后端把 Kotlin 的原因码原样带回来，
                       这里逐码翻成一句话（`androidInstallFailureKey`）。 */
                    commit(
                      'app-update',
                      updateApi.install(staged).then((result) => {
                        const outcome = classifyInstallHandoff(result);
                        if (outcome.kind === 'refused') throw new Error(outcome.reason);
                        setHandedOff(true);
                      }),
                      handoffText,
                    );
                  }}
                >
                  {t('mobileSettings.update.installAction')}
                </MobileButton>
              )}
              {staged === null && target !== null && (
                <MobileButton
                  tone={dl.phase === 'error' ? 'plain' : 'primary'}
                  disabled={busy}
                  onClick={() => {
                    setHandedOff(false);
                    commit(
                      'app-update',
                      updateApi.download(target).then(
                        () => undefined,
                        (err: unknown) => {
                          /* 后端已经自己报过这一次（`error` 帧 → 上面那行红字）⇒ 这里咽下，
                             否则同一句话在同一行出现两遍。没到后端的那一档必须抛。 */
                          if (backendAlreadyReported(err)) return;
                          throw err;
                        },
                      ),
                      handoffText,
                    );
                  }}
                >
                  {busy
                    ? t('mobileSettings.update.downloadingAction')
                    : dl.phase === 'error'
                      ? t('common.retry')
                      : t('settings.update.download')}
                </MobileButton>
              )}
              {/* 「打开发布页」只在**真的查到新版本**时出现。恒显一颗「去 GitHub」按钮等于把
                  「这里有新版本」这句话说给每一个人听，而多数时候它不成立。
                  按过「跳过此版本」之后一并收声：留着它就等于用户刚说「这个版本不用管」、
                  界面还在推着他去下载它。 */}
              {check.snapshot?.hasUpdate === true && !versionSkipped && (
                <MobileButton
                  tone="primary"
                  onClick={() => {
                    /* 与检查那条腿共用同一个行 id：两条腿贴的是**同一行**，红字也该落在那一行下面。
                       用一个不存在的行 id 会让那句话渲染不出来（`SettingsRow` 按 id 自取）。 */
                    commit('app-update', systemApi.openExternal(RELEASES_URL), (err) =>
                      failureText(t, err, {
                        withReason: 'mobileSettings.about.openLinkFailedWithReason',
                        plain: 'mobileSettings.about.openLinkFailed',
                      }),
                    );
                  }}
                >
                  {t('mobileSettings.update.openReleases')}
                </MobileButton>
              )}
              {/* 「跳过此版本」（桌面 `AppUpdateCard.tsx:144` 那一颗）。后端 `update_skip` 把版本号
                  持久化进 updater state，`update_check` 的第四道闸据此过滤 —— 两端同一条腿。
                  IPC 调用**内联在 `commit` 的实参里**：失败必须落成这一行下面的红字（裁定 #14），
                  而 `update_skip` 回的是 `{success}` 信封、不抛，故这里显式把 `false` 折成一次
                  reject —— 不然「跳过失败」会安静地长成「跳过成功」。 */}
              {foundVersion !== null && !versionSkipped && (
                <MobileButton
                  onClick={() => {
                    commit(
                      'app-update',
                      updateApi.skip(foundVersion).then((result) => {
                        /* 🔴 **不编一条 message**：`failureText` 会把 `err.message` 原样当成
                           「原因」贴进那一行的红字里，而后端这一档并没有给原因 —— 编一句英文
                           出来既是开发者信息外露（元规则 #5），也是在替后端造一个它没说过的事实。
                           空 message ⇒ 走 `plain` 那支，界面上是「跳过此版本失败。」。 */
                        if (!result.success) throw new Error();
                        markAppVersionSkipped(foundVersion);
                      }),
                      (err) =>
                        failureText(t, err, {
                          withReason: 'mobileSettings.update.skipFailed',
                          plain: 'mobileSettings.update.skipFailedPlain',
                        }),
                    );
                  }}
                >
                  {t('settings.update.bannerSkip')}
                </MobileButton>
              )}
            </div>
          }
        />
        {/*
          「重装当前版本」（桌面 `AppUpdateCard.tsx:74` 那一颗 + 它的 `data-tip`）。
          它修的是**损坏的安装**，不是升级：走 `update_check(includeCurrent:true)` → 选**当前版本**
          的资产 → 下载 → 交系统安装器。三条早退（真有新版 / 通道最新版与已装版本对不上 /
          那个 release 没有可下载的资产）都折成同一句 `settings.update.reinstallUnavailable`
          —— 判据是纯函数 `reinstallTarget`，见它的文档。

          🔴 **下载那一步不在这里重写**：它把目标交给同一个 `updateApi.download`，于是完整性校验、
          单飞、进度事件、复用本地已有包全部与「下载新版本」那条腿逐字相同。两套下载编排必然漂。
        */}
        <SettingsRow
          stacked
          id="app-reinstall"
          label={t('settings.update.reinstallCurrent')}
          desc={t('settings.update.reinstallCurrentTip')}
          control={
            <MobileButton
              disabled={busy}
              onClick={() => {
                commit(
                  'app-reinstall',
                  checkAppUpdateViaIpc({
                    /* 通道**显式重铺**，不转手一个变量：`settings-logic.test.ts` 那道
                       「全仓前端检查入口必须显式携带持久化通道」的门按调用点的实参文本扫。 */
                    includePrerelease: appUpdateIncludePrerelease(config),
                    includeCurrent: true,
                  }).then((result) => {
                    if (result.error) throw new Error(result.error);
                    const current = reinstallTarget(result);
                    if (current === null) throw new ReinstallUnavailable();
                    return updateApi.download(current).then(
                      () => undefined,
                      (err: unknown) => {
                        if (backendAlreadyReported(err)) return;
                        throw err;
                      },
                    );
                  }),
                  handoffText,
                );
              }}
            >
              {t('settings.update.reinstallCurrent')}
            </MobileButton>
          }
        />
        {/*
          「自动下载新版本」。它约束的正是上面那条下载腿：启动时自动检查查到新版本后，
          后台把包下好但**绝不自动装**（后端 `startup_tasks::spawn_auto_download` 的头注写着
          为什么不顺手装：安装要停代理并退出应用，那三件事不该在用户没点确认时发生）。
          下好之后本页会在重挂载时经 `update:progress` 的快照回读看见它，出一颗「安装」。

          缺省**关**（与「自动检查更新」缺省开相反）：检查是几 KB 的 JSON，下载是几十 MB 的包，
          可能跑在计费网络上 —— 用户没说要就不该替他花流量。判据与桌面同一条
          （`should_auto_download_update` 认 `=== true`，缺省即关）。
        */}
        <SettingsRow
          descDetails={config.autoDownloadUpdate ? t('settings.update.autoDownloadAppDesc') : undefined}
          id="auto-download-update"
          label={t('settings.update.autoDownloadApp')}
          desc={
            config.autoDownloadUpdate
              ? t('mobileHelp.autoDownload')
              : t('settings.update.autoDownloadAppDescOff')
          }
          control={
            <MobileSwitch
              checked={!!config.autoDownloadUpdate}
              ariaLabel={t('settings.update.autoDownloadApp')}
              onChange={(v) => commit('auto-download-update', update({ autoDownloadUpdate: v }))}
            />
          }
        />
        {/*
          应用更新通道（稳定 / 测试）。**这一格此前是「配置里在、界面上看不见」的原样形态**：
          `appUpdateIncludePrerelease(config)` 早就被本页那颗检查按钮读着（经
          `runAppUpdateCheck`），桌面也能改，手机上却没有任何控件能改它 —— 于是一台手机上
          「检查更新」查的是哪条通道，取决于用户上次在桌面端点了什么，而这台设备上看不出来。

        */}
        <SettingsRow
          stacked
          id="update-channel"
          label={t('settings.update.appChannel')}
          desc={t('mobileHelp.appChannel')}
          descDetails={t('settings.update.appChannelDesc')}
          control={
            <MobileSelect
              value={config.appUpdateChannel ?? 'stable'}
              ariaLabel={t('settings.update.appChannel')}
              onChange={(v) =>
                commit('update-channel', update({ appUpdateChannel: v as AppUpdateChannel }))
              }
            >
              <option value="stable">{t('settings.update.appChannelStable')}</option>
              <option value="prerelease">{t('settings.update.appChannelPrerelease')}</option>
            </MobileSelect>
          }
        />
      </SettingsGroup>

      <SettingsGroup header={t('settings.update.subBlock')}>
        <SettingsRow
          first
          stacked
          id="update-interval"
          label={t('settings.update.intervalCard')}
          desc={t('settings.update.intervalCardSub')}
          control={
            <MobileSelect
              value={interval}
              ariaLabel={t('settings.update.intervalCard')}
              onChange={(v) =>
                commit(
                  'update-interval',
                  update({
                    subscriptionUpdateIntervalHours: Number(v),
                    ruleResourceUpdateIntervalHours: Number(v),
                  }),
                )
              }
            >
              <option value="0">{t('settings.update.intervalManualOnly')}</option>
              <option value="6">{t('settings.update.intervalHours', { n: 6 })}</option>
              <option value="12">{t('settings.update.intervalHours', { n: 12 })}</option>
              <option value="24">{t('settings.update.intervalHours', { n: 24 })}</option>
              <option value="72">{t('settings.update.intervalDays', { n: 3 })}</option>
              <option value="168">{t('settings.update.intervalDays', { n: 7 })}</option>
            </MobileSelect>
          }
        />
        <SettingsRow
          id="sub-auto-on-start"
          label={t('settings.update.subAutoOnStart')}
          desc={t('settings.update.subPerSubNote')}
          control={
            <MobileSwitch
              checked={!!config.autoUpdateSubscriptionOnStart}
              ariaLabel={t('settings.update.subAutoOnStart')}
              onChange={(v) => commit('sub-auto-on-start', update({ autoUpdateSubscriptionOnStart: v }))}
            />
          }
        />
        <SettingsRow
          stacked
          id="sub-channel"
          label={t('settings.update.subChannel')}
          desc={t('settings.update.subChannelDesc')}
          control={
            <MobileSelect
              value={config.subscriptionProxyPolicy ?? 'follow'}
              ariaLabel={t('settings.update.subChannel')}
              onChange={(v) => commit('sub-channel', update({ subscriptionProxyPolicy: v as SubProxyPolicy }))}
            >
              <option value="follow">{t('settings.update.subFollow')}</option>
              <option value="proxy">{t('settings.update.subViaProxy')}</option>
              <option value="direct">{t('settings.update.subDirect')}</option>
            </MobileSelect>
          }
        />
        <SettingsRow
          id="rule-resource-auto"
          label={t('settings.update.ruleResourceAutoCard')}
          desc={t('settings.update.ruleResourceAutoDesc')}
          control={
            <MobileSwitch
              checked={ruleResourceAutoUpdateChecked(config)}
              ariaLabel={t('settings.update.ruleResourceAutoCard')}
              onChange={(v) =>
                commit(
                  'rule-resource-auto',
                  update({
                    ruleResourceAutoUpdate: v,
                    ruleResourceUpdateIntervalHours: config.subscriptionUpdateIntervalHours,
                  }),
                )
              }
            />
          }
        />
      </SettingsGroup>

      <SettingsGroup header={t('resources.ghProxy')}>
        <SettingsRow
          first
          stacked
          id="gh-proxy"
          label={t('resources.ghProxy')}
          /* 刻意**不**渲染桌面那句 `ghAccelSub`（「作用于应用、内核与规则资源的 GitHub 下载」）：
             移动端不经它下载应用与内核，照搬会与下面那条常驻说明当场打架。 */
          desc={t('mobileHelp.githubDownload')}
          descDetails={t('settings.update.ghNote')}
          control={
            <MobileSelect
              value={ghProxy}
              ariaLabel={t('resources.ghProxy')}
              onChange={(v) => {
                if (v === 'none') {
                  setGhCustom(false);
                  commit('gh-proxy', update({ ghProxyPrefix: '' }));
                } else if (v === 'custom') {
                  setGhCustom(true);
                } else {
                  setGhCustom(false);
                  commit('gh-proxy', update({ ghProxyPrefix: v }));
                }
              }}
            >
              <option value="none">{t('resources.direct')}</option>
              {GH_PROXY_PRESETS.map((preset) => (
                <option key={preset} value={preset}>
                  {t('settings.update.ghMirrorOption', { url: preset })}
                </option>
              ))}
              <option value="custom">{t('common.customEllipsis')}</option>
            </MobileSelect>
          }
        />
        {ghProxy === 'custom' && (
          <SettingsRow
            stacked
            id="gh-custom-domain"
            label={t('resources.customDomainLabel')}
            control={
              <MobileTextInput
                mono
                value={config.ghProxyPrefix ?? ''}
                ariaLabel={t('resources.customDomainLabel')}
                placeholder="https://cdn.example/ · cdn.example"
                onChange={(v) => commit('gh-custom-domain', update({ ghProxyPrefix: v }))}
                onCommit={() => undefined}
              />
            }
          />
        )}
        <SettingsNote id="gh-proxy-scope">{t('mobileSettings.update.ghScopeNote')}</SettingsNote>
      </SettingsGroup>
    </>
  );
}
