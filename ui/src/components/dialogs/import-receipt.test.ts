/**
 * 导入回执接线门（`ImportDialog.handleImport`）。
 *
 * **为什么是接线门而不是纯函数单测。** 这里要防的缺陷是**遗漏**：`handleImport` 成功后直接
 * `close()`，唯一会说话的那条腿是「撞上 WARP / Tailscale 单例槽」，而它今天恒不命中
 * （clash/singbox/xray 三个解析器都跳过 wireguard）⇒ 导入在用户眼里完全静默。
 * 把三选一的判断抽成纯函数再单测，测到的是**方法体**；而缺陷长在**调用点**上 ——
 * 纯函数留着不调用，单测照样全绿。故本门直接读 `ImportDialog.tsx` 的 AST 断言接线。
 *
 * 一批节点只能交一次共享提交，旧直写路径仍只在落盘后发自己的成功回执。
 */

import { describe, expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import * as ts from '@/test/ts-compiler';

const HERE = dirname(fileURLToPath(import.meta.url));
const SRC_PATH = join(HERE, 'ImportDialog.tsx');
const SRC = readFileSync(SRC_PATH, 'utf8');

const SF = ts.parseSourceFile(SRC_PATH, SRC);

/** `handleImport` 的函数体（箭头函数常量声明）。找不到即门失效，直接抛而不是静默放行。 */
const HANDLE_IMPORT = (() => {
  let found: ts.Node | null = null;
  const visit = (n: ts.Node): void => {
    if (
      ts.isVariableDeclaration(n) &&
      ts.isIdentifier(n.name) &&
      n.name.text === 'handleImport' &&
      n.initializer
    ) {
      found = n.initializer;
      return;
    }
    ts.forEachChild(n, visit);
  };
  visit(SF);
  if (!found) throw new Error('ImportDialog.tsx 里找不到 handleImport —— 本门已失效，先修门');
  return found as ts.Node;
})();

function textOf(n: ts.Node): string {
  return SRC.slice(n.getStart(SF), n.getEnd());
}

function collect<T extends ts.Node>(root: ts.Node, pred: (n: ts.Node) => n is T): T[] {
  const out: T[] = [];
  const visit = (n: ts.Node): void => {
    if (pred(n)) out.push(n);
    ts.forEachChild(n, visit);
  };
  visit(root);
  return out;
}

/** 形如 `a.b(...)` 且点分名恰为 `name` 的调用。 */
function callsTo(root: ts.Node, name: string): ts.CallExpression[] {
  return collect(root, (n): n is ts.CallExpression => ts.isCallExpression(n)).filter(
    (c) => textOf(c.expression) === name
  );
}

describe('导入回执接线', () => {
  it('routes a node batch through one shared submission and reserves the local success toast for direct saves', () => {
    const receipts = callsTo(HANDLE_IMPORT, 'toast.success');
    expect(receipts).toHaveLength(1);
    expect(textOf(receipts[0])).toContain('nodes.importOk');
    const direct = collect(HANDLE_IMPORT, ts.isIfStatement)
      .find(node => textOf(node.expression) === '!staged');
    expect(direct && callsTo(direct.thenStatement, 'toast.success')).toEqual(receipts);

    const submissions = callsTo(HANDLE_IMPORT, 'submitEdit');
    expect(submissions.map(textOf)).toEqual(['submitEdit(first, ...related)']);
    expect(textOf(HANDLE_IMPORT)).not.toContain('nodes.importStagedOk');
  });

  it('staged 判据只算一次，两处消费同一个值', () => {
    // `editRoute(...)` 在本函数里只应出现一次并存进 `staged`：算两次的话，
    // 「走了哪条腿」与「回执怎么说」就可能分叉（中途 store 变更即两值不一致）。
    expect(callsTo(HANDLE_IMPORT, 'editRoute').length).toBe(1);
  });

  it('直落盘腿写完必须刷 store（快路径，与其余四个写节点弹窗同形）', () => {
    const bulk = callsTo(HANDLE_IMPORT, 'api.server.addBulk');
    expect(bulk.length).toBe(1);

    // `addBulk` 与 `loadConfig(true)` 必须同属一个 Block —— 只断言「文件里出现过
    // loadConfig」的话，把它挪到 staged 腿里门照样绿。
    let owner: ts.Block | null = null;
    for (let p: ts.Node | undefined = bulk[0]; p; p = p.parent) {
      if (ts.isBlock(p)) {
        owner = p;
        break;
      }
    }
    expect(owner, 'addBulk 应在 else 块内').not.toBeNull();
    const refresh = callsTo(owner as ts.Block, 'loadConfig');
    expect(refresh.length, 'addBulk 之后必须 loadConfig(true) 刷 store').toBe(1);
    expect(textOf(refresh[0].arguments[0])).toBe('true');
  });
});
