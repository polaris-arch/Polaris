/**
 * ListEditor 的 draft + onBlur —— 守「逐键落盘」这个缺陷的根因。
 *
 * # 缺陷
 *
 * 旧实现的行输入是受控 `onChange` **立刻回调父级**：`update(idx,v) → onChange(copy)` →
 * `useConfig().update` 漏斗 → `editRoute` → `stage()`。于是**每敲一个字符**跑一遍
 * sanitize+validate、一次原子写盘（tmp+rename）、一次 `broadcast_config_changed` →
 * 一次 `spawn(switch_mode)`。本组件是 7 个挂点的共同源头（`bypassLANList` ×2 /
 * `tunConfig` ×3 / `dnsConfig` / `fakeIpFilterList`，全部落在 29 个核心键内 ⇒ 真进暂存）。
 *
 * `SettingsDns.tsx:235-265` 的三个文本框当初正是这个毛病、已改成 draft+onBlur（那里的头注写着），
 * 本组件是唯一没跟上的。修法**照抄那一套**，不发明第二套。
 *
 * # 射程：为什么这里只测纯函数
 *
 * 本仓 vitest 跑 `environment: 'node'` 且**刻意不装 jsdom / testing-library**
 * （`initial-tab-first-frame.test.tsx` 头注明写「别为这道门破例」）。组件级门的既有手段是
 * `react-dom/server` 的 SSR —— 而 SSR **不跑 effect、也无从派发 onChange/onBlur 事件**，
 * 所以「敲字符不写盘、blur 才写盘」这条交互**在本仓结构上无法单测**。
 *
 * 故这里的分工是：
 *  - 把最容易写错的一格（外部刷新时草稿取什么值）抽成纯函数 `nextDraft`，在此穷举；
 *  - 用下面那组**接线守卫**钉住「行输入的 onChange 不得碰父级、onBlur 必须提交」这个形态；
 *  - 交互本身如实标注为真机确认项（见交付说明），不造一道跑不到那一步的门。
 */
import { readFileSync } from 'node:fs';
import { describe, it, expect } from 'vitest';
import { nextDraft, sameEntries, type ListEditorProps } from './ListEditor';
import { editNodeResolverInventory } from './settings-dns-logic';
import { listEntryRow, listEntryRows, listEntrySources, nextListEntryDraft, listEntryAcceptance, sameListEntryRows, type ListEntryRow } from '@/domain/list-entries';

describe('sameEntries', () => {
  it('长度/顺序/内容任一不同即不相同', () => {
    expect(sameEntries([], [])).toBe(true);
    expect(sameEntries(['a', 'b'], ['a', 'b'])).toBe(true);
    expect(sameEntries(['a'], ['a', 'b'])).toBe(false);
    expect(sameEntries(['a', 'b'], ['b', 'a'])).toBe(false);
    expect(sameEntries(['a'], ['A'])).toBe(false); // 大小写敏感：这是用户输入的原文，不是去重键
  });
});

describe('nextDraft —— 外部改动到达时草稿取什么值', () => {
  // 变异对照：把「草稿≠种子 → 保留草稿」那条删掉 → 本条转红。这是最坏的一种失效：
  // 用户正在敲第三个 CIDR，后台写盘/托盘改动一到，输入框被抹回磁盘值。
  it('用户已改过草稿 ⇒ 外部刷新不得打断（保留草稿）', () => {
    const seed = ['10.0.0.0/8'];
    const cur = ['10.0.0.0/8', '192.168.'];
    expect(nextDraft(cur, seed, ['10.0.0.0/8', '172.16.0.0/12'])).toBe(cur);
  });

  // 变异对照：把这条改成恒保留草稿 → 转红。托盘/备份恢复/另一屏保存的改动就永远回填不进来，
  // 用户看到的是一份陈旧列表（且他一保存就把别人的改动覆盖掉）。
  it('用户没动过草稿 ⇒ 跟随新配置', () => {
    const seed = ['10.0.0.0/8'];
    const incoming = ['10.0.0.0/8', '172.16.0.0/12'];
    expect(nextDraft(seed, seed, incoming)).toBe(incoming);
  });

  // props 身份易变（父级常常每次重渲都新建数组）。内容已相同就必须返回原引用，
  // 否则每次父级重渲都白多一次 setState → 重渲。变异对照：去掉这条早退 → 本条转红。
  it('内容已一致 ⇒ 返回原引用（不为身份变化白重渲）', () => {
    const cur = ['10.0.0.0/8'];
    expect(nextDraft(cur, cur, ['10.0.0.0/8'])).toBe(cur);
  });

  it('空列表两侧与「清空」都不误判', () => {
    const empty: string[] = [];
    expect(nextDraft(empty, empty, [])).toBe(empty);
    const incoming: string[] = [];
    expect(nextDraft(['a'], ['a'], incoming)).toBe(incoming); // 外部把列表清空了，用户没在编辑 → 跟随
  });
});

/**
 * **接线守卫** —— 纯函数对 ≠ 组件真的这么接。交互测不了（见文件头），这组就是替代品：
 * 它钉住的是「打字的那条路上没有父级 onChange」这个**形态**。
 */
describe('接线：行输入打字不得碰父级，blur 才提交', () => {
  /** 去注释后再扫：本文件与组件头注都逐字写着旧形态 `onChange(copy)`，扫原文会被说明文字误伤。 */
  const SRC = readFileSync(new URL('./ListEditor.tsx', import.meta.url), 'utf8')
    .replace(/\/\*[\s\S]*?\*\//g, '')
    .replace(/^[ \t]*\/\/.*$/gm, '');

  // 变异对照：把行 onChange 改回 `editRow` → `onChange([...])`（直接回调父级）→ 本条转红。
  it('打字只走 editRow（只动草稿）', () => {
    expect(SRC).toContain('onChange={(e) => editRow(idx, e.target.value)}');
  });

  it('行输入挂了 onBlur 提交 + Enter 触发 blur', () => {
    expect(SRC).toContain('onBlur={() => commit(draft)}');
    expect(SRC).toContain("if (e.key === 'Enter') e.currentTarget.blur();");
  });

  // `editRow` 是唯一不提交的写入口；它绝不能调 commit/onChange，否则等于没改。
  it('editRow 内不得出现 commit / onChange', () => {
    const body = SRC.slice(SRC.indexOf('function editRow('));
    const end = body.indexOf('\n  }');
    const editRowBody = body.slice(0, end);
    expect(editRowBody).not.toContain('commit(');
    expect(editRowBody).not.toContain('onChange(');
  });

  // 离散动作（删/加/导入）必须基于**草稿**：基于 `value` 会丢掉另一行里还没提交的编辑。
  // 变异对照：把任一处的 `draft` 换回 `value` → 本条转红。
  it('删除 / 添加 / 批量导入都基于 draft，不基于 value', () => {
    expect(SRC).toContain('commit(draft.filter((_, i) => i !== idx))');
    expect(SRC).toContain("commit([...draft, listEntryRow('')])");
    expect(SRC).toContain('importListEntryRows(importDraft, draft, max, entryIdentity)');
    // 渲染也必须走草稿，否则打字看不见（草稿变了但画的是 value）。
    expect(SRC).toContain('{draft.map((entry, idx) => (');
  });
});

describe('identified editor acceptance composed with real inventory transactions', () => {
  const one = 'tls://1.1.1.1';
  const nine = 'tls://9.9.9.9';
  const eight = 'tls://8.8.8.8';
  const seedRows = () => listEntryRows([one, nine], ['a', 'b']);
  const inventory = (rows: readonly ListEntryRow[]) => rows.map(row => ({ id: row.id!, spec: row.value }));
  function transactions() {
    let minted = 0;
    function save(previous: ReturnType<typeof inventory>, pool: readonly string[], rows: readonly ListEntryRow[], context?: unknown) {
      const result = editNodeResolverInventory(previous, pool, rows.map(row => ({ id: row.id, spec: row.value })), () => `new-${++minted}`, { sources: listEntrySources(rows), context });
      if (!result.ok) throw new Error(`unexpected ${result.error}`);
      const receipt = listEntryAcceptance(result.editor, rows);
      if (!receipt) throw new Error('missing complete synchronous receipt');
      return { ...result, receipt, incoming: listEntryRows(result.custom.map(row => row.spec), result.custom.map(row => row.id)) };
    }
    return { save, minted: () => minted };
  }

  it('preserves every old string-only dirty-draft transition without opting in', () => {
    for (const [current, seed, incoming] of [
      [['a'], ['a'], ['b']], [['dirty'], ['a'], ['b']],
      [[], [], ['new']], [['dirty'], ['a'], []], [['a'], ['a'], ['a']],
    ]) {
      const actual = nextListEntryDraft(listEntryRows(current), listEntryRows(seed), listEntryRows(incoming));
      expect(actual.map(row => row.value)).toEqual(nextDraft(current, seed, incoming));
      expect(actual.every(row => row.id === undefined)).toBe(true);
    }
  });

  it.each([[], ['a'], ['b'], ['b', 'ali', 'a']].map(pool => ({ pool })))('rebases a retired alias and preserves selected source order: $pool', ({ pool }) => {
    const tx = transactions();
    const seed = seedRows();
    const submitted = seed.map(row => row.id === 'b' ? { ...row, value: one } : row);
    const first = tx.save(inventory(seed), pool, submitted);
    const newer = submitted.map(row => row.id === 'b' ? { ...row, value: eight } : row);
    // Old/unrelated props cannot acknowledge this transaction.
    expect(nextListEntryDraft(newer, seed, seed, first.receipt)).toBe(newer);
    const unrelated = listEntryRows(['udp://8.8.4.4'], ['other']);
    expect(nextListEntryDraft(newer, seed, unrelated, first.receipt)).toBe(newer);
    const rebased = nextListEntryDraft(newer, seed, first.incoming, first.receipt);
    expect(rebased.map(row => [row.value, row.id, row.key])).toEqual([
      [one, 'a', seed[0].key], [eight, undefined, seed[1].key],
    ]);
    const second = tx.save(first.custom, first.pool, rebased, first.receipt.context);
    expect(second.custom).toEqual([{ id: 'a', spec: one }, { id: 'new-1', spec: eight }]);
    expect(second.pool).toEqual(pool.map(id => id === 'b' ? 'new-1' : id));
    expect(editNodeResolverInventory(first.custom, first.pool, newer.map(row => ({ id: row.id, spec: row.value })), () => 'bad')).toEqual({ ok: false, error: 'invalid' });
    const finalDraft = nextListEntryDraft(rebased, first.incoming, second.incoming, second.receipt);
    const reload = tx.save(second.custom, second.pool, finalDraft, second.receipt.context);
    expect(reload.custom).toEqual(second.custom);
    expect(reload.pool).toEqual(second.pool);
    expect(tx.minted()).toBe(1);
  });

  it.each(['a', 'both'])('keeps the unchanged alias endpoint when the primary is edited: %s', edited => {
    const tx = transactions(); const seed = seedRows();
    const submitted = seed.map(row => ({ ...row, value: one }));
    const first = tx.save(inventory(seed), ['a', 'b'], submitted);
    const newer = submitted.map(row => row.id === 'a' ? { ...row, value: eight } : edited === 'both' ? { ...row, value: nine } : row);
    const rebased = nextListEntryDraft(newer, seed, first.incoming, first.receipt);
    expect(rebased.map(row => row.value)).toEqual([eight, edited === 'both' ? nine : one]);
    expect(rebased.map(row => row.key)).toEqual(seed.map(row => row.key));
    expect(new Set(rebased.map(row => row.id)).size).toBe(2);
    const second = tx.save(first.custom, first.pool, rebased, first.receipt.context);
    expect(second.pool.map(id => second.custom.find(row => row.id === id)!.spec)).toEqual([eight, edited === 'both' ? nine : one]);
    expect(second.custom).toHaveLength(2);
  });

  it('adopts a newly minted ID despite later editing without minting it again', () => {
    const tx = transactions(); const seed = listEntryRows([one], ['a']);
    const added = [...seed, listEntryRow(nine)];
    const first = tx.save(inventory(seed), ['a'], added);
    const newer = added.map((row, index) => index === 1 ? { ...row, value: eight } : row);
    const rebased = nextListEntryDraft(newer, seed, first.incoming, first.receipt);
    expect(rebased[1]).toMatchObject({ value: eight, id: 'new-1', key: added[1].key });
    const second = tx.save(first.custom, first.pool, rebased, first.receipt.context);
    expect(second.custom[1]).toEqual({ id: 'new-1', spec: eight });
    expect(tx.minted()).toBe(1);
  });

  it.each(['a', 'b', 'all'].flatMap(removed => [[], ['a'], ['b'], ['a', 'b']].map(pool => ({ removed, pool }))))('does not resurrect removed sources or transfer their selection: $removed / $pool', ({ removed, pool }) => {
    const tx = transactions(); const seed = seedRows();
    const submitted = seed.map(row => ({ ...row, value: one }));
    const first = tx.save(inventory(seed), pool, submitted);
    const newer = submitted.filter(row => removed !== 'all' && row.id !== removed);
    const rebased = nextListEntryDraft(newer, seed, first.incoming, first.receipt);
    const second = tx.save(first.custom, first.pool, rebased, first.receipt.context);
    expect(second.custom).toHaveLength(removed === 'all' ? 0 : 1);
    expect(second.pool).toEqual(pool.some(id => removed !== 'all' && id !== removed) ? ['a'] : []);
    expect(rebased.map(row => row.key)).toEqual(newer.map(row => row.key));
  });

  it('preserves reorder, dirty fork, post-submit additions and original pool order together', () => {
    const tx = transactions(); const seed = seedRows();
    const submitted = seed.map(row => ({ ...row, value: one }));
    const first = tx.save(inventory(seed), ['a', 'b'], submitted);
    const extra = listEntryRow('udp://8.8.4.4');
    const newer = [{ ...submitted[1], value: eight }, extra, submitted[0]];
    const rebased = nextListEntryDraft(newer, seed, first.incoming, first.receipt);
    expect(rebased.map(row => row.key)).toEqual(newer.map(row => row.key));
    const second = tx.save(first.custom, first.pool, rebased, first.receipt.context);
    expect(second.custom.map(row => row.spec)).toEqual([eight, 'udp://8.8.4.4', one]);
    expect(second.pool.map(id => second.custom.find(row => row.id === id)!.spec)).toEqual([one, eight]);
  });

  it('carries selection through a fully collapsed alias into another real edit', () => {
    const tx = transactions(); const seed = seedRows();
    const submitted = seed.map(row => ({ ...row, value: one }));
    const first = tx.save(inventory(seed), ['b'], submitted);
    const collapsed = nextListEntryDraft(submitted, seed, first.incoming, first.receipt);
    expect(collapsed).toHaveLength(1);
    expect(collapsed[0].origins).toEqual(seed.map(row => row.key));
    const second = tx.save(first.custom, first.pool, [{ ...collapsed[0], value: eight }], first.receipt.context);
    expect(second.pool).toEqual(['a']);
    expect(second.custom).toEqual([{ id: 'a', spec: eight }]);
  });

  it('a later receipt survives earlier props, and the next real save uses its identities and selection', () => {
    const tx = transactions(); const seed = seedRows();
    const submitted = seed.map(row => ({ ...row, value: one }));
    const older = tx.save(inventory(seed), ['b'], submitted);
    const newerSubmit = submitted.map(row => row.id === 'a' ? { ...row, value: eight } : row);
    const latest = tx.save(inventory(seed), ['b'], newerSubmit);
    const dirty = newerSubmit.map(row => row.id === 'b' ? { ...row, value: nine } : row);
    expect(nextListEntryDraft(dirty, seed, older.incoming, latest.receipt)).toBe(dirty);
    const rebased = nextListEntryDraft(dirty, older.incoming, latest.incoming, latest.receipt);
    const next = tx.save(latest.custom, latest.pool, rebased, latest.receipt.context);
    expect(next.custom).toEqual([{ id: 'a', spec: eight }, { id: 'b', spec: nine }]);
    expect(next.pool).toEqual(['b']);
  });

  it('ignores a stale selection context after an explicit pool change and keeps quota atomic', () => {
    const tx = transactions(); const seed = seedRows();
    const submitted = seed.map(row => ({ ...row, value: one }));
    const first = tx.save(inventory(seed), ['b'], submitted);
    const newer = submitted.map(row => row.id === 'b' ? { ...row, value: eight } : row);
    const rebased = nextListEntryDraft(newer, seed, first.incoming, first.receipt);
    expect(tx.save(first.custom, ['a', 'ali'], rebased, first.receipt.context).pool).toEqual(['a', 'ali']);
    const enabled = tx.save(inventory(seed), ['ali', 'dnspod', 'a', 'b'], submitted);
    const before = JSON.stringify(enabled);
    const split = nextListEntryDraft(newer, seed, enabled.incoming, enabled.receipt);
    expect(editNodeResolverInventory(enabled.custom, enabled.pool, split.map(row => ({ id: row.id, spec: row.value })), () => 'fork', { sources: listEntrySources(split), context: enabled.receipt.context })).toEqual({ ok: false, error: 'quota' });
    expect(JSON.stringify(enabled)).toBe(before);
  });

  it('the identified transaction keeps builtin-first malformed-inventory and invalid-selected guards', () => {
    const tx = transactions();
    const previous = [{ id: 'ali', spec: 'tls://resolver.example' }, { id: 'bad', spec: 'tls://other.example' }];
    const rows = [...listEntryRows(previous.map(row => row.spec), previous.map(row => row.id)), listEntryRow('udp://8.8.4.4')];
    const accepted = tx.save(previous, ['ali'], rows);
    expect(accepted.pool).toEqual(['ali']);
    expect(accepted.custom.slice(0, 2)).toEqual(previous);
    expect(editNodeResolverInventory(previous, ['bad'], rows.map(row => ({ id: row.id, spec: row.value })), () => 'new', { sources: listEntrySources(rows) })).toEqual({ ok: false, error: 'invalid' });
  });

  it('Add blank → fill duplicate → blur → same-seed adoption closes only the nonblank alias', () => {
    const tx = transactions(); const seed = listEntryRows([one], ['a']);
    const added = [...seed, listEntryRow('')];
    const blank = tx.save(inventory(seed), ['a'], added);
    const editable = nextListEntryDraft(added, seed, seed, blank.receipt);
    expect(editable.map(row => row.value)).toEqual([one, '']);
    expect(editable[1].key).toBe(added[1].key);
    const filled = editable.map((row, index) => index === 1 ? { ...row, value: one } : row);
    const duplicate = tx.save(blank.custom, blank.pool, filled, blank.receipt.context);
    expect(sameListEntryRows(duplicate.incoming, seed)).toBe(true);
    const adopted = nextListEntryDraft(filled, seed, seed, duplicate.receipt);
    expect(adopted.map(row => [row.value, row.id])).toEqual([[one, 'a']]);
    expect(adopted[0].key).toBe(seed[0].key);
    expect(tx.save(duplicate.custom, duplicate.pool, adopted, duplicate.receipt.context).pool).toEqual(['a']);
    expect(tx.minted()).toBe(0);
  });

  it('keeps a new value typed into an actual blank while its no-op receipt arrives', () => {
    const tx = transactions(); const seed = listEntryRows([one], ['a']);
    const added = [...seed, listEntryRow('')];
    const first = tx.save(inventory(seed), ['a'], added);
    const newer = added.map((row, index) => index === 1 ? { ...row, value: nine } : row);
    const rebased = nextListEntryDraft(newer, seed, seed, first.receipt);
    expect(tx.save(first.custom, first.pool, rebased, first.receipt.context).custom[1].spec).toBe(nine);
  });

  it('rejects incomplete/malformed receipts and preserves legacy void/async contracts', () => {
    const legacy: ListEditorProps['onChange'] = (_next: string[]) => undefined;
    const asyncLegacy: ListEditorProps['onChange'] = async (_next: string[]) => undefined;
    const rows = listEntryRows(['value']); const sources = listEntrySources(rows);
    for (const result of [legacy(['value'], [undefined], sources), asyncLegacy(['value'], [undefined], sources), false,
      { acceptedRows: [] }, { acceptedRows: [], acceptedSources: [] },
      { acceptedRows: [], acceptedSources: [{ key: rows[0].key, id: 'missing', primary: true }] }]) {
      expect(listEntryAcceptance(result, rows)).toBeUndefined();
    }
  });

  it('both editors wire source bindings, same-seed adoption and external selection invalidation identically', () => {
    for (const path of ['./ListEditor.tsx', '../../../mobile/settings/SettingsChrome.tsx']) {
      const source = readFileSync(new URL(path, import.meta.url), 'utf8');
      expect(source).toContain('next.map((row) => row.id), listEntrySources(next), continuationRef.current');
      expect(source).toContain('{ ...row, value: next }');
      expect(source).toContain('if (entryIds === undefined) return;');
      expect(source).toContain('listEntryAcceptance(result, submitted)');
      expect(source).toContain('if (!accepted) return;');
      expect(source).toContain('sameListEntryRows(accepted.canonical, incoming)');
      expect(source).toContain('nextListEntryDraft(cur, incoming, incoming, accepted)');
      expect(source).toContain('nextListEntryDraft(cur, seed, incoming, accepted)');
      expect(source).toContain('accepted.contextKey === entryContextKey ? accepted.context : undefined');
      expect(source).toContain('contextKeyRef.current !== entryContextKey');
      expect(source).toContain('accepted.contextKey !== entryContextKey');
      expect(source).toMatch(/key=\{entryIds === undefined \? (?:idx|index) : entry.key\}/);
      expect(source).not.toContain('await onChange');
      expect(source).not.toContain('.then(');
    }
  });
});
