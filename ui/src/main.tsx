/**
 * Polaris 前端入口（React 19 挂载点）。
 *
 * 全局 CSS（Aurora 设计系统底座）+ i18n 初始化 + React 挂载 + **白屏自愈的 renderer 侧防线**。
 *
 * 白屏自愈的三层分工（实现与完整说明见 `lib/renderer-recovery.ts`；主进程侧见
 * `src-tauri/src/window_health.rs`）：
 *  1. **可观测性**：`installErrorForwarding()` —— console.error / window.onerror /
 *     unhandledrejection → 转发 Rust 日志。
 *  2. **渲染期抛错**：根级 `<ErrorBoundary>`（包住 App 及其所有 provider）。
 *  3. **同步 mount 抛错**：`createRoot().render()` 外的 try/catch → `injectStaticFailureDom()`。
 *
 * 这三层同样装在移动端文档 `mobile/MobileMain.tsx` 上（两个入口一份实现），
 * 由 `lib/renderer-recovery.test.ts` 逐层对拍。
 */

import React from 'react';
import ReactDOM from 'react-dom/client';
import App from './App';
import ErrorBoundary from './components/ErrorBoundary';
import { disableNativeContextMenu } from './lib/native-context-menu';
import { injectStaticFailureDom, installErrorForwarding } from './lib/renderer-recovery';
import './styles/index.css';
import { i18nReady } from './i18n';

installErrorForwarding();
// webview 自带右键菜单 → 关掉（自绘的行/节点菜单不受影响，理由见该模块头注）。
disableNativeContextMenu();

const rootEl = document.getElementById('root');
if (rootEl) {
  void i18nReady
    .then(() => {
      try {
        ReactDOM.createRoot(rootEl).render(
          <React.StrictMode>
            <ErrorBoundary>
              <App />
            </ErrorBoundary>
          </React.StrictMode>
        );
      } catch (err) {
        injectStaticFailureDom(rootEl, err);
      }
    })
    .catch((err: unknown) => injectStaticFailureDom(rootEl, err));
}
