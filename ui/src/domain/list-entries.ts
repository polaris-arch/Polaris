/**
 * 字符串列表编辑器的**纯逻辑** —— 批量导入的解析/合并、逐项相等、以及「外部改动到达时草稿取什么值」。
 *
 * # 为什么住在这里而不是组件旁边
 *
 * 这三条是**领域规则**，不是某个组件的实现细节：大小写不敏感去重、`max` 上限、以及
 * 「用户正在编辑就别打断」的草稿守卫，两端必须一模一样。上一版它们长在
 * `components/screens/settings/ListEditor.tsx` 里 —— 那个文件同时 import 桌面原语
 * （`./Primitives` → `dialogs/Csel`），移动端要用就得把整条桌面组件链拖进移动入口。
 * 于是移动端只剩「抄一份」这一条路，而两份去重规则必然各自漂移（本仓已经在 DNS 预设表上
 * 吃过同一个亏）。2026-09-06 搬到 `domain/`，两端读同一份，组件那边原样 re-export。
 *
 * 单测在 `components/screens/settings/ListEditor.bulk-import.test.ts` 与 `.draft.test.ts`
 * （它们经 re-export 打到这里，搬家不改测试文件）。
 */

/**
 * 批量导入的解析/合并（**纯函数，单测在 ListEditor.bulk-import.test.ts**）：
 * 拆分 `/[,\n]/` → trim → 去空 → 与既有条目及批内互相去重（大小写不敏感）→ 尊重 max 上限。
 *
 * 抽成自由函数最初是为了**可测**（留在组件闭包里就只能靠渲染断言，而本仓 vitest 跑在 node 环境、
 * 无 jsdom，那等于这段边界最多的逻辑一条断言都没有）；2026-09-06 起它还有第二个调用点
 * （移动端 TUN 页的网段清单），去重规则因此必须只有这一份。
 *
 * @param draft 用户粘贴的原文
 * @param existing 当前列表（原样保留，含用户点「添加」留下的空行）
 * @param max 上限；达到即停止追加（不报错，与「添加」按钮的 atCap 语义一致）
 * @returns 合并后的**完整**新列表
 */
export function parseBulkEntries(draft: string, existing: string[], max?: number): string[] {
  const next = [...existing];
  // 去重基准里剔掉空串：既有的空行不该让粘贴内容里的第一个条目被误判成重复。
  const seen = new Set(next.map((s) => s.trim().toLowerCase()).filter(Boolean));
  for (const raw of draft.split(/[,\n]/)) {
    const entry = raw.trim();
    if (!entry) continue;
    const key = entry.toLowerCase();
    if (seen.has(key)) continue;
    if (max !== undefined && next.length >= max) break;
    seen.add(key);
    next.push(entry);
  }
  return next;
}

/** 两个列表逐项相同（长度 + 顺序 + 每一项）。 */
export function sameEntries(a: readonly string[], b: readonly string[]): boolean {
  return a.length === b.length && a.every((s, i) => s === b[i]);
}

/**
 * 外部改动到达时草稿该取什么值 —— draft+onBlur 这套里**最容易写错的一格**，故抽成纯函数单测。
 *
 * 判据同 `SettingsDns` 的 `seededRef` 守卫：
 *  - 草稿 ≠ 上次种子 ⇒ 用户正在编辑，**保留草稿**（外部刷新绝不能把人正在敲的字符抹掉）；
 *  - 草稿 == 上次种子 ⇒ 用户没动过，**跟随新配置**（托盘 / 备份恢复 / 另一屏保存要能回填）。
 *
 * 第三条是本仓 props 身份易变带来的：内容已相同就返回**原引用**，避免 `value` 每次父级重渲
 * 都换新数组时白白多一次 `setState` 重渲。
 */
export function nextDraft(
  cur: readonly string[],
  seed: readonly string[],
  incoming: readonly string[]
): readonly string[] {
  if (!sameEntries(cur, seed)) return cur; // 用户改过 → 不打断
  if (sameEntries(cur, incoming)) return cur; // 内容已一致 → 别换引用
  return incoming;
}
