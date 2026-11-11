/**
 * 连接页行右键菜单（G4）的**接线**守卫。
 *
 * 为什么是源码结构守卫：本仓 vitest 是 node 环境、全仓无组件渲染测试（`App.test.ts` 头注），
 * 而这条缺口的性质正是「后端全在、只差一个菜单」—— 四个动作各自的实现早有覆盖
 * （`connections_close` 有 Rust 侧测试、`rules.add` 有规则页测试、`clampToWrap` 有本仓
 * `lib/overlay-position.test.ts`），缺的从来是**调用点**。逻辑单测再多也照不出「菜单没接上」。
 * 沿用既有范式（`nodes/nodes-speedtest-wiring.test.ts`、`store/latency-wiring-invariants.test.ts`）。
 *
 * 守形态不守措辞：断言的是「哪条腿调了什么」，改文案/改变量名不误伤，把动作摘掉必然转红。
 */
import { describe, it, expect } from 'vitest';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

/**
 * 去注释后的源码 —— 所有断言都跑在它上面。
 *
 * 本文件的注释里逐字写着被守的调用形态（`onClose(row)` 等），直接扫原文会让
 * 「把动作删了、注释留着」照样绿。`[^:]` 前瞻避免把 `https://` 当行注释。
 */
const code = (src: string): string =>
  src.replace(/\/\*[\s\S]*?\*\//g, '').replace(/(^|[^:])\/\/.*$/gm, '$1');

const SRC = code(
  readFileSync(fileURLToPath(new URL('./ConnectionsScreen.tsx', import.meta.url)), 'utf8'),
);

describe('行右键菜单已接线', () => {
  /**
   * 三条关闭腿缺一不可，滚动那条尤其容易漏。
   *
   * 变异对照：删掉 `scroll` 监听（或去掉捕获参数 `true`）→ 第三条转红。菜单是 `position:fixed`，
   * 不关的话表一滚它就浮在原处指着**另一行**——比没有菜单更危险。
   * 滚动事件不冒泡，非捕获阶段收不到 `.conn-scroll` 的滚动。
   */
  it('点空白 / ESC / 滚动都能关掉菜单', () => {
    expect(SRC).toContain("document.addEventListener('mousedown', onDown)");
    expect(SRC).toContain("document.addEventListener('keydown', onKey)");
    expect(SRC).toContain("document.addEventListener('scroll', onScroll, true)");
    // 注册了就必须卸载，否则每次开菜单都往 document 上叠一层监听。
    expect(SRC).toContain("document.removeEventListener('scroll', onScroll, true)");
  });
});
