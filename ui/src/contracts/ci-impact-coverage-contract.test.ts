/**
 * CI 影响分类器的**完备性门**（门 4，2026-08-30）。
 *
 * # 背景：一道 fail-open 造出的「绿而零信息量」
 *
 * `scripts/classify-ci-impact.mjs` 枚举「哪些路径影响打包」，其余一律默认无影响。实测后果：改动
 * `crates/system-integration/`、`crates/helper-client/`、`crates/stats-engine/`、
 * `src-tauri/src/runtime/` 时分类器输出 `kernel=false platforms=[] preflight=false hasPackage=false`
 * ⇒ `release-risk.yml` 的 preflight / package 两个 job 全 skip ⇒ gate job 的三条断言（`PREFLIGHT_REQUIRED`
 * / `PACKAGE_REQUIRED` 为 false 时）全不生效 ⇒ **required check 绿，但一个字节的打包信息都没有**。
 *
 * 这不是「这次漏了」：每新增一个 crate、每新建一个 `src-tauri/src` 子树，它就永久落在全部打包门
 * 之外，且没有任何东西会提醒。本门把「有没有判过」与「判成什么」拆开：
 *
 *  - 分类器持有两张表（`PACKAGE_IMPACT_SCOPES` / `NO_PACKAGE_IMPACT_SCOPES`），后者带理由；
 *  - 本门按**文件系统实况**枚举 scope，逐个要求它出现在某张表里 —— 新增 crate ⇒ 硬红；
 *  - 登记根第三条腿 `resources/`（2026-08-30 补）：`resources/data/` 的 28 个 `.srs` 入库且被四份 conf
 *    的 `bundle.resources` 带进每个安装包，而分类器此前对 `resources/**` 输出全 false —— 与 `crates/`、
 *    `src-tauri/` 同一个 fail-open。往 `resources/` 加子目录而不登记，现在同样硬红；
 *  - 分类器自身对登记根内的未登记路径 fail-closed（内核门 + 四平台 + `unregisteredScopes` 自曝），
 *    所以「本门没跑」时默认后果是多花钱，不是静默放行。
 *
 * # 这门抓不到什么（如实登记）
 *
 * - **登记根之外**（`ui/`、`scripts/`、`packaging/`、仓库根文件）没有可枚举边界，本门不做完备性
 *   断言；只用两条反向断言兜住其中风险最高的两类（下面第 4、5 条）：打包 workflow 真正 `run:` 的
 *   脚本、verify-packaging 当判据读的源文件。
 * - **判定内容的对错**：本门只保证「有人显式判过并留了理由」，不保证那句理由是对的。
 *   `crates/x/` 被判成不影响打包而其实影响，本门不会红 —— 这是本门的设计边界，不是漏洞：
 *   判定质量由第 4、5 条（按下游**实际读取面**反推）与 review 覆盖。
 * - **scope 内部的更细粒度**：`src-tauri/src/runtime/` 整体判为不影响（`proxy.rs` 的历史单列例外
 *   已随 F8 消失，见分类器登记表本条注释）。若将来又有 runtime 文件成为打包判据源而没重新单列，
 *   只有第 4 条能抓到它（前提是抓取方是 verify-packaging）。
 * - **YAML / JS 注释剥离**是行级的（只丢整行注释），行尾注释仍会进取材面。方向是「多取」，
 *   失败形态是多要求登记一个脚本，而不是漏掉一个。
 */
import { describe, expect, it } from 'vitest';
import { strict as assert, AssertionError } from 'node:assert';
import { mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, statSync, symlinkSync } from 'node:fs';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { fileURLToPath, pathToFileURL } from 'node:url';

import { maskRustComments, maskRustCommentsAndStrings } from './rust-source.test-support';

const REPO_ROOT = fileURLToPath(new URL('../../..', import.meta.url));

type Impact = {
  kernel: boolean;
  platforms: string[];
  preflight: boolean;
  hasPackage: boolean;
  android: boolean;
  unregisteredScopes: string[];
};

type Classifier = {
  classifyImpact: (paths: string[], options?: { forceFull?: boolean }) => Impact;
  classifyNativeImpact: (rawDiff: string) => { nativeRequired: boolean; reason: string };
  scopeOf: (path: string) => string | null;
  isScopeRegistered: (scopeKey: string) => boolean;
  ALL_PACKAGE_PLATFORMS: readonly string[];
  REGISTRY_ROOTS: readonly string[];
  PACKAGE_IMPACT_SCOPES: Record<string, { kernel: boolean; platforms: readonly string[]; why: string }>;
  NO_PACKAGE_IMPACT_SCOPES: Record<string, string>;
  ANDROID_IMPACT_SCOPES: Record<string, { why: string }>;
};

const { appleRequired } = await import(pathToFileURL(join(REPO_ROOT, 'scripts/apple-ci-impact.mjs')).href);

const classifier: Classifier = await import(
  pathToFileURL(join(REPO_ROOT, 'scripts/classify-ci-impact.mjs')).href
);

const {
  classifyImpact,
  classifyNativeImpact,
  scopeOf,
  REGISTRY_ROOTS,
  isScopeRegistered,
  ALL_PACKAGE_PLATFORMS,
  PACKAGE_IMPACT_SCOPES,
  NO_PACKAGE_IMPACT_SCOPES,
  ANDROID_IMPACT_SCOPES,
} = classifier;

/** 目录 → `<path>/`，文件 → `<path>`；与分类器 `scopeOf` 的归一形态一致。 */
function scopeKeys(relDir: string, root = REPO_ROOT): string[] {
  return readdirSync(join(root, relDir), { withFileTypes: true })
    .map((entry) => {
      // Local Tauri builds may leave ignored links to fetched resource directories. Dirent
      // describes the link itself; classify its directory target as the same scope as CI's
      // fetched directory. Unknown linked directories still reach the registration gate.
      const path = join(root, relDir, entry.name);
      const isDirectory = entry.isDirectory() || (entry.isSymbolicLink() && statSync(path).isDirectory());
      return isDirectory ? `${relDir}/${entry.name}/` : `${relDir}/${entry.name}`;
    })
    .sort();
}

/**
 * 文件系统实况：登记根内的全部 scope（`src-tauri/src` 展开一层）。
 *
 * `resources/` 只展开一层：data/ dashboard/ 与四个平台目录各是一个 scope。其中除 data/ 与
 * `.gitkeep` 外全部被 .gitignore（CI 现拉），**在没 fetch 过的机器上根本不在盘上** ⇒ 枚举面
 * 因机器而异。这不削弱本门：枚举到的每一条都必须登记（多枚举 ⇒ 多要求一次登记，方向是严不是松），
 * 而没枚举到的那几条已经预先登记在表里，第 2 条断言照样按分类器实际输出校验它们。
 */
function scopesOnDisk(): string[] {
  const srcTauri = scopeKeys('src-tauri').filter((key) => key !== 'src-tauri/src/');
  return [
    ...REGISTRY_ROOTS.filter((root) => root !== 'src-tauri/').flatMap((root) => scopeKeys(root.slice(0, -1))),
    ...srcTauri,
    ...scopeKeys('src-tauri/src'),
  ].sort();
}

/** scope key → 一条落在它内部的探针路径（用来问分类器「你实际怎么判这块」）。 */
function probeOf(key: string): string {
  return key.endsWith('/') ? `${key}__scope_probe__.rs` : key;
}

function compact(paths: string[]) {
  const { kernel, platforms, preflight, hasPackage } = classifyImpact(paths);
  return { kernel, platforms, preflight, hasPackage };
}

/** 行级剥注释：只丢整行注释（`//` `#` `*` 开头），不动行尾 —— 方向是「宁可多取」。 */
function stripLineComments(source: string, kind: 'js' | 'yaml'): string {
  const isComment =
    kind === 'js'
      ? (line: string) => /^\s*(\/\/|\/\*|\*)/.test(line)
      : (line: string) => /^\s*#/.test(line);
  return source
    .split('\n')
    .map((line) => (isComment(line) ? '' : line))
    .join('\n');
}

// These views serve only the finite runner owners below, not a JavaScript AST.
// Both views preserve UTF-16 offsets; literal contents cannot provide code anchors.
function runnerViews(source: string): { visible: string; code: string } {
  const visible = source.split(''), code = source.split('');
  const blank = (out: string[], start: number, end: number) => {
    for (let i = start; i < end; i++) if (out[i] !== '\n' && out[i] !== '\r') out[i] = ' ';
  };
  const regexStart = (at: number): boolean => {
    const left = source.slice(0, at).trimEnd();
    return !left || /[=(:,\[!&|?{};]$/.test(left) || /\b(?:return|throw|case)$/.test(left);
  };
  const regexEnd = (at: number): number => {
    let characterClass = false;
    for (let i = at + 1; i < source.length; i++) {
      if (source[i] === '\\') { i++; continue; }
      if (source[i] === '[') characterClass = true;
      if (source[i] === ']') characterClass = false;
      if (source[i] === '/' && !characterClass) return i + 1;
      assert.notEqual(source[i], '\n', 'unterminated runner regular expression');
    }
    assert.fail('unterminated runner regular expression');
  };
  const commentEnd = (at: number): number => {
    if (source[at + 1] === '/') {
      const end = source.indexOf('\n', at + 2); return end < 0 ? source.length : end;
    }
    const end = source.indexOf('*/', at + 2);
    assert.ok(end >= 0, 'unterminated runner block comment'); return end + 2;
  };
  // Literal text is inert; interpolation expressions remain executable code.
  // Nested literals/comments need their own boundaries to prevent replay.
  const interpolationEnd = (at: number): number => {
    let depth = 1;
    for (let i = at; i < source.length;) {
      const c = source[i], two = source.slice(i, i + 2);
      if (two === '//' || two === '/*') { i = commentEnd(i); continue; }
      if (c === "'" || c === '"' || c === '`') { i = quotedEnd(i); continue; }
      if (c === '/' && regexStart(i)) { i = regexEnd(i); continue; }
      if (c === '{') depth++;
      if (c === '}' && --depth === 0) return i + 1;
      i++;
    }
    assert.fail('unterminated runner interpolation');
  };
  const quotedEnd = (at: number): number => {
    const quote = source[at];
    for (let i = at + 1; i < source.length;) {
      if (source[i] === '\\') { i += 2; continue; }
      if (quote === '`' && source.slice(i, i + 2) === '${') { i = interpolationEnd(i + 2); continue; }
      if (source[i] === quote) return i + 1;
      i++;
    }
    assert.fail('unterminated runner literal');
  };
  const scan = (from: number, to: number): void => {
    for (let i = from; i < to;) {
      const c = source[i], two = source.slice(i, i + 2);
      if (two === '//' || two === '/*') {
        const end = commentEnd(i); assert.ok(end <= to, 'comment escaped its runner expression');
        blank(visible, i, end); blank(code, i, end); i = end;
      } else if (c === "'" || c === '"' || c === '`' || c === '/' && regexStart(i)) {
        const end = c === '/' ? regexEnd(i) : quotedEnd(i);
        assert.ok(end <= to, 'literal escaped its runner expression'); blank(code, i, end);
        if (c === '`') {
          for (let at = i + 1; at < end - 1;) {
            if (source[at] === '\\') { at += 2; continue; }
            if (source.slice(at, at + 2) === '${') {
              const stop = interpolationEnd(at + 2);
              for (let k = at + 1; k < stop; k++) code[k] = source[k];
              scan(at + 2, stop - 1); at = stop;
            } else at++;
          }
        }
        i = end;
      } else i++;
    }
  };
  scan(0, source.length);
  assert.equal(visible.length, source.length); assert.equal(code.length, source.length);
  return { visible: visible.join(''), code: code.join('') };
}

function runnerRegion(source: string, owner: string): { start: number; end: number; raw: string; visible: string; code: string } {
  assert.ok(source.trim().length > 0, `${owner}: nonempty actual source`);
  const views = runnerViews(source);
  const declaration = owner === 'TESTS' ? /^export const TESTS = Object\.freeze\(\[/gm
    : new RegExp(`^(?:export )?(?:async )?function ${owner}\\([^\\n]*\\) \\{`, 'gm');
  const matches = [...views.code.matchAll(declaration)];
  assert.equal(matches.length, 1, `${owner}: exactly one executable declaration`);
  const start = matches[0].index!;
  let end = -1;
  if (owner === 'TESTS') end = views.code.indexOf(']);', start) + 3;
  else {
    let depth = 0;
    for (let i = start + matches[0][0].lastIndexOf('{'); i < views.code.length; i++) {
      if (views.code[i] === '{') depth++;
      if (views.code[i] === '}' && --depth === 0) { end = i + 1; break; }
    }
  }
  assert.ok(end > start, `${owner}: nonempty closed owner`);
  return { start, end, raw: source.slice(start, end), visible: views.visible.slice(start, end), code: views.code.slice(start, end) };
}
function executableAnchor(source: string, anchor: string): number {
  const view = runnerViews(source), expected = runnerViews(anchor);
  assert.ok(expected.code.trim().length > 0, 'anchor needs actual code, not a literal or comment alone');
  const hits: number[] = [];
  for (let at = view.visible.indexOf(expected.visible); at >= 0; at = view.visible.indexOf(expected.visible, at + 1)) {
    if (view.code.slice(at, at + anchor.length) === expected.code) hits.push(at);
  }
  assert.equal(hits.length, 1, `one executable anchor: ${anchor}`); return hits[0];
}
function requireRunnerCode(source: string, owner: string, anchor: string): void {
  executableAnchor(runnerRegion(source, owner).raw, anchor);
}
const nativeLines = (source: string): string => runnerViews(source).visible.split('\n').map(line => line.trim()).filter(Boolean).join('\n');

const NATIVE_MAIN = String.raw`async function main() {
  const ordinary = process.argv.length === 3 && ({ '--bootstrap-contracts': BOOTSTRAP_TESTS, '--source-contracts': SOURCE_TESTS })[process.argv[2]];
  if (ordinary) {
    const root = resolve(fileURLToPath(new URL('../', import.meta.url)));
    demand(realpathSync(process.cwd()) === realpathSync(root), 'Run native source contracts from the workspace root');
    return runContractSuite(ordinary, root);
  }

  demand(JSON.stringify(process.argv.slice(2)) === JSON.stringify(TESTS), 'Controlled test scope/order differs from the six reviewed exact names');
  const root = resolve(fileURLToPath(new URL('../', import.meta.url)));
  demand(realpathSync(process.cwd()) === realpathSync(root), 'Run the controlled tests from the workspace root');
  const directory = join(root, 'native-dns-ci-evidence');
  mkdirSync(directory, { recursive: true });
  const statusPath = join(directory, 'runtime-status.txt');
  writeFileSync(statusPath, '');
  const metadata = JSON.parse(execFileSync('cargo', ['metadata', '--locked', '--no-deps', '--format-version=1'],
    { cwd: root, encoding: 'utf8', timeout: 60_000, maxBuffer: 16 * 1024 * 1024 }));
  const prepared = await executeCommand({ id: 'controlled-build', program: 'cargo',
    args: ['build', '--locked', '-p', 'polaris', '--features', 'native-production-verification', '--bin', 'native-admission-verifier', '--message-format=json'] },
  { root, env: process.env, directory, timeout: 20 * 60_000, captureStdout: true });
  demand(prepared.status === 0 && !prepared.error && !prepared.cleanupUncertain && !prepared.interrupted, 'Controlled production verification preparation failed');
  const messages = prepared.stdout.split(/\r?\n/).filter(Boolean).map((line) => JSON.parse(line));
  const selected = selectNativeExecutable(messages, metadata, root);
  selected.sha256 = hash(selected.executable);
  writeFileSync(join(directory, 'controlled-cargo-metadata.json'), JSON.stringify(metadata) + '\n');
  writeFileSync(join(directory, 'controlled-executable.json'), JSON.stringify({ ...selected, preparationCalls: 1 }, null, 2) + '\n');
  bindNativeRuntime(selected, root, process.env, directory);
  const sysroot = execFileSync('rustc', ['--print', 'target-libdir'], { encoding: 'utf8', timeout: 30_000 }).trim();
  const env = runtimeEnvironment(messages, resolve(metadata.target_directory), selected.executable, sysroot);
  // Cargo runs libtests in the package directory, rather than the workspace root.
  await withNativeSession(selected, env, directory, () => runTests(selected, env, directory));
}`;
const NATIVE_SELECTOR = String.raw`export function selectNativeExecutable(messages, metadata, root) {
  const packages=metadata.packages.filter((p)=>p.name==='polaris' && resolve(p.manifest_path)===join(root,'src-tauri/Cargo.toml'));
  demand(packages.length===1,'Expected current App package');
  const pkg=packages[0];
  const targets=pkg.targets.filter((t)=>t.name==='native-admission-verifier' && t.kind.length===1 && t.kind[0]==='bin'
    && resolve(t.src_path)===join(root,'src-tauri/src/bin/native-admission-verifier.rs'));
  demand(targets.length===1,'Expected reviewed production verification binary');
  const rows=messages.filter((m)=>m.reason==='compiler-artifact' && m.package_id===pkg.id && m.profile?.test===false && m.profile?.debug_assertions===true
    && m.target?.name===targets[0].name && JSON.stringify(m.target.kind)===JSON.stringify(targets[0].kind)
    && resolve(m.target.src_path)===resolve(targets[0].src_path) && m.executable
    && Array.isArray(m.features) && m.features.includes('native-production-verification'));
  demand(rows.length===1,'Expected one normal-library verification artifact');
  const artifact=rows[0], executable=artifact.executable;
  demand(isAbsolute(executable) && inside(resolve(metadata.target_directory),executable),'Native artifact escaped Cargo target');
  demand(lstatSync(executable).isFile() && realpathSync(executable)===executable,'Native artifact must be an unlinked regular file');
  demand(artifact.features.every((value) => typeof value === 'string')
    && new Set(artifact.features).size === artifact.features.length, 'Malformed native artifact features');
  const selected = {executable,packageId:pkg.id,manifest:pkg.manifest_path,features:[...artifact.features],
    carrier:'normal-production-library',nativeProduction:true,nonTest:true,debugAssertions:true};
  nativeSelections.set(selected, JSON.stringify(selectionIdentity(selected)));
  return selected;
}`;
const NATIVE_CASE_DECLARATION = String.raw`export const TESTS = Object.freeze([
  'runtime::proxy::tests::lifecycle::real_core_full_lifecycle',
  'runtime::proxy::tests::lifecycle::real_core_lifecycle_race_start_then_immediate_stop',
  'runtime::proxy::tests::hot_switch::real_core_hot_switch_keeps_pid',
  'runtime::proxy::tests::hot_switch::real_core_auto_failover_attests_without_applying_saved_debt',
  'runtime::proxy::tests::hot_switch::real_core_hot_switch_failure_waits_for_explicit_restart_confirmation',
  'runtime::stats::tests::real_core_tests::real_core_aggregate_relay_emits_real_frames',
]);`;

function assertClosedNativeMain(source: string): void {
  const main = runnerRegion(source, 'main');
  assert.equal(nativeLines(main.raw), nativeLines(NATIVE_MAIN), 'exact single awaited normal build/receipt/binding/env/closed-session route');
  assert.equal(nativeLines(runnerRegion(source, 'selectNativeExecutable').raw), nativeLines(NATIVE_SELECTOR), 'exact current normal non-test debug artifact selector');
  assert.equal(nativeLines(runnerRegion(source, 'TESTS').raw), nativeLines(NATIVE_CASE_DECLARATION), 'all six names in their reviewed order, once');
  const code = runnerViews(source).code;
  for (const name of ['withNativeSession', 'runTests', 'bindNativeRuntime']) {
    assert.equal([...code.matchAll(new RegExp(`\\b${name}\\s*\\(`, 'g'))].length, 2, `${name}: one declaration and one actual invocation`);
  }
  requireRunnerCode(source, 'bindNativeRuntime', 'nativeSelections.get(selected) === JSON.stringify(selectionIdentity(selected))');
  requireRunnerCode(source, 'bindNativeRuntime', 'const artifact = nativeArtifact(root, directory);');
  requireRunnerCode(source, 'withNativeSession', 'holder = await start(selected, env, directory);');
  requireRunnerCode(source, 'withNativeSession', 'await run();');
  const session = runnerRegion(source, 'withNativeSession').raw;
  const attach = executableAnchor(session, 'authority.session = holder;');
  const invoke = executableAnchor(session, 'await run();');
  const detach = executableAnchor(session, 'delete authority.session;');
  const finish = executableAnchor(session, 'await holder.finish()');
  assert.ok(attach < invoke && invoke < detach && detach < finish, 'await the suite inside the one attached holder before teardown');
  requireRunnerCode(source, 'runTests', 'for (const name of TESTS) {');
  requireRunnerCode(source, 'runTests', "const result = await execute({ id: name.split('::').at(-1), program: selected.executable,");
  requireRunnerCode(source, 'runTests', "args: selected.carrier === 'normal-production-library' ? [name] : [name, '--exact', '--ignored', '--nocapture', '--test-threads=1']");
}


const CLOSED_SESSION_CALL = 'await withNativeSession(selected, env, directory, () => runTests(selected, env, directory));';
const CLOSED_ROUTE_NEGATIVES = [
  ['empty main', 'main', NATIVE_MAIN, 'async function main() {}'],
  ['duplicate main', 'main', NATIVE_MAIN, NATIVE_MAIN + '\n' + NATIVE_MAIN],
  ['main string replay', 'main', NATIVE_MAIN, 'async function main() {}\nconst replay = ' + JSON.stringify(NATIVE_MAIN) + ';'],
  ['fire-and-forget callback', 'main', '() => runTests(selected, env, directory)', '() => { runTests(selected, env, directory); }'],
  ['extra suite outside main', 'main', NATIVE_MAIN, NATIVE_MAIN + '\nawait runTests(selected, env, directory);'],
  ['extra wrapper outside main', 'main', NATIVE_MAIN, NATIVE_MAIN + '\n' + CLOSED_SESSION_CALL],
  ['binding wrong selected', 'main', 'bindNativeRuntime(selected, root, process.env, directory);', 'bindNativeRuntime(otherSelected, root, process.env, directory);'],
  ['binding wrong env', 'main', 'bindNativeRuntime(selected, root, process.env, directory);', 'bindNativeRuntime(selected, root, otherEnv, directory);'],
  ['binding wrong directory', 'main', 'bindNativeRuntime(selected, root, process.env, directory);', 'bindNativeRuntime(selected, root, process.env, otherDirectory);'],
  ['template expression extra suite', 'main', NATIVE_MAIN, NATIVE_MAIN + '\nconst other = `${runTests(selected, env, directory)}`;'],
  ['suite loop narrowed', 'runTests', 'for (const name of TESTS) {', 'for (const name of TESTS.slice(0, 1)) {'],
  ['suite loop reordered', 'runTests', 'for (const name of TESTS) {', 'for (const name of [...TESTS].reverse()) {'],
  ['suite loop duplicated', 'runTests', 'for (const name of TESTS) {', 'for (const name of [...TESTS, ...TESTS]) {'],
  ['session callback not awaited', 'withNativeSession', 'await run();', 'run();'],
  ['wrapper replaced', 'main', CLOSED_SESSION_CALL, 'await otherSession(selected, env, directory, () => runTests(selected, env, directory));'],
  ['callback disconnected', 'main', '() => runTests(selected, env, directory)', '() => otherTests(selected, env, directory)'],
  ['callback empty', 'main', '() => runTests(selected, env, directory)', '() => {}'],
  ['wrapper not awaited', 'main', CLOSED_SESSION_CALL, CLOSED_SESSION_CALL.replace('await ', '')],
  ['double wrapper', 'main', CLOSED_SESSION_CALL, `${CLOSED_SESSION_CALL}\n  ${CLOSED_SESSION_CALL}`],
  ['extra direct suite', 'main', CLOSED_SESSION_CALL, `${CLOSED_SESSION_CALL}\n  await runTests(selected, env, directory);`],
  ['direct suite bypass', 'main', CLOSED_SESSION_CALL, 'await runTests(selected, env, directory);'],
  ['per-case sessions', 'main', CLOSED_SESSION_CALL, 'for (const name of TESTS) await withNativeSession(selected, env, directory, () => runTests(selected, env, directory));'],
  ['wrong selected', 'main', CLOSED_SESSION_CALL, CLOSED_SESSION_CALL.replace('withNativeSession(selected,', 'withNativeSession(otherSelected,')],
  ['wrong env', 'main', '() => runTests(selected, env, directory)', '() => runTests(selected, otherEnv, directory)'],
  ['wrong directory', 'main', '() => runTests(selected, env, directory)', '() => runTests(selected, env, otherDirectory)'],
  ['binding omitted', 'main', 'bindNativeRuntime(selected, root, process.env, directory);', 'void selected;'],
  ['build repeated', 'main', "const prepared = await executeCommand({ id: 'controlled-build', program: 'cargo',", "await executeCommand({ id: 'other-build', program: 'cargo', args: [] }, {});\n  const prepared = await executeCommand({ id: 'controlled-build', program: 'cargo',"],
  ['forged receipt', 'main', 'JSON.stringify({ ...selected, preparationCalls: 1 }, null, 2)', 'JSON.stringify({ ...otherSelected, preparationCalls: 1 }, null, 2)'],
  ['line comment replay', 'main', CLOSED_SESSION_CALL, `await otherTests(selected, env, directory); // ${CLOSED_SESSION_CALL}`],
  ['block comment replay', 'main', CLOSED_SESSION_CALL, `await otherTests(selected, env, directory); /* ${CLOSED_SESSION_CALL} */`],
  ['string replay', 'main', CLOSED_SESSION_CALL, `await otherTests(selected, env, directory); const replay = ${JSON.stringify(CLOSED_SESSION_CALL)};`],
  ['template replay', 'main', CLOSED_SESSION_CALL, 'await otherTests(selected, env, directory); const replay = `' + CLOSED_SESSION_CALL + '`;'],
  ["binding after session", "main", "  bindNativeRuntime(selected, root, process.env, directory);\n  const sysroot = execFileSync('rustc', ['--print', 'target-libdir'], { encoding: 'utf8', timeout: 30_000 }).trim();\n  const env = runtimeEnvironment(messages, resolve(metadata.target_directory), selected.executable, sysroot);\n  // Cargo runs libtests in the package directory, rather than the workspace root.\n  await withNativeSession(selected, env, directory, () => runTests(selected, env, directory));", "  const sysroot = execFileSync('rustc', ['--print', 'target-libdir'], { encoding: 'utf8', timeout: 30_000 }).trim();\n  const env = runtimeEnvironment(messages, resolve(metadata.target_directory), selected.executable, sysroot);\n  // Cargo runs libtests in the package directory, rather than the workspace root.\n  await withNativeSession(selected, env, directory, () => runTests(selected, env, directory));\n  bindNativeRuntime(selected, root, process.env, directory);"],
  ["case order changed", "TESTS", "  'runtime::proxy::tests::lifecycle::real_core_full_lifecycle',\n  'runtime::proxy::tests::lifecycle::real_core_lifecycle_race_start_then_immediate_stop',", "  'runtime::proxy::tests::lifecycle::real_core_lifecycle_race_start_then_immediate_stop',\n  'runtime::proxy::tests::lifecycle::real_core_full_lifecycle',"],
  ["empty suite", "TESTS", NATIVE_CASE_DECLARATION, "export const TESTS = Object.freeze([]);"],
  ["duplicate case", "TESTS", "  'runtime::proxy::tests::lifecycle::real_core_full_lifecycle',\n  'runtime::proxy::tests::lifecycle::real_core_lifecycle_race_start_then_immediate_stop',", "  'runtime::proxy::tests::lifecycle::real_core_full_lifecycle',\n  'runtime::proxy::tests::lifecycle::real_core_lifecycle_race_start_then_immediate_stop',\n  'runtime::proxy::tests::lifecycle::real_core_full_lifecycle',"],
] as const;


// The no-package explanation also depends on the real native-required CI job,
// its full platform matrix and the actual six-case shell invocation.
function nativeCiRegion(source: string, owner: 'header' | 'runtime'): { start: number; end: number; raw: string; visible: string } {
  const visible = source.replace(/^[ \t]*#.*$/gm, line => ' '.repeat(line.length));
  const jobs = [...visible.matchAll(/^  test:\n/gm)];
  assert.equal(jobs.length, 1, 'one actual native CI job');
  const jobStart = jobs[0].index!;
  const next = /^  [a-z][a-z0-9_-]*:\n/gm; next.lastIndex = jobStart + jobs[0][0].length;
  const jobEnd = next.exec(visible)?.index ?? visible.length;
  let start = jobStart, end = visible.indexOf('\n    steps:', start) + '\n    steps:'.length;
  if (owner === 'runtime') {
    const marker = '      - name: Run exact controlled App lifecycle hot-switch and stats tests';
    const positions = [...visible.slice(jobStart, jobEnd).matchAll(/^      - name: Run exact controlled App lifecycle hot-switch and stats tests$/gm)];
    assert.equal(positions.length, 1, 'one actual controlled runtime step in native job');
    start = jobStart + positions[0].index!;
    const sibling = /^      - /gm; sibling.lastIndex = start + marker.length;
    end = sibling.exec(visible)?.index ?? jobEnd;
  }
  assert.ok(start >= jobStart && end > start && end <= jobEnd, 'closed nonempty native CI owner');
  return { start, end, raw: source.slice(start, end), visible: visible.slice(start, end) };
}
const yamlNativeLines = (source: string): string => source.replace(/^[ \t]*#.*$/gm, '').split('\n').map(line => line.trim()).filter(Boolean).join('\n');
const NATIVE_CI_HEADER = "  test:\n    needs: cheap_format\n    if: >-\n      needs.cheap_format.outputs.runtime_config_loopback_only != 'true' && needs.cheap_format.outputs.native_required != 'false'\n    name: ${{ needs.cheap_format.outputs.app_native_compile_only == 'true' && 'Native consumer compile only' || 'build + test' }} (${{ matrix.os }})\n    runs-on: ${{ matrix.os }}\n    timeout-minutes: 120\n    permissions:\n      contents: read\n    strategy:\n      # 维持 false 的理由见 lint job 的同名字段（那段别优化掉；两个矩阵 job 取同一判据）。\n      fail-fast: false\n      matrix:\n        # 与 lint job 的矩阵表达式**逐字相同**（事件条件矩阵 + dispatch 单平台分支，逐条理由写在那边）。\n        # 两份必须一起改：只改一份，同一事件下「lint 跑了哪些平台」与「test 跑了哪些平台」就分家了。\n        # 逐字相等由 `ui/src/contracts/package-ci-matrix-contract.test.ts` 钉住。\n        os: >-\n          ${{ (github.event_name == 'workflow_dispatch'\n              && github.event.inputs.os != ''\n              && github.event.inputs.os != 'all')\n              && fromJSON(format('[\"{0}\"]', github.event.inputs.os))\n              || ((github.event_name == 'push' && github.ref == 'refs/heads/main')\n                  && fromJSON('[\"ubuntu-22.04\"]')\n                  || fromJSON('[\"ubuntu-22.04\",\"windows-2022\",\"macos-15\"]')) }}\n\n    steps:";
const NATIVE_CI_RUNTIME = "      - name: Run exact controlled App lifecycle hot-switch and stats tests\n        id: native_dns_app_runtime\n        if: ${{ needs.cheap_format.outputs.app_native_compile_only != 'true' && !cancelled() && (needs.cheap_format.outputs.native_dns_candidate != '' || needs.cheap_format.outputs.native_dns_default == 'true' || needs.cheap_format.outputs.native_dns_formal == 'true') && steps.workspace_build.outcome == 'success' && (steps.workspace_tests.outcome == 'success' || steps.workspace_tests.outcome == 'failure') }}\n        shell: bash\n        run: |\n          set -euo pipefail\n          node --test scripts/native-dns-ci-routing.test.mjs scripts/run-native-dns-controlled-tests.test.mjs scripts/windows-directory-toolchain.test.mjs\n          tests=(\n            runtime::proxy::tests::lifecycle::real_core_full_lifecycle\n            runtime::proxy::tests::lifecycle::real_core_lifecycle_race_start_then_immediate_stop\n            runtime::proxy::tests::hot_switch::real_core_hot_switch_keeps_pid\n            runtime::proxy::tests::hot_switch::real_core_auto_failover_attests_without_applying_saved_debt\n            runtime::proxy::tests::hot_switch::real_core_hot_switch_failure_waits_for_explicit_restart_confirmation\n            runtime::stats::tests::real_core_tests::real_core_aggregate_relay_emits_real_frames\n          )\n          node scripts/run-native-dns-controlled-tests.mjs \"${tests[@]}\"\n";
function assertNativeCiRoute(source: string): void {
  assert.equal(yamlNativeLines(nativeCiRegion(source, 'header').raw), yamlNativeLines(NATIVE_CI_HEADER), 'native-required gate keeps its full platform scope');
  assert.equal(yamlNativeLines(nativeCiRegion(source, 'runtime').raw), yamlNativeLines(NATIVE_CI_RUNTIME), 'real native CI runs all six normal cases with existing build/admission/error gates');
}
const NATIVE_CI_NEGATIVES = [
  ['native job disconnected', 'header', "needs.cheap_format.outputs.native_required != 'false'", 'false'],
  ['platform scope narrowed', 'header', "fromJSON('[\"ubuntu-22.04\",\"windows-2022\",\"macos-15\"]')", "fromJSON('[\"ubuntu-22.04\"]')"],
  ['runner disconnected', 'runtime', 'node scripts/run-native-dns-controlled-tests.mjs "${tests[@]}"', 'node scripts/other.mjs "${tests[@]}"'],
  ['case omitted', 'runtime', '            runtime::stats::tests::real_core_tests::real_core_aggregate_relay_emits_real_frames\n', ''],
  ['default admission removed', 'runtime', "needs.cheap_format.outputs.native_dns_default == 'true'", 'false'],
  ['build gate bypassed', 'runtime', "steps.workspace_build.outcome == 'success'", 'true'],
  ['error ignored', 'runtime', '        shell: bash', '        continue-on-error: true\n        shell: bash'],
] as const;

describe('CI 影响分类器的完备性（fail-open 根治）', () => {
  it('实际核身份采集入口的变更同时点亮真核门与桌面包装，不能只登记包装腿', () => {
    expect(compact(['scripts/native-payload-input.mjs'])).toEqual({
      kernel: true,
      platforms: [...ALL_PACKAGE_PLATFORMS],
      preflight: true,
      hasPackage: true,
    });
  });
  it('实际编译前安装身份入口和共享进程租约覆盖全部桌面载体', () => {
    for (const path of ['scripts/prepare-core-identity.mjs', 'crates/helper/src/process_lease.rs']) {
      const actual = classifyImpact([path]);
      expect(actual.platforms).toEqual([...ALL_PACKAGE_PLATFORMS]);
      expect(actual.hasPackage).toBe(true);
      expect(actual.unregisteredScopes).toEqual([]);
    }
  });
  it('验收 carrier 的 no-package 判定绑定真实 feature/target，并保留 native CI', () => {
    const cargo = readFileSync(join(REPO_ROOT, 'src-tauri/Cargo.toml'), 'utf8');
    const binaries = cargo.split(/^\[\[bin\]\]\s*$/m).slice(1)
      .map((section) => section.split(/^\[/m)[0]);
    const verifier = binaries.filter((section) => /^name\s*=\s*"native-admission-verifier"\s*$/m.test(section));
    expect(verifier).toHaveLength(1);
    expect(verifier[0]).toMatch(/^path\s*=\s*"src\/bin\/native-admission-verifier\.rs"\s*$/m);
    expect(verifier[0]).toMatch(/^required-features\s*=\s*\["native-production-verification"\]\s*$/m);
    expect(cargo).toMatch(/^native-production-verification\s*=\s*\[\]\s*$/m);
    expect(cargo.match(/^default\s*=.*$/m)?.[0]).not.toContain('native-production-verification');
    // A new implicit/default binary needs a fresh impact decision, rather than
    // inheriting the current acceptance-only explanation for src/bin/.
    expect(readdirSync(join(REPO_ROOT, 'src-tauri/src/bin')).sort())
      .toEqual(['native-admission-verifier.rs']);
    const entry = maskRustComments(readFileSync(join(REPO_ROOT, 'src-tauri/src/bin/native-admission-verifier.rs'), 'utf8'));
    expect(entry).toMatch(/#\[cfg\(not\(debug_assertions\)\)\]\s*compile_error!\("native admission verifier is unavailable in release builds"\);/);
    expect(entry).toContain('polaris_lib::run_native_production_verification(&args[0]).await');
    const library = maskRustComments(readFileSync(join(REPO_ROOT, 'src-tauri/src/lib.rs'), 'utf8'));
    const proxy = maskRustComments(readFileSync(join(REPO_ROOT, 'src-tauri/src/runtime/proxy.rs'), 'utf8'));
    const carrier = maskRustComments(readFileSync(join(REPO_ROOT, 'src-tauri/src/native_verification/proxy_carrier.rs'), 'utf8'));
    // Separate cfg attrs are an AND. All three parent guards, the local debug
    // guard and the exact normal-library route are mandatory, never optional.
    const guardedDefinitionIsUnique = (source: string, guard: RegExp, declaration: RegExp): boolean => {
      // Both Rust masks preserve UTF-8 byte offsets. Compare byte-wide views so
      // Unicode before a declaration cannot move the ownership comparison.
      const visible = Buffer.from(source, 'utf8').toString('latin1');
      const code = Buffer.from(maskRustCommentsAndStrings(source), 'utf8').toString('latin1');
      const guarded = [...visible.matchAll(guard)];
      const declared = [...code.matchAll(declaration)];
      return guarded.length === 1 && declared.length === 1
        && guarded[0].index + guarded[0][0].indexOf('pub') === declared[0].index;
    };
    const parentGuardMutations = [
      ['#[cfg(desktop)]', ''],
      ['#[cfg(desktop)]', '#[cfg(mobile)]'],
      ['#[cfg(feature = "native-production-verification")]', ''],
      ['#[cfg(feature = "native-production-verification")]', '#[cfg(feature = "unreviewed-native-feature")]'],
      ['#[cfg(debug_assertions)]', ''],
      ['#[cfg(debug_assertions)]', '#[cfg(not(debug_assertions))]'],
    ] as const;
    const guardedOwners = [
      {
        name: 'normal-library export', source: library,
        guard: /^#\[cfg\(desktop\)\]\s*#\[cfg\(feature = "native-production-verification"\)\]\s*#\[cfg\(debug_assertions\)\]\s*pub async fn run_native_production_verification\(case: &str\) -> Result<\(\), String> \{\s*runtime::proxy::native_verification::run_case\(case\)\.await\s*\}/gm,
        declaration: /\bpub\s+async\s+fn\s+run_native_production_verification\s*\(/g,
        mutations: [
          ...parentGuardMutations,
          ['pub async fn run_native_production_verification(', 'pub async fn unreviewed_native_verification('],
          ['runtime::proxy::native_verification::run_case(case).await', 'runtime::proxy::native_verification::unreviewed_case(case).await'],
        ],
      },
      {
        name: 'normal-library carrier module', source: proxy,
        guard: /^#\[cfg\(desktop\)\]\s*#\[cfg\(feature = "native-production-verification"\)\]\s*#\[cfg\(debug_assertions\)\]\s*#\[path = "\.\.\/native_verification\/proxy_carrier\.rs"\]\s*pub\(crate\) mod native_verification;/gm,
        declaration: /\bpub\(crate\)\s+mod\s+native_verification\s*;/g,
        mutations: [
          ...parentGuardMutations,
          ['../native_verification/proxy_carrier.rs', '../native_verification/unreviewed_carrier.rs'],
          ['pub(crate) mod native_verification;', 'pub(crate) mod unreviewed_verification;'],
        ],
      },
      {
        name: 'local closed case entry', source: carrier,
        guard: /^#\[cfg\(debug_assertions\)\]\s*pub\(crate\) async fn run_case\(case: &str\) -> Result<\(\), String> \{\s*if !CASE_NAMES\.contains\(&case\) \{/gm,
        declaration: /\bpub\(crate\)\s+async\s+fn\s+run_case\s*\(/g,
        mutations: [
          ['#[cfg(debug_assertions)]', ''],
          ['#[cfg(debug_assertions)]', '#[cfg(not(debug_assertions))]'],
          ['pub(crate) async fn run_case(', 'pub(crate) async fn unreviewed_case('],
          ['if !CASE_NAMES.contains(&case)', 'if false'],
        ],
      },
    ] as const;
    for (const { name, source, guard, declaration, mutations } of guardedOwners) {
      expect(source.trim().length, `${name}: nonempty actual source`).toBeGreaterThan(0);
      expect(guardedDefinitionIsUnique(source, guard, declaration), name).toBe(true);
      const matches = [...source.matchAll(guard)];
      expect(matches, `${name}: one actual guarded target`).toHaveLength(1);
      const target = matches[0][0];
      const start = matches[0].index;
      expect(target.length, `${name}: nonempty target read domain`).toBeGreaterThan(0);
      for (const [old, replacement] of mutations) {
        expect(guardedDefinitionIsUnique(source, guard, declaration), `${name}: positive before each mutation`).toBe(true);
        expect(target.split(old), `${name}: unique target anchor ${old}`).toHaveLength(2);
        const changedTarget = target.replace(old, replacement);
        expect(changedTarget, `${name}: target itself must change`).not.toBe(target);
        expect(changedTarget.trim().length, `${name}: mutation cannot empty the read domain`).toBeGreaterThan(0);
        const changed = source.slice(0, start) + changedTarget + source.slice(start + target.length);
        expect(changed.slice(start, start + changedTarget.length)).toBe(changedTarget);
        expect(changed.slice(0, start)).toBe(source.slice(0, start));
        expect(changed.slice(start + changedTarget.length)).toBe(source.slice(start + target.length));
        expect(guardedDefinitionIsUnique(changed, guard, declaration), `${name}: reject ${old} -> ${replacement}`).toBe(false);
      }
      expect(guardedDefinitionIsUnique(`${source}\n${target}`, guard, declaration), `${name}: duplicate guarded target`).toBe(false);
      const unguarded = target.slice(target.indexOf('pub'));
      expect(guardedDefinitionIsUnique(`${source}\n${unguarded}`, guard, declaration), `${name}: duplicate unguarded target`).toBe(false);
      const withoutDebug = target.replace('#[cfg(debug_assertions)]', '');
      const changed = source.slice(0, start) + withoutDebug + source.slice(start + target.length);
      expect(guardedDefinitionIsUnique(`${changed}\nconst REPLAY: &str = r####"\n${target}\n"####;`, guard, declaration), `${name}: string replay cannot replace the actual guard`).toBe(false);
    }
    const runner = readFileSync(join(REPO_ROOT, 'scripts/run-native-dns-controlled-tests.mjs'), 'utf8');
    assertClosedNativeMain(runner);
    const nativeCi = readFileSync(join(REPO_ROOT, '.github/workflows/ci.yml'), 'utf8');
    assertNativeCiRoute(nativeCi);
    for (const [name, owner, before, after] of NATIVE_CI_NEGATIVES) {
      assertNativeCiRoute(nativeCi);
      const region = nativeCiRegion(nativeCi, owner), at = executableAnchor(region.raw, before);
      expect(region.raw.slice(at, at + before.length), name).toBe(before);
      const changedTarget = region.raw.slice(0, at) + after + region.raw.slice(at + before.length);
      expect(changedTarget, name).not.toBe(region.raw);
      expect(changedTarget.trim().length, name).toBeGreaterThan(0);
      const changed = nativeCi.slice(0, region.start) + changedTarget + nativeCi.slice(region.end);
      expect(changed.slice(0, region.start), name).toBe(nativeCi.slice(0, region.start));
      expect(changed.slice(region.start + changedTarget.length), name).toBe(nativeCi.slice(region.end));
      expect(() => assertNativeCiRoute(changed), name).toThrow(AssertionError);
    }
    for (const [name, owner, before, after] of CLOSED_ROUTE_NEGATIVES) {
      assertClosedNativeMain(runner); // Positive baseline before every individual mutation.
      const region = runnerRegion(runner, owner), at = executableAnchor(region.raw, before);
      const changedTarget = region.raw.slice(0, at) + after + region.raw.slice(at + before.length);
      expect(region.raw.slice(at, at + before.length), name).toBe(before);
      expect(changedTarget, name).not.toBe(region.raw);
      expect(changedTarget.trim().length, name).toBeGreaterThan(0);
      const changed = runner.slice(0, region.start) + changedTarget + runner.slice(region.end);
      expect(changed.slice(0, region.start), name).toBe(runner.slice(0, region.start));
      expect(changed.slice(region.start + changedTarget.length), name).toBe(runner.slice(region.end));
      expect(() => assertClosedNativeMain(changed), name).toThrow(AssertionError);
    }
    for (const path of [
      'src-tauri/src/bin/native-admission-verifier.rs',
      'src-tauri/src/native_verification/proxy_carrier.rs',
      'src-tauri/src/native_verification/cleanup.rs',
    ]) {
      expect(compact([path]), path).toEqual({ kernel: false, platforms: [], preflight: false, hasPackage: false });
      expect(classifyImpact([path]).android, path).toBe(false);
      expect(classifyImpact([path]).unregisteredScopes, path).toEqual([]);
      const diff = `:100644 100644 ${'1'.repeat(40)} ${'2'.repeat(40)} M\0${path}\0`;
      expect(classifyNativeImpact(diff).nativeRequired, path).toBe(true);
      expect(classifyNativeImpact(`:100644 100644 ${'1'.repeat(40)} ${'2'.repeat(40)} M\0scripts/postprocess-appimage.test.mjs\0${diff}`).nativeRequired, `mixed: ${path}`).toBe(true);
    }
    for (const scope of ['src-tauri/src/bin/', 'src-tauri/src/native_verification/']) {
      expect(PACKAGE_IMPACT_SCOPES).not.toHaveProperty(scope);
      expect(NO_PACKAGE_IMPACT_SCOPES[scope]).toContain('native-production-verification');
    }
    for (const path of ['src-tauri/src/bin_other/entry.rs', 'src-tauri/src/native_verification_other/mod.rs']) {
      expect(classifyImpact([path]).unregisteredScopes, path).toHaveLength(1);
      expect(classifyImpact([path]).platforms, path).toEqual([...ALL_PACKAGE_PLATFORMS]);
    }
  });

  it('本地资源目录 symlink 按目录 scope 枚举，未知目录仍会触发登记门', () => {
    const root = mkdtempSync(join(tmpdir(), 'polaris-impact-scopes-'));
    try {
      mkdirSync(join(root, 'resources'));
      mkdirSync(join(root, 'fetched'));
      for (const name of ['dashboard', 'linux', '__not_registered__']) {
        symlinkSync(join(root, 'fetched'), join(root, 'resources', name), 'dir');
      }
      const scopes = scopeKeys('resources', root);
      expect(scopes).toEqual([
        'resources/__not_registered__/', 'resources/dashboard/', 'resources/linux/',
      ]);
      expect(scopes.filter((key) => !isScopeRegistered(key))).toEqual(['resources/__not_registered__/']);
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  });

  it('每个 crate / 每个 src-tauri 顶层子树都显式落在两张登记表之一', () => {
    const onDisk = scopesOnDisk();

    // 哨兵：readdir 变哑（空表 / 少读一棵树）在此红，而不是让下面的 for 循环恒真。
    expect(onDisk.length, `枚举到的 scope 太少（${onDisk.length}）—— 枚举器坏了，门在裸奔`).toBeGreaterThan(35);
    for (const known of [
      'crates/system-integration/',
      'crates/helper-client/',
      'crates/stats-engine/',
      'crates/config-engine/',
      'src-tauri/src/runtime/',
      'src-tauri/src/bin/',
      'src-tauri/src/native_verification/',
      'src-tauri/tauri.conf.json',
      // resources/data/ 的 28 个 .srs 入库且进四个包（见分类器的 PACKAGE_IMPACT_SCOPES）；
      // 它掉出枚举面 = 整个 resources/ 登记根没被扫到，那正是 2026-08-30 前的 fail-open 形态。
      'resources/data/',
      'vendor/swift-rs/',
    ]) {
      expect(onDisk, `枚举结果里没有 ${known} —— 枚举器的取材面漏了整棵树`).toContain(known);
    }

    // 「算不算已登记」只有分类器那一份实现（含 `foo.rs` ↔ `foo/` 模块别名）。在这里另写一份
    // `new Set(Object.keys(...)).has(key)` 就是同一事实两处实现：本门会因此与真正的分类结果背离
    // —— 门说「没登记」而分类器说「登记了」，或者反过来（后者是静默放行）。
    const missing = onDisk.filter((key) => !isScopeRegistered(key));
    expect(
      missing,
      `这些 scope 在 scripts/classify-ci-impact.mjs 的两张表里都没有：\n` +
        missing.map((key) => `  · ${key}`).join('\n') +
        `\n新增 crate / 新建 src 子树时必须显式判一次：影响打包 ⇒ 写进 PACKAGE_IMPACT_SCOPES` +
        `（带 kernel / platforms）；不影响 ⇒ 写进 NO_PACKAGE_IMPACT_SCOPES 并附一句理由。` +
        `不判的默认后果是分类器 fail-closed 成内核门 + 四平台。`,
    ).toEqual([]);
  });

  it('登记表不是装饰：每条登记的判定必须等于分类器对该 scope 的实际输出', () => {
    // 表里写「四平台」而分类器实际什么都不加（或反过来），在这里红。
    // 没有这条，两张表就退化成注释——注释对执行没有强制力。
    expect(Object.keys(PACKAGE_IMPACT_SCOPES).length).toBeGreaterThan(20);
    expect(Object.keys(NO_PACKAGE_IMPACT_SCOPES).length).toBeGreaterThan(30);

    for (const [key, decision] of Object.entries(PACKAGE_IMPACT_SCOPES)) {
      const actual = compact([probeOf(key)]);
      expect(actual.kernel, `PACKAGE_IMPACT_SCOPES['${key}'].kernel 与实际分类不一致`).toBe(
        decision.kernel,
      );
      expect(actual.platforms, `PACKAGE_IMPACT_SCOPES['${key}'].platforms 与实际分类不一致`).toEqual(
        ALL_PACKAGE_PLATFORMS.filter((platform) => decision.platforms.includes(platform)),
      );
      expect(
        decision.why.length,
        `PACKAGE_IMPACT_SCOPES['${key}'] 缺少可读的判据（why）`,
      ).toBeGreaterThan(10);
    }

    for (const [key, why] of Object.entries(NO_PACKAGE_IMPACT_SCOPES)) {
      expect(
        compact([probeOf(key)]),
        `NO_PACKAGE_IMPACT_SCOPES['${key}'] 声称不影响打包，但分类器对它加了腿/内核门 —— 两者必须同真值`,
      ).toEqual({ kernel: false, platforms: [], preflight: false, hasPackage: false });
      expect(why.length, `NO_PACKAGE_IMPACT_SCOPES['${key}'] 的理由太短，等于没判`).toBeGreaterThan(10);
    }
  });

  it('登记根内的未登记路径 fail-closed 为内核门 + 四平台，并在输出里自曝', () => {
    // 「本门没跑 / 有人绕过本门」时的兜底：默认后果必须是多花钱，不是静默放行。
    for (const [probe, scope] of [
      ['crates/__not_registered__/src/lib.rs', 'crates/__not_registered__/'],
      ['src-tauri/src/__not_registered__/mod.rs', 'src-tauri/src/__not_registered__/'],
      ['src-tauri/__not_registered__.json', 'src-tauri/__not_registered__.json'],
      ['resources/__not_registered__/x.srs', 'resources/__not_registered__/'],
      ['vendor/__not_registered__/src/lib.rs', 'vendor/__not_registered__/'],
    ]) {
      const result = classifyImpact([probe]);
      expect(scopeOf(probe), `scopeOf('${probe}') 归一结果与预期 scope 不一致`).toBe(scope);
      expect(result.kernel, `${probe} 未登记却没触发内核门`).toBe(true);
      expect(result.platforms, `${probe} 未登记却没触发四平台`).toEqual([...ALL_PACKAGE_PLATFORMS]);
      expect(
        result.unregisteredScopes,
        `${probe} 未登记却没进 unregisteredScopes —— 「为什么这次全量」在日志里读不出来`,
      ).toEqual([scope]);
    }

    // 反向对照：已登记的路径不得进自曝表（否则上面那条恒真，等于没检查）。
    expect(classifyImpact(['crates/stats-engine/src/lib.rs']).unregisteredScopes).toEqual([]);
    expect(classifyImpact(['resources/data/geosite-cn.srs']).unregisteredScopes).toEqual([]);
  });

  it('真实 `.srs` 资源变更必须点亮完整配置真核门所需的 release-risk 路径', () => {
    // 这不是只验证「resources/data 已登记」：完整配置门现在让 sing-box 真读 route.rule_set 的
    // `.srs`，故分类为 kernel=false 会令 release-risk 跳过 Fetch core/cronet 与 mandatory gates。
    expect(compact(['resources/data/geosite-cn.srs'])).toEqual({
      kernel: true,
      platforms: [...ALL_PACKAGE_PLATFORMS],
      preflight: true,
      hasPackage: true,
    });
  });

  it('verify-packaging 当判据读的每个 src-tauri 源文件都必须触发打包腿', () => {
    // 根因形态：判据的**消费者**（verify-packaging.mjs）在影响表里，判据的**来源**却不在。
    // 历史形态：改 proxy.rs 的 LINUX_BUNDLE_PRODUCT_DIR 不触发任何腿，confs 门要等下一次真打包才炸。
    // 那个常量已随 F8 消失（productName 由 build.rs 注入，事实只剩一份），但本门要守的是**类**，
    // 不是那一个文件：今天的取材面是 build.rs 的 EXPECTED_SRS_COUNT + 两份 conf。
    const source = stripLineComments(
      readFileSync(join(REPO_ROOT, 'scripts/verify-packaging.mjs'), 'utf8'),
      'js',
    );

    // 切片自检：注释里确实提到过别的源文件路径，剥干净了才轮到下面的断言说话。
    expect(
      source.includes('crates/updater/src/github.rs'),
      '注释剥离失效：这个路径只出现在 verify-packaging.mjs 的注释里，却仍留在取材面上',
    ).toBe(false);

    const reads = [
      ...source.matchAll(/(?:join|resolve|readJson|readFileSync)\(\s*SRC_TAURI\s*,\s*'([^']+)'/g),
    ].map((match) => `src-tauri/${match[1]}`);

    // 哨兵：抓取器变哑（空表 / 漏条）在此红，否则下面的 for 恒真。
    // 取「必须包含」而非逐字等值：新增一处读取由下面的 for 直接判（不登记就红），不必回来改哨兵；
    // 而某处读取**消失**仍会在这里红一次，逼作者确认那条不变量是真没了、还是搬了家。
    for (const known of [
      'src-tauri/build.rs',
      'src-tauri/tauri.conf.json',
      'src-tauri/core-manifest.json',
    ]) {
      expect(
        [...new Set(reads)].sort(),
        `抓取器没读到 ${known} —— 要么它变哑了（门在裸奔，先修抓取器），` +
          '要么 verify-packaging.mjs 真的不再读它（那么本哨兵该跟着改）',
      ).toContain(known);
    }

    for (const path of new Set(reads)) {
      expect(
        classifyImpact([path]).hasPackage,
        `${path} 是 verify-packaging confs 的判据源，但改它不触发任何打包腿 —— ` +
          `release-risk.yml 的「Verify packaging conf invariants」步骤条件是 has_package == 'true'，` +
          `不加腿就等于这条不变量在合入前从不被检查`,
      ).toBe(true);
    }
  });

  it('CI workflow 真正执行的每个仓库脚本都必须触发它所在的那条腿', () => {
    // 同一个 fail-open 在 scripts/ 的姊妹腿：package.yml 真跑 `node scripts/fetch-protoc.mjs`，
    // 而它 2026-08-30 前不在任何表里 —— 改坏它，合入前零信号，打包链在发布时才断。
    //
    // 2026-09-04 两处加强：
    //  ① 取材面加入 `android.yml`（Android APK 腿）。它跑的 `verify-apk.mjs` / `build-libbox.sh`
    //     若落表外，就是同一个 fail-open 在 Android 侧原样复发。
    //  ② 断言改成**逐 (脚本, workflow) 对**、且每个 workflow 有各自的「该亮哪条腿」判据。
    //     原判据用 `Map<脚本, workflow>`（后写覆盖先写）：`fetch-protoc.mjs` 同时被 package.yml 与
    //     android.yml 跑，Map 只会留下后一个，另一条腿的要求**静默消失**。
    const requiredLeg: Record<string, { signal: (impact: Impact) => boolean; label: string }> = {
      'package.yml': { signal: (impact) => impact.hasPackage, label: '打包腿' },
      'release-risk.yml': { signal: (impact) => impact.hasPackage, label: '打包腿' },
      'android.yml': { signal: (impact) => impact.android, label: 'Android APK 腿' },
      'apple.yml': { signal: () => false, label: 'Apple unsigned App/PacketTunnel 腿' },
    };

    const invoked: Array<[string, string]> = [];
    for (const workflow of Object.keys(requiredLeg)) {
      const raw = readFileSync(join(REPO_ROOT, '.github/workflows', workflow), 'utf8');
      const source = stripLineComments(raw, 'yaml');
      // 切片自检：这两句各自只在整行注释里出现，剥干净了取材面才是「真跑的命令」。
      const commentOnly: Record<string, string> = {
        'package.yml': 'root cause：此前每次 main push',
        'android.yml': '[不选 package.yml 矩阵腿：见上',
      };
      if (commentOnly[workflow]) {
        expect(
          source.includes(commentOnly[workflow]),
          `YAML 注释剥离失效（${workflow}）：整行注释仍留在取材面上`,
        ).toBe(false);
      }
      for (const match of source.matchAll(
        /(?:^|[\s;&|(`])(?:node|sh|bash)\s+(?:--test\s+)?(scripts\/[A-Za-z0-9._/-]+)/g,
      )) {
        invoked.push([match[1], workflow]);
      }
    }

    // 哨兵：抓取器变哑在此红（空表会让下面的 for 恒真）。
    expect([...new Set(invoked.map(([script]) => script))].sort()).toEqual([
      'scripts/apple-ci-impact.mjs',
      'scripts/assert-pc-runtime-release.mjs',
      'scripts/assert-pc-runtime-release.test.mjs',
      'scripts/assert-r8-evidence.mjs',
      'scripts/build-android-apk.sh',
      'scripts/classify-ci-impact.mjs',
      'scripts/desktop-core/validation-origin.mjs',
      'scripts/desktop-core/wire-input-handoff.mjs',
      'scripts/fetch-core.mjs',
      'scripts/fetch-cronet.mjs',
      'scripts/fetch-dashboard.mjs',
      'scripts/fetch-protoc.mjs',
      'scripts/gate-android-release-behavior.sh',
      'scripts/gate-node-test.sh',
      'scripts/macos-nested-code.sh',
      'scripts/native-payload-input.mjs',
      'scripts/postprocess-appimage.mjs',
      'scripts/prepare-core-identity.mjs',
      'scripts/release-assets.mjs',
      'scripts/verify-apk.mjs',
      'scripts/verify-packaging.mjs',
      'scripts/verify-wry-keep-rules.mjs',
    ]);
    // 哨兵②：android.yml 必须真有脚本被抓到（它整个漏掉时上面那张表由别的 workflow 填满，
    // 逐对断言就会对 Android 侧恒真）。
    expect(
      invoked.filter(([, workflow]) => workflow === 'android.yml').length,
      'android.yml 一条 `run: node scripts/…` 都没抓到 —— 抓取器或该 workflow 的形状变了',
    ).toBeGreaterThan(1);

    expect(invoked.filter(([, workflow]) => workflow === 'apple.yml').map(([script]) => script))
      .toContain('scripts/fetch-protoc.mjs');
    // Python orchestration owns the SDK → final carrier link; it is not a legacy
    // build-libbox shell invocation and cannot disappear behind the node scanner.
    const androidSource = stripLineComments(readFileSync(join(REPO_ROOT, '.github/workflows/android.yml'), 'utf8'), 'yaml');
    const appleSource = stripLineComments(readFileSync(join(REPO_ROOT, '.github/workflows/apple.yml'), 'utf8'), 'yaml');
    for (const action of ['prepare', 'build', 'verify']) {
      expect(androidSource).toContain(`python3 scripts/mobile-core/ci_android.py ${action}`);
    }
    for (const action of ['prepare', 'build', 'evidence']) {
      expect(appleSource).toContain(`python3 -B scripts/mobile-core/ci_apple.py ${action}`);
    }
    expect(classifyImpact(['scripts/mobile-core/ci_android.py']).android).toBe(true);
    expect(appleRequired(['scripts/mobile-core/ci_apple.py'])).toBe(true);

    for (const [script, workflow] of invoked) {
      const { signal, label } = requiredLeg[workflow];
      expect(
        workflow === 'apple.yml' || script === 'scripts/apple-ci-impact.mjs'
          ? appleRequired([script])
          : signal(classifyImpact([script])),
        `${workflow} 里真跑 ${script}，但改它不触发${label} —— 那条腿的一环没有合入前信号`,
      ).toBe(true);
    }
  });

  it('Android 登记表不是装饰：每条登记都必须真的点亮 Android 腿', () => {
    expect(Object.keys(ANDROID_IMPACT_SCOPES).length).toBeGreaterThan(10);
    for (const [key, decision] of Object.entries(ANDROID_IMPACT_SCOPES)) {
      expect(
        classifyImpact([probeOf(key)]).android,
        `ANDROID_IMPACT_SCOPES['${key}'] 登记了，但分类器不给它点亮 Android 腿`,
      ).toBe(true);
      expect(
        decision.why.length,
        `ANDROID_IMPACT_SCOPES['${key}'] 缺少可读的判据（why）`,
      ).toBeGreaterThan(10);
    }
    // 反向对照：没有它，上面那条会被「android 恒真」满足。
    expect(classifyImpact(['ui/src/App.tsx']).android).toBe(false);
    expect(classifyImpact(['src-tauri/tauri.linux.conf.json']).android).toBe(false);
  });
});
