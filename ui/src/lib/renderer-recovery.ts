/**
 * 白屏自愈的 renderer 侧共享防线 —— 每个**完整应用入口**都要装的那两层实现。
 *
 * 本模块是从 `ui/src/main.tsx` 原样抽出来的（行为逐字节等价，抽取当次不含任何"改进"）。
 * 抽取的理由是复用面：移动端文档 `mobile/MobileMain.tsx` 是 `main` 窗在 Android/iOS 上加载的那份，
 * 与桌面同为完整 React 应用、同样会白屏，而这两层此前是 `main.tsx` 的**模块私有函数** ——
 * 移动端因此长期处在「抛错 = 白屏且零日志」的状态（真机上连一行痕迹都没有，等于盲调）。
 *
 * ── 三层分工（主进程侧见 `src-tauri/src/window_health.rs`）────────────────────
 *  1. **可观测性**（本模块 `installErrorForwarding`）：console.error / window.onerror /
 *     unhandledrejection → 转发 Rust 日志。Tauri **没有** Electron 的主进程 `console-message` 事件，
 *     renderer 不主动上报就等于零痕迹 —— 而「白屏时日志一行都没有」正是排障最大的阻碍。
 *  2. **渲染期抛错**：根级 `<ErrorBoundary>`（`components/ErrorBoundary.tsx`，本来就是共享组件，
 *     无需抽取；它必须包住应用根及其所有 provider，理由见该文件头注）。
 *  3. **同步 mount 抛错**（本模块 `injectStaticFailureDom`）：`createRoot().render()` 外的 try/catch
 *     → 注入静态兜底 DOM。try/catch 那几行是入口自己的胶水，留在各入口；可复用的是兜底 DOM 本身。
 *
 * 各层兜不住的部分由主进程 mount 健康门（超时 → reload → 终局页）收口：
 *  - 模块级 import 抛错（入口文件根本没跑起来）→ 只有主进程门能兜。
 *  - React concurrent 渲染的异步抛错 → 由根 ErrorBoundary 兜（不进入口的 try/catch）。
 *
 * 「两个入口都真的装了这三层」由 `renderer-recovery.test.ts` 逐层正面钉住。
 */

import { invoke } from '@tauri-apps/api/core';
import { IPC_CHANNELS } from '@/domain/ipc-channels';
import { reportRendererReady } from '@/lib/renderer-ready';
import { recoveryText } from '@/i18n/recovery-text';

/** 尽力上报，绝不因上报失败再抛（错误处理器自己抛错 = 错误风暴）。 */
function reportSafely(level: 'error' | 'warn', message: string): void {
  try {
    void invoke(IPC_CHANNELS.RENDERER_LOG, { level, message }).catch(() => {});
  } catch {
    /* 非 Tauri 环境（vite dev 纯浏览器预览）/ IPC 不可用：静默 */
  }
}

/**
 * 装可观测性钩子。**必须最先执行**（在任何应用模块求值之前），否则早期抛错落不进日志。
 *
 * 限频放在 Rust 侧（`window_health.rs` 的 `admit_console_message`，滑动窗口 10 条/秒）：单一权威点 +
 * 纯函数可测。此处只负责如实上报。
 */
export function installErrorForwarding(): void {
  // console.error 转发：保留原实现（devtools 里仍要看得见），只做旁路。
  const originalError = console.error.bind(console);
  console.error = (...args: unknown[]) => {
    originalError(...args);
    reportSafely(
      'error',
      args
        .map((a) => (a instanceof Error ? `${a.message}\n${a.stack ?? ''}` : String(a)))
        .join(' ')
    );
  };

  window.addEventListener('error', (event: ErrorEvent) => {
    const err = event.error as Error | undefined;
    reportSafely('error', `未捕获错误：${err?.message ?? event.message}\n${err?.stack ?? ''}`);
    // 刻意**不** preventDefault：吞掉后 devtools/vite overlay 就看不到了，排障反而更难。
  });

  window.addEventListener('unhandledrejection', (event: PromiseRejectionEvent) => {
    const reason: unknown = event.reason;
    const message = reason instanceof Error ? reason.message : String(reason);
    const stack = reason instanceof Error ? (reason.stack ?? '') : '';
    reportSafely('error', `未处理的 Promise 拒绝：${message}\n${stack}`);
  });
}

/**
 * `createRoot().render()` 的**同步**早期抛错（render 调用栈内同步 throw，未进 React 提交阶段）的最后
 * 一道防线：注入纯静态 DOM（不依赖 React / i18next / theme；文案来自同步 auxiliary locale），
 * 避免只剩纯背景色空窗。
 *
 * 分工：React concurrent render 的错误异步浮出 → 根 ErrorBoundary 兜（不进此 catch）；模块级 import
 * 抛错（入口文件没跑起来）→ 主进程 mount 健康门兜。此处只兜同步 render 抛错。
 */
export function injectStaticFailureDom(root: HTMLElement, err: unknown): void {
  const detail = err instanceof Error ? `${err.message}\n${err.stack ?? ''}` : String(err);
  // 上报是此路径唯一的可观测信号。
  reportSafely('error', `React mount 失败（createRoot.render 同步抛错）：${detail}`);
  const shell = document.createElement('div');
  shell.style.cssText =
    'min-height:100vh;display:flex;align-items:center;justify-content:center;text-align:center;' +
    'padding:24px;box-sizing:border-box;background:#0B0F14;color:#E6EDF3;' +
    'font-family:system-ui,-apple-system,"Segoe UI",Roboto,sans-serif';
  const content = document.createElement('div');
  content.style.maxWidth = '520px';
  const title = document.createElement('h1');
  title.style.cssText = 'font-size:18px;font-weight:600;margin:0 0 12px';
  title.textContent = recoveryText('title');
  const body = document.createElement('p');
  body.style.cssText = 'font-size:13px;line-height:1.6;color:#9DA7B3;margin:0 0 20px';
  body.textContent = recoveryText('body');
  const reload = document.createElement('button');
  reload.type = 'button';
  reload.style.cssText =
    'font-size:13px;color:#fff;background:#2F81F7;border:0;border-radius:8px;' +
    'padding:9px 20px;cursor:pointer';
  reload.textContent = recoveryText('reload');
  reload.addEventListener('click', () => window.location.reload());
  content.append(title, body, reload);
  shell.append(content);
  root.replaceChildren(shell);
  // 静态兜底 DOM 已提交且按钮可交互；立即兑现上屏，不能再让用户额外等 3s show deadline。
  reportRendererReady();
}
