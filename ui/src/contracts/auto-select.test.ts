/**
 * 自动选择合同的跨语言对拍：`auto-select.ts` 里的取值表 ⇄ 后端 `runtime/auto_select.rs` 实际
 * 发出的串；意图键的形状 ⇄ 存储层的规范形；命令名与事件名 ⇄ 后端注册的那一份。
 *
 * 两侧任一侧加、减、改一个取值而另一侧不动，界面会对着一个不认识的原因显示空白，或者对着一个
 * 再也不会出现的原因留着文案。这里逐字对拍，改一侧即红。
 */
import { readdirSync, readFileSync, statSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import { parse } from '@babel/parser';
import { IPC_CHANNELS } from '../domain/ipc-channels';
import {
  AUTO_SELECT_CAUSES,
  AUTO_SELECT_COMMIT_FAILED_REASONS,
  AUTO_SELECT_ERROR_CODES,
  AUTO_SELECT_HOLD_REASONS,
  AUTO_SELECT_LACKING,
  AUTO_SELECT_MODES,
  AUTO_SELECT_NO_DATA_REASONS,
  AUTO_SELECT_NOT_EVALUATED_REASONS,
} from './auto-select';
import { moduleSource } from './rust-source.test-support';

const RUST = moduleSource('src-tauri/src/runtime/auto_select');

/** `impl <name> { … fn as_str … }` 里各臂给出的串，按源码顺序。 */
function asStrValues(name: string): string[] {
  const start = RUST.indexOf(`impl ${name} {`);
  if (start < 0) throw new Error(`找不到 impl ${name}：取材锚点失效`);
  const body = RUST.slice(start, RUST.indexOf('\n}\n', start));
  const arms = [...body.matchAll(/=> "([A-Za-z]+)"/g)].map((match) => match[1]);
  if (arms.length === 0) throw new Error(`impl ${name} 里一个取值都没抠到`);
  return arms;
}

describe('自动选择合同与后端一致', () => {
  it('未评估的原因 = 闸门的全部取值，去掉两个不以原因出现的', () => {
    // `manual` 是模式，不是原因；`waitingFirstRound` 单独成一个模式。`notYetEvaluated` 由状态
    // 投影给出（意图刚写入、还没有评估过），不是闸门。
    const gates = asStrValues('Gate').filter(
      (gate) => gate !== 'manual' && gate !== 'waitingFirstRound',
    );
    expect([...gates, 'notYetEvaluated'].sort()).toEqual(
      [...AUTO_SELECT_NOT_EVALUATED_REASONS].sort(),
    );
    expect(RUST).toContain('Some("notYetEvaluated")');
  });

  it('已选定之下不换的原因 = Hold 自己给出的取值，去掉两个单独成模式的', () => {
    const holds = asStrValues('Hold').filter(
      (hold) => !['noCandidates', 'needsRestart'].includes(hold),
    );
    expect(holds.sort()).toEqual([...AUTO_SELECT_HOLD_REASONS].sort());
  });

  it('没有数据的原因 = 当前出口没有结果的各子类 + 周期测速被挡住的各原因', () => {
    expect([...asStrValues('NoResult'), ...asStrValues('Starved')].sort()).toEqual(
      [...AUTO_SELECT_NO_DATA_REASONS].sort(),
    );
  });

  it('提交失败的原因 = 没换成的各原因 + 等待重试', () => {
    expect([...asStrValues('CommitFailure'), 'retryPending'].sort()).toEqual(
      [...AUTO_SELECT_COMMIT_FAILED_REASONS].sort(),
    );
    expect(RUST).toContain('Some("retryPending")');
  });

  it('换点原因、欠缺项逐字相同', () => {
    expect(asStrValues('Cause').sort()).toEqual([...AUTO_SELECT_CAUSES].sort());
    expect(asStrValues('Lacking').sort()).toEqual([...AUTO_SELECT_LACKING].sort());
  });

  it('模式的每个取值都在状态投影里出现，投影给出的模式也都在表里', () => {
    const projection = RUST.slice(RUST.indexOf('pub(crate) fn project_status('));
    for (const mode of AUTO_SELECT_MODES) {
      expect(projection, mode).toContain(`"${mode}"`);
    }
    // 反向：「模式, 原因」那个 match 里每个二元组的第一项。
    const table = projection.slice(projection.indexOf('let (mode, reason) = match &decision {'));
    const emitted = new Set(
      [...table.slice(0, table.indexOf('\n    };')).matchAll(/\("([A-Za-z]+)",\s*(?:Some|None)/g)].map(
        (match) => match[1],
      ),
    );
    expect(emitted.size).toBeGreaterThanOrEqual(8);
    emitted.add('manual');
    expect([...emitted].sort()).toEqual([...AUTO_SELECT_MODES].sort());
  });

  it('被拒的稳定码逐字相同', () => {
    const server = moduleSource('src-tauri/src/commands/server');
    const codes = [...server.matchAll(/"(AUTO_SELECT_[A-Z_]+)"/g)].map((match) => match[1]);
    expect([...new Set(codes)].sort()).toEqual([...AUTO_SELECT_ERROR_CODES].sort());
  });

  it('意图键的规范形与存储层一致', () => {
    const store = moduleSource('crates/store/src/sanitize');
    expect(store).toContain('pub const SELECTION_INTENT_KEY: &str = "selectionIntent";');
    const canonical = store.slice(store.indexOf('pub fn selection_intent_auto('));
    for (const field of ['"mode": "auto"', '"scope": "subscription"', '"subscriptionId": ']) {
      expect(canonical.slice(0, canonical.indexOf('\n}\n')), field).toContain(field);
    }
  });

  it('命令名与事件名在后端都有注册', () => {
    const lib = readFileSync(
      fileURLToPath(new URL('../../../src-tauri/src/lib.rs', import.meta.url)),
      'utf8',
    );
    const server = moduleSource('src-tauri/src/commands/server');
    for (const command of [
      IPC_CHANNELS.AUTO_SELECT_ENABLE,
      IPC_CHANNELS.AUTO_SELECT_SWITCH_NOW,
      IPC_CHANNELS.AUTO_SELECT_STATUS,
    ]) {
      expect(lib, command).toMatch(new RegExp(`\\n\\s+${command},\\n`));
      expect(server, command).toMatch(new RegExp(`pub (async )?fn ${command}\\(`));
    }
    const events = moduleSource('src-tauri/src/events');
    expect(events).toContain(
      `pub const EVENT_AUTO_SELECT_STATUS: &str = "${IPC_CHANNELS.EVENT_AUTO_SELECT_STATUS}";`,
    );
  });
});

/**
 * 界面写实际出口与选择意图的位置。
 *
 * 用户选出口只有一条路：`api.server.switch`（后端在同一次写里把意图清回手动）。界面源码里
 * 凡是把 `selectedServerId` 写进一份要提交的配置的地方，都是另一条能改出口而不经那条路的
 * 路径 —— 后端把它一律当作系统代选（不清意图）。所以每一处都要登记并说明它为什么不是用户在
 * 选出口；新增一处而不登记即红。意图键界面一处都不许写。
 */
describe('界面写出口与意图的位置逐处登记', () => {
  const SRC = fileURLToPath(new URL('..', import.meta.url));
  /** 文件 → 写入处数 → 说明。 */
  const REGISTERED: Record<string, [number, string]> = {
    'components/screens/nodes/NodesScreen.tsx': [
      1,
      '传给节点删除hook的只读参数对象，字段简写也登记；不提交配置',
    ],
    'lib/staged-config.ts': [
      1,
      '暂存区删掉了当前出口节点：随保存一起提交的兜底出口。用户删的是节点，不是在选出口',
    ],
    'store/app-store.ts': [
      6,
      'store 内存态的初值、重置归零及四处配置投影；全部登记，不以右值读取同名字段为由豁免写入',
    ],
  };

  function sources(dir: string, out: string[] = []): string[] {
    for (const entry of readdirSync(dir).sort()) {
      const full = join(dir, entry);
      if (statSync(full).isDirectory()) sources(full, out);
      else if (/\.tsx?$/.test(entry) && !/\.(test|spec)\.|test-support|\.fixture\./.test(entry)) {
        out.push(full);
      }
    }
    return out;
  }

  /** Parse object construction and mutation targets; type declarations and destructuring reads do not write.
   * The compiler package exposes no AST API, so the parser is a test-only dependency.
   */
  function writes(source: string, tsx = false, field = 'selectedServerId'): number {
    const ast = parse(source, { sourceType: 'module', plugins: tsx ? ['typescript', 'jsx'] : ['typescript'] });
    let count = 0;
    function named(key: Record<string, unknown>, computed: unknown): boolean {
      if (['TSAsExpression', 'TSTypeAssertion', 'TSNonNullExpression'].includes(String(key.type))) return named(key.expression as Record<string, unknown>, computed);
      if (key.type === 'TemplateLiteral' && (key.expressions as unknown[]).length === 0) {
        const value = ((key.quasis as Record<string, unknown>[])[0].value as Record<string, unknown>);
        return (value.cooked ?? value.raw) === field;
      }
      return (!computed && key.type === 'Identifier' && key.name === field)
        || (key.type === 'StringLiteral' && key.value === field);
    }
    function target(value: unknown): number {
      if (!value || typeof value !== 'object') return 0;
      const node = value as Record<string, unknown>;
      switch (node.type) {
        case 'MemberExpression':
        case 'OptionalMemberExpression':
          return Number(named(node.property as Record<string, unknown>, node.computed));
        case 'ObjectPattern':
          return (node.properties as Record<string, unknown>[]).reduce((n, property) => n + target(property.type === 'RestElement' ? property.argument : property.value), 0);
        case 'ArrayPattern':
          return (node.elements as unknown[]).reduce<number>((n, element) => n + target(element), 0);
        case 'AssignmentPattern': return target(node.left);
        case 'RestElement': return target(node.argument);
        case 'TSAsExpression':
        case 'TSTypeAssertion':
        case 'TSNonNullExpression': return target(node.expression);
        default: return 0;
      }
    }
    function visit(value: unknown, parent?: Record<string, unknown>) {
      if (Array.isArray(value)) { for (const child of value) visit(child, parent); return; }
      if (!value || typeof value !== 'object') return;
      const node = value as Record<string, unknown>;
      if (typeof node.type !== 'string') return;
      if ((node.type === 'ObjectProperty' || node.type === 'ObjectMethod') && parent?.type === 'ObjectExpression') {
        const key = node.key as Record<string, unknown>;
        if (named(key, node.computed)) count += 1;
      }
      if (node.type === 'AssignmentExpression' || node.type === 'ForInStatement' || node.type === 'ForOfStatement') count += target(node.left);
      if (node.type === 'UpdateExpression' || (node.type === 'UnaryExpression' && node.operator === 'delete')) count += target(node.argument);
      for (const child of Object.values(node)) visit(child, node);
    }
    visit(ast);
    return count;
  }

  it('写出口的位置与登记表逐文件相同', () => {
    const files = sources(SRC);
    expect(files.length).toBeGreaterThan(400);
    const found: Record<string, number> = {};
    for (const file of files) {
      const count = writes(readFileSync(file, 'utf8'), file.endsWith('.tsx'));
      if (count > 0) found[file.slice(SRC.length).replace(/\\/g, '/')] = count;
    }
    expect(found).toEqual(
      Object.fromEntries(Object.entries(REGISTERED).map(([file, [count]]) => [file, count])),
    );
  });

  it('分类器认得出写与读', () => {
    expect(writes('const next = { ...cfg, selectedServerId: fallback ?? DIRECT };')).toBe(1);
    expect(writes("patch({ selectedServerId: 'n1' })")).toBe(1);
    expect(writes('interface X { selectedServerId: string | null; }')).toBe(0);
    expect(writes('set({ selectedServerId: config.selectedServerId })')).toBe(1);
    expect(writes('patch({ ...cfg, selectedServerId })')).toBe(1);
    expect(writes('const { selectedServerId } = cfg;')).toBe(0);
    expect(writes('patch({ ["selectedServerId"]: next })')).toBe(1);
    expect(writes('const text = "{ selectedServerId: 1 }";')).toBe(0);
    expect(writes('const node = <View selectedServerId={selectedServerId} />;', true)).toBe(0);
    expect(writes('// { selectedServerId: 1 }')).toBe(0);
    for (const field of ['selectedServerId', 'selectionIntent']) {
      for (const code of [`cfg.${field} = next;`, `cfg.${field} ??= next;`, `delete cfg.${field};`, `cfg["${field}"] = next;`, `({ value: cfg.${field} } = next);`, `cfg.${field}++;`, `save({ ${field} });`, `save({ ["${field}"]: next });`]) {
        expect(writes(code, false, field), code).toBe(1);
      }
      expect(writes(`delete cfg[\`${field}\`];`, false, field)).toBe(1);
      expect(writes(`save({ ["${field}" as const]: next });`, false, field)).toBe(1);
      expect(writes(`save({ get ${field}() { return next; } });`, false, field)).toBe(1);
      for (const code of [`const value = cfg.${field};`, `const { ${field} } = cfg;`, `interface Config { ${field}: string; }`]) expect(writes(code, false, field), code).toBe(0);
    }
  });

  it('键名以字符串形态出现的位置逐文件登记', () => {
    // `setValue('selectedServerId', …)`、`patch['selectedServerId'] = …` 这类按键名写值的形态
    // 不是对象字面量，上一条数不到。带引号的键名在界面里每多出现一处，都要先看它是不是一次写。
    const QUOTED: Record<string, [number, string]> = {
      'contracts/user-config-fields.ts': [1, 'UserConfig 的字段名表（与 Rust 侧对拍用）'],
      'lib/staged-config.ts': [1, '暂存条目的 configKey：标识「切换节点」这一类条目，不写值'],
    };
    const found: Record<string, number> = {};
    for (const file of sources(SRC)) {
      const code = readFileSync(file, 'utf8')
        .replace(/\/\*[\s\S]*?\*\//g, '')
        .replace(/^\s*\/\/.*$/gm, '');
      const count = [...code.matchAll(/['"`]selectedServerId['"`]/g)].length;
      if (count > 0) found[file.slice(SRC.length).replace(/\\/g, '/')] = count;
    }
    expect(found).toEqual(
      Object.fromEntries(Object.entries(QUOTED).map(([file, [count]]) => [file, count])),
    );
  });

  it('界面不写选择意图，也不绕过封装直接调意图命令', () => {
    for (const file of sources(SRC)) {
      const relative = file.slice(SRC.length).replace(/\\/g, '/');
      const code = readFileSync(file, 'utf8')
        .replace(/\/\*[\s\S]*?\*\//g, '')
        .replace(/^\s*\/\/.*$/gm, '');
      expect(writes(code, file.endsWith('.tsx'), 'selectionIntent'), relative).toBe(0);
      if (relative !== 'ipc/api/servers.ts' && relative !== 'domain/ipc-channels.ts') {
        expect(code, relative).not.toMatch(/AUTO_SELECT_(ENABLE|SWITCH_NOW|STATUS)\b/);
      }
    }
  });
});
