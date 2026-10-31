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
export function parseBulkEntries(draft: string, existing: string[], max?: number, entryIdentity: (value: string) => string = (value) => value.trim().toLowerCase()): string[] {
  const next = [...existing];
  // 去重基准里剔掉空串：既有的空行不该让粘贴内容里的第一个条目被误判成重复。
  const seen = new Set(next.map(entryIdentity).filter(Boolean));
  for (const raw of draft.split(/[,\n]/)) {
    const entry = raw.trim();
    if (!entry) continue;
    const key = entryIdentity(entry);
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

/** Transient editor identity is separate from a persisted inventory ID. */
export interface ListEntryValue { value: string; id?: string }
export interface ListEntryRow extends ListEntryValue { key: string; origins?: readonly string[] }
export interface ListEntrySource { key: string; origins: readonly string[] }
export interface ListEntryBinding { key: string; id?: string; primary: boolean }
export interface ListEntryCommitResult {
  acceptedRows: readonly ListEntryValue[];
  acceptedSources: readonly ListEntryBinding[];
  context?: unknown;
  contextKey?: string;
}
export interface ListEntryAcceptance {
  submitted: readonly ListEntryRow[];
  canonical: readonly ListEntryValue[];
  bindings: readonly ListEntryBinding[];
  context?: unknown;
  contextKey?: string;
}

let nextLocalRowKey = 0;
export function listEntryRow(value: string, id?: string): ListEntryRow {
  return { value, id, key: id === undefined ? `new:${++nextLocalRowKey}` : `id:${id}` };
}
export function listEntryRows(values: readonly string[], ids?: readonly (string | undefined)[]): ListEntryRow[] {
  return values.map((value, index) => listEntryRow(value, ids?.[index]));
}
export function listEntrySources(rows: readonly ListEntryRow[]): ListEntrySource[] {
  return rows.map((row) => ({ key: row.key, origins: row.origins ?? [row.key] }));
}
export function sameListEntryRows(a: readonly ListEntryValue[], b: readonly ListEntryValue[]): boolean {
  return a.length === b.length && a.every((row, index) => row.value === b[index].value && row.id === b[index].id);
}

/** Use explicit submitted-key bindings, never domain-specific string guesses.
 * Unchanged aliases collapse; newer edits split while retaining their values,
 * local keys and exact source origins for the next inventory transaction. */
function rebaseAcceptedRows(current: readonly ListEntryRow[], accepted: ListEntryAcceptance): readonly ListEntryRow[] {
  const submitted = new Map(accepted.submitted.map((row) => [row.key, row]));
  const bindings = new Map(accepted.bindings.map((row) => [row.key, row]));
  const groups = new Map<string, number[]>();
  const output = new Map<number, ListEntryRow>();
  current.forEach((row, index) => {
    const binding = bindings.get(row.key);
    if (!binding) { output.set(index, row); return; } // Added after submission.
    if (binding.id === undefined) { output.set(index, { ...row, id: undefined, origins: [row.key] }); return; }
    const group = groups.get(binding.id) ?? [];
    group.push(index);
    groups.set(binding.id, group);
  });
  for (const [id, indices] of groups) {
    const unchanged = indices.filter((index) => current[index].value === submitted.get(current[index].key)?.value);
    const candidates = unchanged.length ? unchanged : indices;
    const owner = candidates.find((index) => bindings.get(current[index].key)?.primary) ?? candidates[0];
    const anchor = unchanged.length ? unchanged[0] : owner;
    const row = current[owner];
    const canonical = accepted.canonical.find((item) => item.id === id)!;
    output.set(anchor, { ...row, id, value: unchanged.length ? canonical.value : row.value,
      origins: (unchanged.length ? unchanged : [owner]).map((index) => current[index].key) });
    for (const index of indices) {
      if (index === owner || unchanged.includes(index)) continue;
      output.set(index, { ...current[index], id: undefined, origins: [current[index].key] });
    }
  }
  return [...output].sort(([a], [b]) => a - b).map(([, row]) => row);
}

export function nextListEntryDraft(
  current: readonly ListEntryRow[], seed: readonly ListEntryRow[], incoming: readonly ListEntryRow[],
  accepted?: ListEntryAcceptance,
): readonly ListEntryRow[] {
  // Same-seed nonblank acceptance must also close duplicate rows. A blank's
  // binding has no persisted ID, so rebase keeps that actual placeholder.
  if (accepted && sameListEntryRows(incoming, accepted.canonical)) return rebaseAcceptedRows(current, accepted);
  if (sameListEntryRows(seed, incoming) || !sameListEntryRows(current, seed)) return current;
  return incoming.map((row) => ({ ...row, key: row.id === undefined ? row.key : current.find((old) => old.id === row.id)?.key ?? row.key }));
}

/** Only synchronous, complete source bindings acknowledge an identified edit.
 * Legacy void/async returns never install a delayed callback. */
export function listEntryAcceptance(result: unknown, submitted: readonly ListEntryRow[]): ListEntryAcceptance | undefined {
  if (!result || typeof result !== 'object' || !('acceptedRows' in result) || !Array.isArray(result.acceptedRows)
    || !('acceptedSources' in result) || !Array.isArray(result.acceptedSources)) return undefined;
  if (!result.acceptedRows.every((row: unknown) => row && typeof row === 'object' && 'value' in row && typeof row.value === 'string' && 'id' in row && typeof row.id === 'string')) return undefined;
  const ids = new Set(result.acceptedRows.map((row: ListEntryValue) => row.id));
  const keys = new Set(submitted.map((row) => row.key));
  if (ids.size !== result.acceptedRows.length || keys.size !== submitted.length || result.acceptedSources.length !== submitted.length) return undefined;
  const found = new Set<string>();
  for (const row of result.acceptedSources) {
    if (!row || typeof row.key !== 'string' || found.has(row.key) || !keys.has(row.key)
      || typeof row.primary !== 'boolean' || (row.id !== undefined && !ids.has(row.id))) return undefined;
    found.add(row.key);
  }
  return { submitted, canonical: result.acceptedRows, bindings: result.acceptedSources,
    context: 'context' in result ? result.context : undefined,
    contextKey: 'contextKey' in result && typeof result.contextKey === 'string' ? result.contextKey : undefined };
}

/** Bulk import retains the exact existing row prefix, including transient keys. */
export function importListEntryRows(text: string, existing: readonly ListEntryRow[], max?: number, identity?: (value: string) => string): ListEntryRow[] {
  const values = parseBulkEntries(text, existing.map((row) => row.value), max, identity);
  return [...existing, ...values.slice(existing.length).map((value) => listEntryRow(value))];
}
