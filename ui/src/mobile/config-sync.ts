/**
 * 移动端的**配置水合腿** —— 让 app-store 那份 `config` 在 Android/iOS 上也真的被装满。
 *
 * # 缺陷原样：写得进、读不回
 *
 * 前端有**两份**配置副本，各自独立地读同一份 `config.json`：
 *
 * | 副本 | 谁持有 | 谁装满它 | 谁让它收敛 |
 * |---|---|---|---|
 * | ① app-store 的 `config` + `servers`/`rules` 扁平镜像 | `store/app-store.ts` | `loadConfig()` | `api.config.onChanged` → `loadConfig(true)` |
 * | ② 设置屏漏斗自持的磁盘副本 | `components/screens/settings/use-config.ts` | 挂载那次 `configApi.get()` | 它**自己**的 `configApi.onChanged` 订阅 |
 *
 * 桌面把 ① 的两条腿都挂在 `App.tsx`（挂载首拉 + `onChanged` 重拉）。移动端三个入口文件
 * （`MobileMain` / `MobileApp` / `MobileShell`）一处都没挂 ⇒ **① 在移动端恒为 `null`**：
 * `useEffectiveConfig()` 恒空、`useEffectiveServers()` 恒空、`useEffectiveRules()` 恒空。
 * 于是首页那张模式卡渲染的是 `config?.proxyMode ?? 'smart'` 里的兜底常量，而不是盘上的值；
 * 节点屏、规则屏读到的同样是空集合。而 ② 一直是好的 —— 设置屏因此在**同一时刻**显示正确，
 * 这正是「写得进、读不回」这个症状的来源：写走 ②（`useConfig().update`），读走 ①。
 *
 * # 三条路里为什么选「把 ① 在移动端接上」
 *
 * · **让首页改读 ②**（首页本来就已经 `const { update } = useConfig()`，改一行就能拿到值）——
 *   只治首页那一颗芯片。`servers` / `rules` 两个扁平镜像**只由 `loadConfig`/`saveConfig` 写**
 *   （`app-store.ts` 里 `servers:` 的全部赋值点），② 不碰它们 ⇒ 节点屏与规则屏照旧空着。
 *   把「读不回」按屏逐个改读 ②，等于让移动端凭空多出一套与桌面不同的读点分类，
 *   而 `lib/config-read-wiring.test.ts` 的整张去向登记表是按 ① 的形态建的。
 * · **把两份合一**（让 ② 直接吃 ①）—— 真正的根因修法，但改的是 `use-config.ts`，
 *   那是桌面 9 个设置子页与 3 个弹窗共用的漏斗，且带着首帧种子、代际守卫、U-7 重启提示、
 *   暂存分流四条互相咬合的腿。桌面此刻**没有**这个缺陷（两份都活着、都收敛），
 *   为移动端的一条缺腿去动桌面的汇流点，改动半径与收益不成比例。
 * · **本文件这条**：把桌面已经在跑、且被 `config-read-wiring` 登记表管辖的那两条腿，
 *   在移动端也挂上。一处接线同时治好首页 / 节点 / 规则三屏，且移动端与桌面从此同构。
 *
 * # 两份副本同时活着会不会互相打架
 *
 * 不会，前提是**两份都订了同一条广播**。后端每一条写盘路径都汇到
 * `commands/config.rs` 的 `broadcast_config_changed_with` → 一条**无载荷**的 `config:changed`；
 * 两份副本各自收到即各自重拉整份，收敛到同一个磁盘真值。谁先谁后只影响几毫秒内的先后，
 * 不产生分叉 —— 桌面就是这么跑的。
 *
 * 今天移动端的病灶恰恰是这条对称性缺了一半：只有 ② 订了广播，① 既没首拉也没订阅，
 * 于是「在 ② 上写一笔」这件事 ① 永远不知道。本文件补上的就是缺的那一半。
 *
 * 唯一的行为差：移动端首页那颗芯片的写腿走 ②（`useConfig().update`），不像桌面走
 * `app-store.updateProxyMode` 那样同步改 ①，故点完之后要等一次广播回声才回显。
 * 那是一次 IPC 往返的量级，且它**收敛**——不是分叉。写腿归属由 `lib/config-write-wiring.test.ts`
 * 的登记表管辖，不在本线射程内改。
 *
 * # 退订与泄漏
 *
 * `ipc-client.ts` 的 `listen()` **同步**返回退订闭包，且自带「注册还在飞时就被 cleanup」
 * 的补退订（那里的 `cleaned` 标志）。故这里把它原样当作 effect 的 cleanup 返回即可，
 * 卸载后不会再有回调落到 store 上（`config-sync.test.tsx` ③ 正面对拍：退订后再发一帧，
 * store 一动不动，而退订**之前**同一帧确实推动过它）。
 *
 * # 为什么拆成 [`startConfigSync`] + [`useMobileConfigSync`] 两个导出
 *
 * 本仓 vitest 跑 `environment: 'node'`、刻意不装 jsdom ⇒ `useEffect` 在
 * `renderToStaticMarkup` 下**不执行**。effect 体若直接写在 `MobileApp` 里，
 * 「首拉 / 重拉 / 退订」三件事就一条行为判据都写不出来，只剩源码级断言。
 * 拆开之后：effect 体是一个普通函数，能在 node 里喂假 IPC 真跑（判据 ①②③④）；
 * 而 hook 是在**渲染期**被调用的，`MobileApp` 到底挂没挂它可以用一次真渲染断言
 * （判据 ⑤，不是源码扫描）。剩下 hook 体那一行由判据 ⑥ 的源码断言钉住。
 *
 * ⚠️ 登记：本文件是 `.onChanged(` 的**第四个**渲染端消费方，而
 * `src-tauri/src/commands/config/tests/config_changed_payload_tests.rs::ts_consumers()`
 * 那张表里只有 `App.tsx` / `TrayMenu.tsx` / `use-config.ts` 三行 —— 那道 Rust 门按表逐文件扫，
 * 表外的文件它一眼都不看，故本文件既不会让它转红、也不受它管辖。回调形态
 * （零形参箭头 ⇒ 结构上读不到 payload）在本文件这侧由 `config-sync.test.tsx` ②⑥ 自守；
 * 把本文件补进那张 Rust 表需要动 `src-tauri/**`，不在本线的文件面内。
 */

import { useEffect } from 'react';
import { api } from '@/ipc';
import { useAppStore } from '@/store/app-store';

/**
 * 挂上 app-store 配置副本的两条腿，返回退订闭包。
 *
 * · **挂载首拉**不带 `force`：单飞会把启动期的重复拉取合并掉。
 * · **广播重拉必须 `force`**：非 force 会被在飞的那次 `loadConfig` 合并 —— 写 1 的回声启动 get，
 *   写 2 在其在飞期间落盘、其回声被合并进写 1 那次 get，而那次 get 携带的是写 2 之前的快照 ⇒
 *   store 停在旧值且不会再有刷新（与磁盘持久分叉）。这条逐字取自桌面 `App.tsx` 同一处的结论。
 * · 回调**零形参**：`config:changed` 是无载荷信号，读它只会拿到 `{}`。
 */
export function startConfigSync(): () => void {
  void useAppStore.getState().loadConfig();
  return api.config.onChanged(() => void useAppStore.getState().loadConfig(true));
}

/** 移动端根组件挂这一条即可（`MobileApp`）。空依赖：两条腿的生命周期 = 整个应用。 */
export function useMobileConfigSync(): void {
  useEffect(startConfigSync, []);
}
