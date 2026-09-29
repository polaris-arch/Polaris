/**
 * 自定义应用**包名那一格**的装袋门（批 16）。
 *
 * # 为什么单开这一条（它守的缝，别的门一条都盖不住）
 *
 * 包名这条链跨了三层，每一层各有自己的门，而**它们全绿时这一格照样可以是坏的**：
 *  · Rust 侧有正面断言「包名真的流到 `tun-in.exclude_package`」
 *    （`app_rules_preset/tests/mod.rs#get_custom_preset`）—— 它从一份**夹具**出发，
 *    证明不了「表单装袋时装的是用户挑的那些」；
 *  · `frontend_sot_guard.rs` 有键名契约（TS ⇄ Rust 的 `packageNames` 逐字相等）——
 *    它证明键名没漂，证明不了那个键里装的是什么；
 *  · 接线完成度门数「还剩几条没接」—— 它数的是渲染点与登记表。
 * 缝正好在中间那一跳：**表单把哪个值装进 `packageNames`**。
 *
 * 后果不是显示不对，是**静默不生效**：装成 `app.label`（"Chrome"）而不是 `app.packageName`
 * （`com.android.chrome`）时，表单照样提交、盘上照样有 `packageNames`、Rust 照样把它发射进
 * `exclude_package` —— 然后 `VpnService.Builder.addDisallowedApplication` 对每一条抛
 * `NameNotFoundException`，官方客户端 catch 后继续跑，**一条都命不中**。
 * 那正是本批接通之前那条债的原样，只是这次界面上写着「已按应用匹配」。
 *
 * # 判据是**源码级**的，这一条如实写清楚
 *
 * 本仓 vitest 跑 `environment: 'node'`、刻意不装 jsdom ⇒ 点不了列表里的那一行，
 * 而 `pkgSel` 是组件内 state，没有第二条路能把它填上。真机上「点一行 → 提交 → 盘上有包名」
 * 归真机验收；本门钉的是源码面的四跳，外加一次**切点自检**（取材面塌了要当场说话，
 * 否则一堆 `toContain` 在空串上恒绿）。
 */

import { describe, expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

/**
 * 剥注释、**保留字符串**、保长度（与 `nodes-screen.test.tsx` 同一口径）。
 *
 * 不剥就会把本文件要断言的那些标识符从**头注的引文里**抓进取材面 —— 上面那段散文里
 * `app.packageName` 与 `app.label` 各出现过一次，不剥的话「装的是包名」这条恒绿。
 */
function strip(src: string): string {
  let out = '';
  let i = 0;
  const blank = (s: string): string => s.replace(/[^\n]/g, ' ');
  while (i < src.length) {
    const c = src[i];
    const n = src[i + 1];
    if (c === '/' && n === '*') {
      const end = src.indexOf('*/', i + 2);
      const stop = end === -1 ? src.length : end + 2;
      out += blank(src.slice(i, stop));
      i = stop;
    } else if (c === '/' && n === '/') {
      const end = src.indexOf('\n', i);
      const stop = end === -1 ? src.length : end;
      out += blank(src.slice(i, stop));
      i = stop;
    } else if (c === "'" || c === '"' || c === '`') {
      let j = i + 1;
      while (j < src.length && src[j] !== c) j += src[j] === '\\' ? 2 : 1;
      out += src.slice(i, Math.min(j + 1, src.length));
      i = j + 1;
    } else {
      out += c;
      i += 1;
    }
  }
  return out;
}

const PANEL = strip(
  readFileSync(fileURLToPath(new URL('./AppAddPanel.tsx', import.meta.url)), 'utf8'),
);

/** 落盘对象那一段（`const preset: CustomAppPreset = { … }` 的体）。 */
function presetLiteral(): string {
  const head = 'const preset: CustomAppPreset = {';
  const at = PANEL.indexOf(head);
  expect(at, '取材面塌了：`AppAddPanel` 里找不到落盘对象那一段 —— 下面每条断言都会在空串上跑').toBeGreaterThan(
    -1,
  );
  const rest = PANEL.slice(at + head.length);
  let depth = 1;
  for (let i = 0; i < rest.length; i += 1) {
    if (rest[i] === '{') depth += 1;
    else if (rest[i] === '}') {
      depth -= 1;
      if (depth === 0) return rest.slice(0, i);
    }
  }
  throw new Error('落盘对象字面量没有闭合');
}

describe('自定义应用：包名那一格的装袋', () => {
  it('⓪ 切点自检：取材面非空，且剥注释器真的把散文剥掉了', () => {
    expect(PANEL.length, '面板源码读空了').toBeGreaterThan(2000);
    const body = presetLiteral();
    expect(body.length, '落盘对象字面量切出来是空的').toBeGreaterThan(50);
    /* 反向对照：本文件头注里写着 `app.packageName`，而**面板的头注**里也讲过同一件事。
       若剥注释器失灵，`PANEL` 里会同时留下面板头注那些引文 —— 用一个只在面板头注里出现、
       代码里绝不会有的串来证明它真的被剥掉了。 */
    expect(PANEL, '剥注释器失灵：面板头注的散文还在取材面里').not.toContain('静默不生效');
  });

  it('① 落盘对象里有 `packageNames`，且它装的是选中集（不是某个字面量）', () => {
    expect(
      presetLiteral(),
      '落盘对象没有装 `packageNames` —— 用户挑的包名不会写进盘，「按应用匹配」整条哑掉',
    ).toContain('packageNames: [...pkgSel]');
  });

  it('② 没挑就不写这个键（空数组与缺省等价，Rust 侧同一口径）', () => {
    expect(
      presetLiteral(),
      '空选中集也会写出一个 `packageNames: []` —— 盘上多一个没有含义的键，与 Rust 侧 '
        + '`skip_serializing_if = "Vec::is_empty"` 的口径也对不上',
    ).toContain('pkgSel.size > 0 ?');
  });

  it('③ 🔴 选中的是 `packageName`，不是 `label`（这一格错了会静默命不中）', () => {
    expect(
      PANEL,
      '列表行没有把 applicationId 交给选中集 —— 装 label（"Chrome"）进去的话，'
        + 'addDisallowedApplication 对每一条抛 NameNotFoundException，一条都命不中，且是静默的',
    ).toContain('togglePkg(app.packageName)');
    expect(PANEL, '把显示名当标识用了').not.toContain('togglePkg(app.label)');
  });

  it('④ 包名与进程名是两条独立的腿，不许由同一个值派生', () => {
    const body = presetLiteral();
    /* 进程名那一格走 `parseProcessNames(proc)`，包名那一格走 `pkgSel`。两者串台 = 把桌面那条腿的
       值喂给 Android 那条腿，正是本批接通之前的缺陷形态（只是那时它至少是空的）。 */
    expect(body, '`processNames` 那一格不见了').toContain('processNames');
    expect(body, '包名由进程名派生 —— 它们是同一件事的两个平台形态，不是同一个值').not.toContain(
      'packageNames: processNames',
    );
    expect(PANEL, '选中集被进程名输入框写过').not.toContain('setPkgSel(parseProcessNames');
  });

  it('⑤ 数据源是 `listInstalledApps`，且失败**不折成空表**', () => {
    expect(PANEL, '没有从已装应用清单取候选 —— 那张列表要么是空的、要么是编出来的').toContain(
      'api.system.listInstalledApps()',
    );
    /* 「读不到」与「一个都没有」必须分得开：后端为此刻意不返空表（`success:false` + 原因）。
       面板这一侧用 `appsFailed` 承载那一档，并画出一句自己的说明。 */
    expect(PANEL, '失败没有单独的一档 —— 它会与「这台设备上一个应用都没有」长得一样').toContain(
      'setAppsFailed(true)',
    );
    expect(PANEL, '失败那一档没有自己的文案').toContain('mobileRules.customAppPackagesFailed');
  });
});
