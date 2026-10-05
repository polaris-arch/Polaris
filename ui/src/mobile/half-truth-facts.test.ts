/**
 * 「**半真**文案」的事实门 —— 与 `wiring-completeness.test.ts` 成对，补它结构性守不住的那一格。
 *
 * # 半真是什么，为什么它比纯缺席更坏
 *
 * 「文案说这个平台不支持 X，而代码里 X 其实已经能用（至少能用一半）」。
 * 它比「什么都没写」更坏，因为：
 *  ① 用户照着它做决定会做错 —— 被告知「去桌面端做」，于是他真的去了，而这边本来就能做；
 *  ② 它把一个**已经存在的能力**藏起来，最后没人知道那条腿是通的，下一批还会再实现一遍。
 *
 * # 为什么主门抓不到它（这才是本文件存在的理由）
 *
 * `wiring-completeness.test.ts` 的三个面全是**词法/登记**面：它数的是「还写着未接线的地方」。
 * 半真文案里往往一个「未接线」字样都没有（`This platform does not route by application` 说的是
 * 「不支持」，不是「没接」），而且——更要命的——**把文案删掉，主门就变绿了**，能力有没有接上
 * 它一无所知。主门自己的头注把这条写成了第 1 号射程自曝。
 *
 * ⇒ 本门反着来：**不断言文案，断言事实。** 每一组都断言「那条能力今天确实在」，
 *   于是当有人把能力删掉、或把文案改回「不支持」而能力还在时，说话的是这道门。
 *   它的红有两种读法，两种都对：要么能力真被拿掉了（那是回归），要么文案在撒谎（那是半真）。
 *
 * # 三组事实（每组都带**谓词自检**：合成一段反例，证明谓词分得清「在」与「不在」）
 *
 *  A. **导出腿**（日志 / 诊断 / 备份）：`tauri-plugin-dialog` 在 `lib.rs` 上是**无平台门控**注册的；
 *     `logs.rs` 的导出腿没有平台门控；`backup.rs` 仅在**导入选择器的扩展过滤**上用
 *     `cfg(not(android))`，让 Android SAF 能看见 .polaris-backup，导出保存腿不受它影响；
 *     五条命令都在 `generate_handler!` 里、那一整块也没有平台 `#[cfg]`；
 *     两个模块都**真的处理了 content URI**（不再有 `into_path().ok()` 那个吞掉 SAF 目标的形态）。
 *     ⚠️ 插件的 Android 实现本身（`tauri-plugin-dialog` 的 `DialogPlugin.kt` 里那个
 *       `ACTION_CREATE_DOCUMENT`）**不在本门射程内** —— 它住在 crate registry，不在本仓，
 *       本门够不着。本组证明的是「本仓这一侧没有把这条腿从移动端门控掉」，不是「插件真的实现了」。
 *     ⇒ **2026-09-06（W-18）起本组不再是「半真」那一类，而是保鲜断言**：那两句文案原来说
 *       「这条腿在移动端尚未接线」，当时只对**前端**成立（后端注册是通的、但 Rust 侧
 *       `into_path().ok()` 会把 SAF 的 `content://` 吃成「用户取消」）。现在前后端两侧都接上了，
 *       备份页文案改成了实话（`mobileSettings.backup.pickerNote` 说明系统选择器；
 *       日志页的导出动作由文件保存器接管）。本组因此转为盯着
 *       **能力别被拿掉**：谁把 dialog 插件门控掉、把命令从注册表摘掉、或把 `into_path().ok()`
 *       写回来，本组当场红。
 *  B. **Android 按应用分流**：`android_exclude_packages()` 把 `Direct` 档的包名写进 tun 的
 *     `exclude_package`，内置预设里有一批真的带 `package_names`。
 *     ⚠️ **这条的用途 2026-09-06 变了，如实记在这里**：立项时它抓的是文案里那句
 *     「本平台不按应用分流」——**只对一半**。那句话已经被改掉（现在的文案说的是「认不出应用本身，
 *     只认流量特征；带包名的应用由系统 VPN 整个排除在隧道外」），本组随之从「抓半真」转成
 *     **保鲜断言**：`exclude_package` 这条腿哪天被摘掉、或内置预设的 `package_names` 被清空，
 *     本组转红，而那正是该回头重判这句文案的时刻。
 *  C. **toast 宿主**（2026-09-06 翻面：宿主接上了，本组随之从「保鲜断言」改成「正面事实 + 归零棘轮」）：
 *     立项时全仓唯一的 `setToastImpl` 注入点是 `components/layout/Toaster.tsx`，它只挂在桌面外壳上
 *     ⇒ `lib/error-handler.ts` 的门面在移动端**落 console**，`RulesScreen.tsx` 那 24 处
 *     `toast.*` 一条都到不了用户眼前（「写了反馈但它是静音的」，比没写更难被发现）。
 *     W-25 补上了移动端自己的宿主（`mobile/MobileToaster.tsx`，由 `MobileApp` 挂在停靠区），
 *     于是本组现在断言三件事：
 *       ① 注入点恰好两个，且第二个是**移动端自己的**（不是把桌面 Toaster 引了进来 —— 那会违约契约 A1）；
 *       ② 移动树里没有任何人装载桌面 `Toaster`（A1 那条边界一格没松）；
 *       ③ **静音账本归零**，且这条棘轮**有牙**：两条反向对照分别把「注入点」与「谁调了注入函数」
 *         从取材面里拿掉，账本都必须弹回 `SILENT_TOAST_WITHOUT_HOST`。
 *     「注入了」是**两半**（写了一段非空注入 ＋ 生产装载路径上真的调了它），缺一不算 ——
 *     2026-09-06 复审实测：只判前半时，把宿主组件里那句 `installMobileToastHost(push)` 换掉，
 *     全门照绿而门面已退回 console。
 *     这样它仍然是双向的：把宿主拆掉或让它不再注入（回归）→ ③ 红；把桌面 Toaster 引进移动树 → ② 红。
 *
 * # 射程自曝
 *
 *  · 本门是**源码级**判据：它证明「这条腿在代码里存在且没有平台门控」，**不证明它在真机上跑通**。
 *    Android 上 `ACTION_CREATE_DOCUMENT` 真弹出来、真写进用户选的 URI —— 那归真机验收。
 *  · 它不检查文案。文案怎么改（接上 / 改成说实话）是产品决定，本门只保证那个决定有事实可依。
 *  · A 组的门控判据是**词法**的：它看 `#[cfg(...)]` 属性行，不做 Rust 语义分析。
 *    用 `cfg!()` 宏在**运行期**分流的写法它看不见（本仓这几处都不是那种写法）。
 */
import { describe, expect, it } from 'vitest';
import { existsSync, readdirSync, readFileSync, statSync } from 'node:fs';
import { join, relative } from 'node:path';
import { fileURLToPath } from 'node:url';

import { IS_TEST_ONLY_MODULE } from '@/contracts/test-only-modules';
import {
  CALL_SITE_FIXTURE,
  callSitesIn,
  isCallSiteLine,
  stripComments as stripCommentsKeepingLines,
} from '@/test/call-sites.test-support';
import { ROW_ACTIONS } from './nodes/absence-register';

const REPO_DIR = fileURLToPath(new URL('../../../', import.meta.url));
const UI_SRC_DIR = fileURLToPath(new URL('..', import.meta.url));
const MOBILE_DIR = fileURLToPath(new URL('.', import.meta.url));

function readRepoFile(relativePath: string): string {
  const full = join(REPO_DIR, relativePath);
  if (!existsSync(full)) throw new Error(`取材面缺失：${relativePath}（路径变了就该红，不该静默跳过）`);
  return readFileSync(full, 'utf8');
}

/** 去掉块注释与行注释 —— 注释里提到一个符号不等于调用它（本仓正是这么被喂绿过）。 */
function stripComments(source: string): string {
  return source
    .replace(/\/\*[\s\S]*?\*\//g, '')
    .split('\n')
    .map((line) => line.replace(/(^|[^:])\/\/.*$/, '$1'))
    .join('\n');
}

function walkTypeScript(dir: string, out: string[] = []): string[] {
  for (const entry of readdirSync(dir).sort()) {
    const full = join(dir, entry);
    if (statSync(full).isDirectory()) walkTypeScript(full, out);
    else if (/\.tsx?$/.test(full) && !IS_TEST_ONLY_MODULE.test(full)) out.push(full);
  }
  return out;
}

/* ══════════════ A 组：导出腿（dialog 插件 + 两个命令模块） ══════════════ */

const LIB_RS = 'src-tauri/src/lib.rs';
const LOGS_RS = 'src-tauri/src/commands/misc/logs.rs';
const BACKUP_RS = 'src-tauri/src/commands/misc/backup.rs';

type Gating = 'gated' | 'ungated' | 'absent';

/**
 * 某个插件注册**是否**被 `#[cfg(...)]` 门控。
 *
 * 判法：找到 `needle` 所在行，向上走到最近的 `let builder` 那一行（Tauri builder 链的起点），
 * 再向上跳过空行与注释行，看紧邻的那一行是不是 `#[cfg(`。
 * 词法判据，射程见文件头注最后一条。
 */
function pluginGating(source: string, needle: string): Gating {
  const lines = source.split('\n');
  const hit = lines.findIndex((line) => line.includes(needle));
  if (hit < 0) return 'absent';
  let chainStart = hit;
  while (chainStart >= 0 && !/^\s*let\s+builder\b/.test(lines[chainStart] ?? '')) chainStart -= 1;
  if (chainStart < 0) return 'absent';
  let above = chainStart - 1;
  while (above >= 0 && /^\s*(?:\/\/.*)?$/.test(lines[above] ?? '')) above -= 1;
  return /^\s*#\[cfg\(/.test(lines[above] ?? '') ? 'gated' : 'ungated';
}

/**
 * 源码里所有**平台**门控属性（`#[cfg(desktop)]` / `#[cfg(target_os = "…")]` 之类）。
 *
 * 🔴 不是「所有 `#[cfg(`」：本仓这两个命令模块各自带着 `#[cfg(test)]` 的测试 mod，
 * 那与「这条腿在不在移动端」毫无关系。把它一并算进去，判据会因为一件无关的事恒红，
 * 而恒红的门与没有门等价 —— 下一个人只会把它删掉。
 */
function platformGates(source: string): string[] {
  return (source.match(/#\[cfg\([^\]]*\)\]/g) ?? []).filter((attribute) =>
    /desktop|mobile|target_os|target_family|android|ios|windows|macos|linux|unix/.test(attribute),
  );
}

/**
 * `invoke_handler(tauri::generate_handler![...])` 那一整块（用于断言里面没有任何 `#[cfg]`）。
 *
 * 🔴 锚必须带 `.invoke_handler(` 前缀：`generate_handler!` 这个词在本文件里**第一次出现是在一段
 * 文档注释里**（`lib.rs:550`），只搜宏名会切出一大段与注册表无关的源码，
 * 里面正好含着 `#[cfg(desktop)]` 的插件注册 —— 判据会被一段注释带到沟里。
 */
function invokeHandlerBlock(source: string): string {
  const marker = '.invoke_handler(tauri::generate_handler![';
  const start = source.indexOf(marker);
  if (start < 0) throw new Error('lib.rs 里找不到 invoke_handler 注册块 —— 注册口径变了');
  const end = source.indexOf('])', start);
  if (end < 0) throw new Error('invoke_handler 注册块的结尾找不到 —— 解析口径变了');
  return source.slice(start, end);
}

/* ══════════════ B 组：Android 按应用分流 ══════════════ */

const INBOUNDS_RS = 'crates/config-engine/src/builder/inbounds.rs';
const PRESET_DATA_RS = 'crates/config-engine/src/user_config/app_rules_preset_data.rs';
const GOLDEN_ANDROID_RS = 'crates/config-engine/tests/golden_inbounds_android.rs';

/**
 * 「Direct 档的包名真的写进了 tun 的 `exclude_package`」这条链在源码里是否完整。
 *
 * 🔴 `if deps.platform == "android"` 在本文件里出现**三次**（296 / 575 / 673 行），
 * 只取第一处会量错块。故遍历**每一个**这样的块，任一块同时含「调用」与「写回」即为接通。
 */
function androidAppRoutingWired(source: string): boolean {
  if (!/fn\s+android_exclude_packages\s*\(/.test(source)) return false;
  for (const block of source.matchAll(/if\s+deps\.platform\s*==\s*"android"\s*\{[\s\S]*?\n\s{4}\}/g)) {
    if (
      block[0].includes('android_exclude_packages(config)') &&
      /tun\.exclude_package\s*=\s*Some\(/.test(block[0])
    ) {
      return true;
    }
  }
  return false;
}

/* ══════════════ C 组：toast 宿主 ══════════════ */

const TOAST_IMPL_DEFINITION = join(UI_SRC_DIR, 'lib', 'error-handler.ts');

/**
 * 真的**装了实现**的 `setToastImpl(` 调用 —— 不是「文件里出现过这个名字」。
 *
 * 🔴 判据必须认「非空注入」（2026-09-06 变异实测补强）：两个宿主的卸载腿都写着
 * `return () => setToastImpl({})`（恢复 console 兜底）。只匹配 `setToastImpl(` 时，
 * **把四条通道的映射整段删掉**这个变异照样绿 —— 那个文件仍然「含有 setToastImpl(」，
 * 而门面其实已经退回 console。故这里要求括号里那个对象字面量至少带一条通道键。
 */
const NON_EMPTY_INJECTION = /\bsetToastImpl\s*\(\s*\{\s*[\s\S]{0,120}?\b(?:success|error|info|warning)\s*:/;

/** 全仓（除门面自己）真的装了实现的文件，路径相对 `ui/src`。 */
function toastInjectionSites(): string[] {
  return walkTypeScript(UI_SRC_DIR)
    .filter((file) => file !== TOAST_IMPL_DEFINITION)
    .filter((file) => NON_EMPTY_INJECTION.test(stripComments(readFileSync(file, 'utf8'))))
    .map((file) => relative(UI_SRC_DIR, file))
    .sort();
}

/** 移动端生产源码里每个文件的 `toast.*` 调用数（注释里的不算）。 */
function mobileToastCalls(): Record<string, number> {
  const perFile: Record<string, number> = {};
  for (const file of walkTypeScript(MOBILE_DIR)) {
    const hits =
      stripComments(readFileSync(file, 'utf8')).match(
        /\btoast\.(?:success|info|warning|error)\s*\(/g,
      ) ?? [];
    if (hits.length > 0) perFile[relative(UI_SRC_DIR, file)] = hits.length;
  }
  return perFile;
}

/**
 * 宿主组件**真的调了**那个注入函数 —— 移动树里 `installMobileToastHost(` 的调用点（声明行不算）。
 *
 * 🔴 这一跳是 2026-09-06 复审的 major：上一版 `hosted` 只问「`mobile/**` 里哪个文件含一段非空
 * `setToastImpl({success:…})`」。那段字面量住在 `installMobileToastHost` 的函数体里，而**组件有没有
 * 真的调它**账本一个字都不问。实测把 `MobileToaster.tsx` 里那句
 * `const restore = installMobileToastHost(push);` 换成一个什么都不做的常量函数：`tsc` rc=0、
 * 全量 vitest 全绿、本组 `total === 0` 与反向对照 `=== 25` 双双照绿，而生产上那 24 处 `toast.*`
 * 全部退回 `console` —— 正是这条棘轮存在的全部理由。
 *
 * 判据用共用谓词（`@/test/call-sites.test-support`，声明行不算，自检见下面 C 组第一条）：
 * 定义与调用同在一个文件里，按文件文本匹配的判据对「组件不再调它」完全不可见。
 */
function toastHostInstallSites(): string[] {
  const sources = new Map(
    walkTypeScript(MOBILE_DIR).map((file) => [
      relative(UI_SRC_DIR, file),
      stripCommentsKeepingLines(readFileSync(file, 'utf8')),
    ]),
  );
  return callSitesIn(sources, 'installMobileToastHost');
}

/**
 * 移动端生产源码里**落 console 的** toast 调用数。
 *
 * 定义（而不是「调用数」本身）：门面未被移动端注入时 = 全部调用都是静音的；注入了 = 0。
 * `sites` 与 `installSites` 都从外面传进来，正是为了让 ③ 那两条反向对照能各喂一个「宿主不在」
 * 的取材面进来，证明这条棘轮真的会说话 —— 不传参的话它会退化成一句恒等于 0 的空话。
 *
 * 「注入了」是**两半**，缺一不算：① 有人写了一段非空注入（`sites`）；② 生产装载路径上真的
 * 调了它（`installSites`）。少了②就是上面那条 major 的形态：组件挂着、但它不再注入。
 */
function silentToastCalls(
  sites: readonly string[],
  installSites: readonly string[],
): { total: number; perFile: Record<string, number> } {
  const perFile = mobileToastCalls();
  const hosted = sites.some((file) => file.startsWith('mobile/')) && installSites.length > 0;
  const total = hosted ? 0 : Object.values(perFile).reduce((sum, n) => sum + n, 0);
  return { total, perFile };
}

/**
 * 静音 toast 调用的**精确棘轮**，只许降不许升。
 *
 * 变更史：24（2026-09-06 立项实测，全部在 `mobile/screens/rules/RulesScreen.tsx`，宿主缺席）
 *       → 0（同日 W-25：`mobile/MobileToaster.tsx` 注入门面，那 24 处当场从静音变可见）。
 *
 * 🔴 归零**不是**靠删调用换来的：下面 ③ 的反向对照断言「把宿主从取材面里拿掉，这个数必须弹回
 * `SILENT_TOAST_WITHOUT_HOST`」。删掉一处 `toast.*` 会让那条对照红，而不是让本条更绿。
 */
const SILENT_TOAST_BASELINE = 0;

/**
 * 宿主不在时该有的数（反向对照的期望值；同时是「调用还都在」的正面账）。
 *
 * 变更史：24（立项实测，全在 `screens/rules/RulesScreen.tsx`）
 *       → 25（2026-09-06 W-25 同批：`mobile/app-wiring.ts` 的 `reportIfFails` 多了一处
 *         `toast.warning` —— 事件驱动的副作用没有可归因的控件，失败只能落全局 toast）
 *       → 28（同日复审 major 补接两条桌面订阅：`onMeshLoginFallback` 的「已临时直连」/「已切回」
 *         两条，与 `onAutoNodeSwitched` 的「已自动切换到 X」一条。三条都是**后端自驱**改变了
 *         流量走向而界面上什么都不显 —— 没有可归因的控件，只能落全局 toast）
 *       → 30（同日批 2 复审 major：`forms/ImportFormPanel.tsx` 的导入成功回执从**面板自持的
 *         `notice`** 换成全局 toast 两处（`toast.info` 撞单例跳过 / `toast.success` 已导入·已暂存）。
 *         换通道的理由是那条回执**画不出来**：`setNotice` 与下一行的 `closeInstance` 在同一次提交里，
 *         面板当帧卸载 ⇒ 「导入 18 项、跳过 2 项」一帧都没渲染过。桌面 `ImportDialog.tsx:299-316`
 *         走的就是 toast，且那段注释记着它当初修的正是「导入完全静默」这个形态）。
 *       → 31（2026-09-25 ζ 批 A8：`forms/SubFormPanel.tsx#SubCreateTaskPanel` 恢复面的成功回执
 *         `toast.success`（已添加 / 部分添加）。与导入那两处同一个理由：它紧跟在 `closeInstance` 之后，
 *         面板当帧卸载，面板内 notice 一帧都画不出来；而且它的典型时刻是「进程被回收后冷启动找回来
 *         的那次创建刚好跑完」，用户未必正盯着这张面板。只播一次由 `markTerminalHandled` 兜着）。
 *       → 51（2026-09-26：资源更新收据补齐后的字面 toast 调用账本。资源动作仍由
 *         MobileRulesScreen 发出，MobileToaster 的真实注入宿主承接；回包的成功/取消/失败
 *         动态 tone 也通过同一 toast 门面，无第二套静音通道）。
 *       → 53（上游 TS 主核授权链接自动打开失败新增一处全局提示；
 *         TS 面板卸载后的原生取消失败只能由全局宿主告知用户，面板内 notice 已不存在。
 *         无效 URL 的全局取消改走现有 reportIfFails，不新增第二套宿主）。
 *       → 74（本轮移动表单反馈新增 17 处，节点删除独立反馈新增 3 处，备份页新增 2 处，
 *         节点列表整合后少 1 处；53 + 17 + 3 + 2 - 1 = 74。表单、节点与备份页都调用同一
 *         `toast` 门面，移动宿主仍由 `MobileToaster` 装载并在 effect 中注入；下方两条反向
 *         对照分别移除注入调用和移动宿主，确保这些新反馈不会被误算成可见）。
 *       → 75（2026-10-06：Home 手动网络 observer 精确取消失败新增一处全局 error。
 *         Home 卸载后仍由既有 MobileToaster 宿主显示；picker-speed.browser.test.ts 的
 *         cancel reject + 离开 Home 负门使用真实宿主证明可见，不将取消记为只读豁免）。
 *
 * 🔴 抬这个数的门槛：**必须确认新增的那几处调用真的能到用户眼前**。它今天成立的依据是四条：
 * 宿主已挂（上面第一条断言）＋ 宿主真的调了注入函数（上面第二条）＋ 装配面已证
 * （`mobile-chrome.test.tsx` ④）＋ 注入发生在 effect 里（`app-wiring.test.tsx` ⑪）。
 * 那三处订阅腿还各有一条行为判据（`app-wiring.test.tsx` ⑥.2）；新加的这两处由
 * `forms/form-host.test.ts` 的「成功回执不许落在会被卸载的通道上」那一条守着。
 * 另有一条**层叠**前提：上面那句「哪天有人让某条 toast 在表单还开着时发出，先解决那个层叠
 * 问题再抬数」，在 2026-09-06 批 3 当场被兑现了 —— WARP 注册腿的单例闸（`registerWarpIfSlotFree`）
 * 就是在表单还开着时发 toast 的。层叠问题已修：`.m-toast-host` 拿到 `z-index:50`
 * （`mobile.css`），高于移动端全部弹层（最高 40），判据是 `mobile-chrome.test.tsx` ⑥
 * ——它按整个移动端层叠上下文取材，谁新加一个更高的层就当场红。
 * 🔴 抬这个数之前仍要先过上面那几条，否则就是又写了几处静音反馈还把账做平了。
 */
const SILENT_TOAST_WITHOUT_HOST = 75;

/* ══════════════════════════ 断言 ══════════════════════════ */

describe('半真文案的事实门', () => {
  describe('A 组｜导出腿：dialog 插件在 Android 上没有被门控掉', () => {
    const libRs = readRepoFile(LIB_RS);

    it('谓词自检：合成的正反例都要判对（否则下面几条一个字都不算数）', () => {
      const gatedSample = [
        '    #[cfg(desktop)]',
        '    let builder = builder.plugin(tauri_plugin_x::init());',
      ].join('\n');
      const ungatedSample = [
        '    let builder = builder',
        '        .plugin(tauri_plugin_x::init());',
      ].join('\n');
      expect(pluginGating(gatedSample, 'tauri_plugin_x::init')).toBe('gated');
      expect(pluginGating(ungatedSample, 'tauri_plugin_x::init')).toBe('ungated');
      expect(pluginGating(ungatedSample, 'tauri_plugin_nope::init')).toBe('absent');
      // 平台门控的识别面同样要正反各证一次：认得出平台 cfg，且**不**把 `#[cfg(test)]` 当平台门控
      // ——后者若被误判，这两个命令模块会因为各自带的测试 mod 而恒红。
      expect(platformGates('#[cfg(desktop)]\n#[cfg(target_os = "android")]')).toHaveLength(2);
      expect(platformGates('#[cfg(test)]\n#[cfg(feature = "x")]')).toEqual([]);
    });

    it('正面对照：本仓那三个**确有**平台门控的注册，谓词必须报 gated', () => {
      // 这一条是「全绿也需反向对照」：不先证明谓词认得出门控，下面那句
      // 「dialog 没有门控」就可能只是因为谓词对谁都报 ungated。
      expect(pluginGating(libRs, 'tauri_plugin_single_instance::init'), '单实例锁').toBe('gated');
      expect(pluginGating(libRs, 'tauri_plugin_autostart::init'), '开机自启').toBe('gated');
      expect(pluginGating(libRs, 'runtime::proxy::android_bridge::init'), 'Android 起停核桥').toBe(
        'gated',
      );
    });

    it('事实：tauri_plugin_dialog 是**无平台门控**注册的 ⇒ Android 上也装载', () => {
      expect(
        pluginGating(libRs, 'tauri_plugin_dialog::init'),
        'dialog 插件被加上了平台门控 —— 要么导出腿真的没了（回归），' +
          '要么该同批把「移动端没有文件对话框」这件事重新判一次',
      ).toBe('ungated');
    });

    it('导出腿保留文件保存；Android 备份导入只跳过扩展过滤', () => {
      for (const [path, expectedSaveCalls] of [
        [LOGS_RS, 3],
        [BACKUP_RS, 1],
      ] as const) {
        const source = readRepoFile(path);
        expect(
          platformGates(source),
          `${path} 出现了未登记的平台分支`,
        ).toEqual(path === LOGS_RS ? [
          '#[cfg(all(target_os = "android", debug_assertions))]',
          '#[cfg(all(target_os = "android", debug_assertions))]',
          '#[cfg(not(all(target_os = "android", debug_assertions)))]',
        ] : ['#[cfg(not(target_os = "android"))]']);
        if (path === LOGS_RS) {
          expect(source).toContain('share_debug_report(markdown).await');
          expect(source).toContain('"shared": true');
        } else {
          expect(source).toMatch(
            /#\[cfg\(not\(target_os = "android"\)\)\]\s*let picker = picker\s*\.add_filter\(/,
          );
          expect(source).toContain('picker.pick_file(move |p| {');
        }
        expect(
          (source.match(/\.save_file\(/g) ?? []).length,
          `${path} 的原生保存对话框调用数变了`,
        ).toBe(expectedSaveCalls);
      }
    });

    it('谓词自检：剥注释这一步真的在起作用（否则下面那条只是被注释喂饱）', () => {
      // 被禁的形态在这两个模块的**注释里**逐字写着（那是解释缺陷成因的话）。不剥注释，
      // 下面那条会因为一句解释而恒红；剥错了方向，它又会因为什么都扫不到而恒绿。
      expect(stripComments('let x = 1; // p.into_path().ok()')).not.toContain('into_path');
      expect(stripComments('let y = p.into_path().ok();')).toContain('into_path');
      for (const path of [LOGS_RS, BACKUP_RS]) {
        const raw = readRepoFile(path);
        expect(
          stripComments(raw).length,
          `${path} 剥注释后长度没变 —— 剥离没有作用在真实语料上`,
        ).toBeLessThan(raw.length);
      }
    });

    it('事实：两条导出腿真的处理了 content URI，没把它吃成「用户取消」', () => {
      for (const path of [LOGS_RS, BACKUP_RS]) {
        const code = stripComments(readRepoFile(path));
        expect(
          code,
          `${path} 又出现了 into_path().ok() —— SAF 的 content:// 会重新被吃成「用户取消」，` +
            '而那是一次静默失败：用户选好了保存位置，界面上什么都不会发生',
        ).not.toMatch(/into_path\(\)\s*\.\s*ok\(\)/);
        expect(
          code,
          `${path} 不再经 commands::picked_file 分派 —— 两种目标形态的读写没了统一出口`,
        ).toContain('picked_file::');
      }
    });

    it('事实：五条导出命令都在 generate_handler! 里，且那一整块没有 #[cfg]', () => {
      const block = invokeHandlerBlock(libRs);
      for (const command of [
        'logs_export',
        'diagnostic_export',
        'backup_export',
        'backup_import_pick',
        'backup_import_apply',
      ]) {
        expect(block, `${command} 不在 invoke_handler 注册表里`).toContain(command);
      }
      expect(
        platformGates(block),
        'invoke_handler 里出现了平台门控 —— 命令面在两端不再一致，本组结论要重判',
      ).toEqual([]);
    });
  });

  describe('B 组｜Android 按应用分流：Direct 档真的落到 tun.exclude_package', () => {
    const inbounds = readRepoFile(INBOUNDS_RS);

    it('谓词自检：链断掉的合成样本必须判 false', () => {
      expect(androidAppRoutingWired(inbounds), '真源码上判 false ⇒ 谓词本身坏了').toBe(true);
      // 三种断法各自都要被认出来，否则「判 true」可能只是谓词太宽。
      expect(androidAppRoutingWired(inbounds.replace('fn android_exclude_packages', 'fn xxx'))).toBe(
        false,
      );
      // 三处 `if deps.platform == "android"` 全部改掉才叫「这条门控没了」——
      // `replace` 只换第一处（那是 296 行那个无关的分支），换不到判据所在的那一块。
      expect(
        androidAppRoutingWired(inbounds.replaceAll('if deps.platform == "android"', 'if false')),
      ).toBe(false);
      expect(
        androidAppRoutingWired(inbounds.replace('tun.exclude_package = Some(', 'let _drop = Some(')),
      ).toBe(false);
    });

    it('事实：内置预设里有一批真的带 package_names（否则这条腿恒空转）', () => {
      const presets = readRepoFile(PRESET_DATA_RS);
      const count = (presets.match(/package_names:/g) ?? []).length;
      expect(
        count,
        '带 Android 包名的内置预设少于 10 条 —— 「按应用分流在 Android 上生效」这句话会退化成空话',
      ).toBeGreaterThanOrEqual(10);
    });

    it('事实：Rust 侧的 golden 测试正面钉着 exclude_package', () => {
      const golden = readRepoFile(GOLDEN_ANDROID_RS);
      expect(golden).toContain('exclude_package');
      expect(golden, 'golden 只剩反面断言 ⇒ 「什么都没发生」也能骗过它').toMatch(
        /fn\s+direct_app_rules_land_in_exclude_package/,
      );
    });
  });

  describe('C 组｜toast 宿主：移动端有了自己的一个（接线后的正面事实 + 归零棘轮）', () => {
    it('谓词自检：只有「装了实现」才算注入点，光有卸载腿不算', () => {
      // 正面：真正的注入认得出。
      expect(
        NON_EMPTY_INJECTION.test("setToastImpl({\n  success: (m) => push(m),\n});"),
      ).toBe(true);
      // 反面：卸载腿那句 `setToastImpl({})` **不算** —— 变异实测里正是它把本组喂绿过。
      expect(NON_EMPTY_INJECTION.test('return () => setToastImpl({});')).toBe(false);
      expect(NON_EMPTY_INJECTION.test("import { setToastImpl } from '@/lib/error-handler';")).toBe(
        false,
      );
    });

    it('事实：注入点恰好两个 —— 桌面外壳的 Toaster + 移动端自己的宿主', () => {
      expect(
        toastInjectionSites(),
        `toast 注入点集合变了。少了移动端那个 ⇒ 当前 ${SILENT_TOAST_WITHOUT_HOST} 处 toast 调用又回到静音；` +
          '多出第三个 ⇒ 两个宿主会互相顶掉（门面是模块级单例，后注入的赢）',
      ).toEqual(['components/layout/Toaster.tsx', 'mobile/MobileToaster.tsx']);
    });

    it('事实：移动树里没有任何人装载**桌面**的 Toaster（契约 A1 那条边界一格没松）', () => {
      const mounts = walkTypeScript(MOBILE_DIR)
        .filter((file) =>
          /<Toaster\b|from '@\/components\/layout\/Toaster'/.test(
            stripComments(readFileSync(file, 'utf8')),
          ),
        )
        .map((file) => relative(UI_SRC_DIR, file));
      expect(
        mounts,
        '移动端引了桌面 Toaster —— 它的外观全在 prototype.css/index.css 上，' +
          '整条桌面层叠链会被拖进移动包（`mobile-entry.test.ts` 的 CSS 集合恰等会红）',
      ).toEqual([]);
      // 正面对照：谓词认得出真的装载（否则上面那条空数组可能只是正则永远匹不到）。
      expect(/<Toaster\b/.test("return <Toaster />;")).toBe(true);
    });

    it('事实：克隆那条腿 2026-09-06 接上了 —— 处置从 absent 翻成 ported', () => {
      /* 这一条 2026-09-06（批 2）**翻面**。此前它是「宿主有了、但造节点面仍走桌面弹窗层 ⇒
         处置不动」；表单宿主（`mobile/forms/**`）落地之后那半个理由也不成立了，而克隆本身
         **根本不需要表单** —— 它是一次直调 `server_add` 的动作。
         本组现在断言的是**事实**：登记码是 ported，且那条腿真的在接线层里
         （不是「登记表改了个字」）。两条成对交 —— 只判登记码会被「改表不改码」骗过。 */
      const clone = ROW_ACTIONS.find((entry) => entry.id === 'clone');
      expect(clone, '登记表里没有 clone 这一条了 —— 重判本组是否还该存在').toBeDefined();
      expect(clone?.disposition.kind).toBe('ported');
      const wiring = stripComments(readRepoFile('ui/src/mobile/nodes/MobileNodesScreen.tsx'));
      expect(wiring, '克隆登记成 ported，接线层里却找不到那条腿').toContain('const cloneNode = useCallback(');
      expect(
        wiring,
        '克隆没过组网单例硬闸门 —— 它是绕开表单直调 `server_add` 的一条造节点路径，' +
          '不拦就能造出第二个 TS/WARP 实例',
      ).toContain('meshSingletonConflict(server, servers)');
      expect(wiring, '克隆没有真的调 `server_add`').toContain('api.server.add({ ...rest, name: cloneName })');
    });

    it('谓词自检：`installMobileToastHost` 的调用点认得出，而它自己的声明行不算', () => {
      // 共用谓词的正反夹具（含「声明行不算」那四对）—— 与 `app-wiring.test.tsx` ⑩ 同一份。
      for (const [name, line, expected] of CALL_SITE_FIXTURE) {
        expect(isCallSiteLine(name, line), `${name} @ ${line}`).toBe(expected);
      }
      // 反向对照：只有声明、没有调用点的取材面必须报零（否则下面那半判据恒真）。
      expect(
        callSitesIn(
          new Map([['mobile/syn.ts', 'export function installMobileToastHost(p: number) {\n  return p;\n}\n']]),
          'installMobileToastHost',
        ),
      ).toEqual([]);
    });

    it('事实：宿主组件真的调了注入函数（不只是「文件里含那段字面量」）', () => {
      expect(
        toastHostInstallSites(),
        '`installMobileToastHost(` 在移动树里零调用点 —— 宿主组件挂着但不再注入门面，' +
          `当前 ${SILENT_TOAST_WITHOUT_HOST} 处 toast 调用全部退回 console，而下面那条账本按文件文本匹配看不见这一档`,
      ).not.toEqual([]);
    });

    it(`账本：移动端静音 toast 调用 = ${SILENT_TOAST_BASELINE}（宿主接上后归零）`, () => {
      // marker：`scripts/wiring-verdict.mjs` 靠它证明**这一半门真的跑过**。
      // 缺了它就退 2 —— 只跑主门而拿它的绿宣布「全部接线」，是这对门之间唯一那条缝。
      console.log('HALF_TRUTH_FACTS=ok');
      const sites = toastInjectionSites();
      const { total, perFile } = silentToastCalls(sites, toastHostInstallSites());
      expect(
        total,
        '静音 toast 调用数变了。\n' +
          `变成非 0 ⇒ 移动端的 toast 宿主没了（回归），当前 ${SILENT_TOAST_WITHOUT_HOST} 处反馈重新落 console；\n` +
          `当前分布（这些调用本身仍在，只是不再静音）：${JSON.stringify(perFile, null, 2)}`,
      ).toBe(SILENT_TOAST_BASELINE);
    });

    it('反向对照之二：注入函数没人调时，账本同样必须弹回（守住「组件挂着但不注入」那一档）', () => {
      const notInstalled = silentToastCalls(toastInjectionSites(), []);
      expect(
        notInstalled.total,
        '把「谁调了 installMobileToastHost」这一半抽掉，账本竟然还是 0 —— ' +
          '说明 hosted 又退回了「文件里含那段字面量」的判法',
      ).toBe(SILENT_TOAST_WITHOUT_HOST);
    });

    it(`反向对照：把宿主从取材面里拿掉，账本必须弹回 ${SILENT_TOAST_WITHOUT_HOST}（证明这条棘轮有牙）`, () => {
      // 「全绿也需反向对照」：不喂一个已知应失败的输入，上面那条 `toBe(0)` 与
      // 「把当前全部 toast.* 调用删掉」在结果上完全不可区分 —— 而后者是这条账本明确禁止的走法。
      const withoutHost = silentToastCalls(
        ['components/layout/Toaster.tsx'],
        toastHostInstallSites(),
      );
      expect(
        withoutHost.total,
        `宿主不在时的静音数应为 ${SILENT_TOAST_WITHOUT_HOST}。` +
          '变少 = 有人把 `toast.*` 调用删掉了（那不是接线，是把话删了）；' +
          '变多 = 又写了一批新的反馈，确认它们真的能到用户眼前。\n' +
          `当前分布：${JSON.stringify(withoutHost.perFile, null, 2)}`,
      ).toBe(SILENT_TOAST_WITHOUT_HOST);
    });
  });
});
