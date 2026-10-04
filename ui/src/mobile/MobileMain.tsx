/**
 * 移动端前端入口（`ui/mobile.html` 唯一加载的模块）。
 *
 * # 为什么叫 `MobileMain.tsx` 而不是 `main.tsx`
 *
 * `ui/src/<label>/main.ts(x)` 在本仓是**次级窗 label** 的约定标记：
 * `src-tauri/src/tests/secondary_window_capabilities.rs` 按这个形状枚举窗口，并要求每一个都有一份
 * `windows` 含该 label 的 capability（`tray` / `update-popup` 就是这么来的）。
 * **移动端文档不是次级窗** —— 它就是 `main` 窗在 Android/iOS 上加载的那份文档，capability 走
 * `default.json` 的 `main`。若取名 `main.tsx`，那道门会把 `mobile` 当成一个新窗，
 * 逼人为一个**不存在的 window label** 造一份 capability —— 那是把假话写进登记表。
 * 改名让那条启发式重新说真话，而不是绕开它。（顺带满足 `.tsx` 一律 PascalCase 的命名门。）
 *
 * # CSS：共享基础与显式移动设计层
 *
 * `tokens.resolved.css` 是移动端**唯一** token 入口 —— 桌面 token 的终值不在 `tokens.css`，而是
 * `index.css` 那条 `tailwindcss → fonts.css → tokens.css → components.css → screens.css →
 * prototype.css → index.css 自有规则` 五层层叠压出来的（四个语义色的无障碍校准、`--sans` 的 CJK 族
 * 都在最后一层）。移动端按直觉引 tokens 声明层会拿到中间层，**不报任何错**，只在真机上看起来"差不多"。
 * `mobile-token-parity.test.ts` 仍夹住共享基线；用户要求的移动暗色重设计由 `theme.css`
 * 单独覆写，浅色和桌面继续使用基线。`mobile-entry.test.ts` 精确登记移动样式链与字体面。
 *
 * # 三件必须做、少一件就出真实缺陷的事
 *
 * 1. **`i18nReady` 之后再 render**。与桌面同一条 `i18n/index.ts`（不是 `auxiliary`）：移动端要移植的是
 *    整套桌面屏，用得上的正是那份完整文案面。不 await 就先渲染，首帧会是一屏 i18n key。
 * 2. **`disableNativeContextMenu()`**。WebView 自带的长按/右键菜单会盖住自绘菜单；本仓对**每个**
 *    webview 入口都要求这一句（`lib/native-context-menu.test.ts` 逐入口对拍）。
 * 3. **`reportRendererReady()`**。`window_health.rs` 的 mount 健康门在建窗那一刻**无条件**武装
 *    （不分平台），12s 收不到 `renderer:ready` 就 reload，再 12s 进终局兜底页。debug 档默认不武装，
 *    **release 档武装** ⇒ 少这一行，release APK 会在启动 24s 后把用户送进终局页，而 debug 下一切正常。
 *
 * # 白屏自愈的三层（与桌面 `src/main.tsx` 同一份实现）
 *
 * 实现与逐层职责见 `lib/renderer-recovery.ts` 头注。移动端此前一层都没有 —— 后果不是"少个功能"，
 * 而是 **Android 上移动包抛错 = 白屏且零日志**（Tauri 没有主进程 `console-message` 事件，
 * renderer 不主动上报就一行痕迹都不留）。这三层是移动端五个屏能进模拟器调试的前置。
 * `lib/renderer-recovery.test.ts` 对两个入口逐层正面对拍。
 */

import { StrictMode } from 'react';
import ReactDOM from 'react-dom/client';
import { isTauri } from '@tauri-apps/api/core';
import { platform } from '@tauri-apps/plugin-os';
import ErrorBoundary from '@/components/ErrorBoundary';
import { i18nReady } from '@/i18n';
import { disableNativeContextMenu } from '@/lib/native-context-menu';
import { injectStaticFailureDom, installErrorForwarding } from '@/lib/renderer-recovery';
import { reportRendererReady } from '@/lib/renderer-ready';
import { MobileApp } from './MobileApp';
import { installIosLayout, refreshNativeViewportCss } from './ios-layout';
import { DebugReportButton } from './DebugReportButton';
import '../styles/tokens.resolved.css';
import './theme.css';
import './mobile.css';
// Mobile-only design layer follows the five screen styles; desktop entry stays untouched.
import './redesign.css';
import './screens/rules/rules-redesign.css';
import './connections/connections-redesign.css';

installErrorForwarding();
disableNativeContextMenu();

// Keep the iOS layout switch out of desktop and Android. Tauri's OS plugin
// reports the target at build time, so this is ready before the first render.
if (isTauri() && platform() === 'ios') {
  document.documentElement.dataset.mobileOs = 'ios';
  // Keyboard geometry changes the visible viewport, not the React tree. Keep
  // the focused field visible by scrolling its form body only after layout.
  const viewport = window.visualViewport;
  if (viewport) {
    let frame = 0;
    const update = (): void => {
      frame = 0;
      const style = document.documentElement.style;
      const nativeViewport = refreshNativeViewportCss();
      const nativeBottom = Number.parseFloat(
        window.getComputedStyle(document.documentElement).getPropertyValue('--ios-keyboard-visible-bottom'),
      );
      const scale = viewport.scale || 1;
      // Native reports a height from the physical WebView top. offsetTop only
      // locates that visible region within the layout viewport; don't subtract it.
      const height = Math.max(0, Math.min(
        viewport.height,
        Number.isFinite(nativeBottom) ? nativeBottom / scale : Infinity,
      ));
      style.setProperty('--ios-form-viewport-height', `${height}px`);
      style.setProperty('--ios-form-viewport-top', `${viewport.offsetTop}px`);
      style.setProperty('--ios-form-visual-scale', String(scale));
      style.setProperty(
        '--ios-form-viewport-bottom-gap',
        `${Math.max(0, (nativeViewport?.height ?? document.documentElement.clientHeight) / scale - height)}px`,
      );
      const focused = document.activeElement;
      if (!(focused instanceof HTMLElement)) return;
      const body = focused.closest<HTMLElement>('.m-form-body');
      if (!body) return;
      const panel = focused.closest<HTMLElement>('.m-form-panel');
      const scroller = window.getComputedStyle(body).overflowY === 'visible' ? panel : body;
      if (!scroller) return;
      const field = focused.getBoundingClientRect();
      const fieldStyle = window.getComputedStyle(focused);
      // Outlines paint outside the border box returned by getBoundingClientRect.
      const outline = fieldStyle.outlineStyle === 'none' ? 0 : Math.max(0,
        (Number.parseFloat(fieldStyle.outlineWidth) || 0) +
        (Number.parseFloat(fieldStyle.outlineOffset) || 0),
      );
      const fieldTop = field.top - outline;
      const fieldBottom = field.bottom + outline;
      const bounds = scroller.getBoundingClientRect();
      const scrollerStyle = window.getComputedStyle(scroller);
      // The scrollport excludes its border, when present.
      const top = bounds.top + (Number.parseFloat(scrollerStyle.borderTopWidth) || 0);
      const innerBottom = bounds.bottom - (Number.parseFloat(scrollerStyle.borderBottomWidth) || 0);
      const footer = panel?.querySelector<HTMLElement>('.m-form-foot');
      const bottom = scroller === panel && footer && window.getComputedStyle(footer).position === 'sticky'
        ? Math.min(innerBottom, footer.getBoundingClientRect().top)
        : innerBottom;
      if (fieldTop < top) scroller.scrollTop += fieldTop - top;
      else if (fieldBottom > bottom) {
        scroller.scrollTop += Math.min(fieldBottom - bottom, fieldTop - top);
      }
    };
    const schedule = (): void => {
      if (!frame) frame = window.requestAnimationFrame(update);
    };
    viewport.addEventListener('resize', schedule);
    viewport.addEventListener('scroll', schedule);
    window.addEventListener('resize', schedule);
    window.addEventListener('polaris-ios-viewport-change', schedule);
    document.addEventListener('focusin', schedule);
    const disposeLayout = installIosLayout(schedule);
    schedule();
    import.meta.hot?.dispose(() => {
      window.cancelAnimationFrame(frame);
      viewport.removeEventListener('resize', schedule);
      viewport.removeEventListener('scroll', schedule);
      window.removeEventListener('resize', schedule);
      window.removeEventListener('polaris-ios-viewport-change', schedule);
      document.removeEventListener('focusin', schedule);
      disposeLayout();
    });
  } else {
    const disposeLayout = installIosLayout();
    import.meta.hot?.dispose(disposeLayout);
  }
}

const rootEl = document.getElementById('root');
if (rootEl) {
  void i18nReady
    .then(() => {
      try {
        ReactDOM.createRoot(rootEl).render(
          <StrictMode>
            <ErrorBoundary recoveryAction={<DebugReportButton />}>
              <MobileApp />
            </ErrorBoundary>
          </StrictMode>
        );
        // 外壳是同步渲染（无 Suspense 边界），render 返回即已提交首帧 ⇒ 此处即"真实页面已上屏"。
        reportRendererReady();
      } catch (err) {
        injectStaticFailureDom(rootEl, err);
      }
    })
    .catch((err: unknown) => injectStaticFailureDom(rootEl, err));
}
