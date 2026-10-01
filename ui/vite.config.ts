// Vite 配置（Polaris / Tauri 2 集成）。
//
// Tauri 官方推荐：
//  - clearScreen: false —— 保留 cargo/rust 编译输出，dev 时 rust 报错不被 vite 清屏盖掉。
//  - server.port 对齐 tauri.conf.json 的 devUrl（:5173）。
//  - strictPort —— 端口占用直接报错（而非递增），否则 Tauri 连不上 webview 会静默黑屏。
//  - HMR 端口与 dev server 分离；忽略 Rust 侧文件变更（改 Rust 不触发前端热更，无意义）。
//  - envPrefix: VITE_ / TAURI_ —— 暴露 TAURI_ENV_* 给前端读取（平台/架构）。
//  - resolve.alias '@' → src（与 tsconfig paths 对齐）。
//
// Tauri 2 提供 @tauri-apps/cli 的 vite 插件（host 配置等），但底座阶段用纯 vite 配置 + 手写对齐
// 已足够；cli 主要提供 tauri dev/build 命令编排（beforeDevCommand 等，已在 tauri.conf.json 配好）。
/// <reference types="vitest/config" />
import { defineConfig } from 'vitest/config';
import react from '@vitejs/plugin-react';
import path from 'node:path';

// Tauri dev 环境变量（TAURI_ENV_PLATFORM 等），供条件编译用
const host = process.env.TAURI_DEV_HOST;
const nodeMajor = Number.parseInt(process.versions.node, 10);

export default defineConfig({
  plugins: [react()],
  clearScreen: false,

  resolve: {
    alias: {
      '@': path.resolve(import.meta.dirname, './src'),
    },
  },

  // 仅暴露 VITE_ 与 TAURI_ 前缀的环境变量给前端
  envPrefix: ['VITE_', 'TAURI_'],

  server: {
    port: 5173,
    strictPort: true,
    // Tauri 移动端 dev（via @tauri-apps/cli mobile）走网络 host
    host: host || false,
    hmr: host
      ? {
          protocol: 'ws',
          host,
          port: 5174,
        }
      : undefined,
    watch: {
      // 忽略 Rust 侧变更（改 src-tauri 不该触发前端 HMR）
      ignored: ['**/src-tauri/**'],
    },
  },

  build: {
    // Tauri webview 用相对路径加载本地文件；不指定 base 时默认相对，满足 frontendDist(../ui/dist)。
    target: ['es2021', 'chrome100', 'safari13'],
    // 生产构建产物最小化（Tauri 打包体积敏感）
    minify: !process.env.TAURI_ENV_DEBUG ? 'oxc' : false,
    sourcemap: !!process.env.TAURI_ENV_DEBUG,
    // 主入口本就该是一整块：导航层刻意不做代码分割（理由见 `src/components/screens/ScreenRouter.tsx`
    // 顶部）。默认 500kB 警告开口就建议「用 dynamic import 拆包」——在本项目里它推的正是刚被否掉的
    // 方案，每次构建刷一遍只会把下一个人拉回原地。阈值抬到主包实际量级之上，让这条提示重新有信息量。
    chunkSizeWarningLimit: 700,

    rollupOptions: {
      // 多入口：主窗（index）+ mini 更新弹窗（update-popup）+ 托盘自绘浮层（tray）+ 移动端外壳（mobile）。
      //
      // 弹窗/浮层**刻意不复用 index.html**：复用主入口会把整个 React 应用（i18n / 路由 / 全部 provider）
      // 塞进小窗——既拖慢首帧，又会让主窗白屏自愈门（`window_health.rs` 只认 label=="main"）对着它们误判。
      // update-popup 是零框架 vanilla TS；tray 浮层需真实接线（连接态/节点/api）故用精简 React 入口。
      //
      // `mobile` 走独立入口的理由**与上面那两个不同**（上面那条理由在移动端不成立：移动外壳就是
      // label=="main" 的整个应用，白屏自愈门正该管它）。真正的判据是 **CSS 契约 A1**：移动端唯一
      // token 入口是 `src/styles/tokens.resolved.css`，**不得**走桌面那条 `index.css` 五层层叠链。
      // 而 `index.html` → `src/main.tsx` 里 `import './styles/index.css'` 是**静态**的：同一份文档
      // 服务两端，移动端必然连带整条桌面层叠。要在同一文档里躲开它，只能把桌面 CSS 改成动态 chunk
      // ——那等于**为移动端把桌面首帧从"HTML 里的阻塞 <link>"降级成"JS 跑起来才注入"**，把 FOUC
      // 窗口开给已发布的桌面产品。一份文档一个平台，是这里唯一不拿桌面首帧换的形态。
      // 装载面（谁在 Android 上打开 mobile.html）见 `src-tauri/src/lib.rs` 的 `create_main_window`。
      input: {
        index: path.resolve(import.meta.dirname, 'index.html'),
        'update-popup': path.resolve(import.meta.dirname, 'update-popup.html'),
        tray: path.resolve(import.meta.dirname, 'tray.html'),
        mobile: path.resolve(import.meta.dirname, 'mobile.html'),
      },
    },
  },

  // Vitest —— 仅跑纯逻辑单测（Csel 定位/键盘协议、URL 推断/字段校验），无 DOM 依赖，
  // 走默认 node 环境（不引 jsdom/happy-dom，零额外运行时；与已装 vite 同源）。
  test: {
    environment: 'node',
    include: ['src/**/*.{test,spec}.{ts,tsx}'],
    // Node 25+ 默认暴露实验性 Web Storage；纯逻辑测试不使用它，关闭后既避免每个 worker
    // 访问 globalThis.localStorage 时刷 ExperimentalWarning，也保持旧 Node 的“未提供”语义。
    execArgv: nodeMajor >= 25 ? ['--no-experimental-webstorage'] : [],
  },
});
