/**
 * 「谁**调**了这个符号」的共用判据 —— 两道门（`mobile/app-wiring.test.tsx` ⑩ 与
 * `mobile/half-truth-facts.test.ts` C 组）共用同一份，避免口径漂移。
 *
 * # 为什么要单独一层：**声明行不是调用点**
 *
 * 第一版的正则是 `(?<![\w$])name\s*\(`，它把**函数声明行**也算成一个调用点 ——
 * `export function foo(` 与 `void foo(` 在那条正则下不可区分。后果不是理论上的：
 *
 *  · `startMobileAppWiring` 的定义与唯一调用点同在 `mobile/app-wiring.ts`。把那条
 *    `useEffect(() => startMobileAppWiring(...), [])` 整个删掉（＝移动端一条 app 级订阅都不挂，
 *    正是本批立项要修的原缺陷），旧正则仍会在定义那一行上命中一次 ⇒
 *    `expect(sites).not.toEqual([])` 照样通过。
 *  · `installMobileToastHost` / `reconcileEntityCaches` / `hydrateTailscaleStates` 同形。
 *
 * ⇒ 判据必须先把「声明」从「调用」里剔出去，否则**任何自产自销的符号都自满足**。
 *
 * # 射程自曝（别把绿读成「调用一定发生」）
 *
 *  · 只剔 `function <name>(` 这一种声明形态（含 `export` / `async` 前缀）。类方法简写
 *    （`onLifecycle(listener: …) {`）与接口成员**不在剔除面内** —— 本仓两个消费面
 *    （`ui/src/mobile/**` 生产源码）里这几条腿都不是那种写法，硬猜会把真调用点误删成零。
 *    真要扫到那种文件时，先加自检再扩这条，不许默默放宽。
 *  · 它是**行级词法**判据：证明「源码里有人写了这次调用」，不证明「运行期真的调到了」。
 *    后者要么行为驱动（`app-wiring.test.tsx` ①–⑦），要么 AST（同文件 ⑪ 的挂载面）。
 */

/** 剥注释，**保留行号与行数**（注释体换成等量空白）—— 下游要按行号报「跳过去看哪一行」。 */
export function stripComments(source: string): string {
  return source
    .replace(/\/\*[\s\S]*?\*\//g, (match) => match.replace(/[^\n]/g, ' '))
    .split('\n')
    .map((line) => line.replace(/(^|[^:])\/\/.*$/, '$1'))
    .join('\n');
}

/**
 * 调用形态。边界只排 `[\w$]`，**不排 `.`**：本仓这几条腿的真实写法全是成员调用
 * （`store().refreshProxyStatus()` / `api.proxy.onLifecycle(...)`），把点也排掉会让判据
 * 对着真源码报零 —— 一道恒红的门与没有门等价。
 */
const callPattern = (name: string): RegExp => new RegExp(`(?<![\\w$])${name}\\s*\\(`);

/** 声明形态（见头注：只认 `function <name>(`，前缀 `export` / `async` 可选）。 */
const declarationPattern = (name: string): RegExp =>
  new RegExp(`(?:^|[^\\w$.])(?:export\\s+)?(?:async\\s+)?function\\s+${name}\\s*\\(`);

/** 这一行上 `name` 是不是一次**调用**（声明行不算）。入参应当是已剥注释的源码行。 */
export function isCallSiteLine(name: string, line: string): boolean {
  return callPattern(name).test(line) && !declarationPattern(name).test(line);
}

/**
 * `文件 → 已剥注释源码` 的映射里，`name` 的全部调用点，形如 `path/to/file.ts:123`。
 * 行号来自剥注释**之后**的文本，故与原文一致（这正是 [`stripComments`] 保行的理由）。
 */
export function callSitesIn(sources: ReadonlyMap<string, string>, name: string): string[] {
  const hits: string[] = [];
  for (const [file, text] of sources) {
    text.split('\n').forEach((line, index) => {
      if (isCallSiteLine(name, line)) hits.push(`${file}:${index + 1}`);
    });
  }
  return hits;
}

/**
 * 谓词自检夹具：`[符号名, 源码行, 是不是调用点]`。**两个消费面各跑一遍** ——
 * 口径漂移的下场是两道门同时失明，而它们各自的断言都不会说话。
 *
 * 每一条都取自本仓真实的行（不是编出来的），前四组正是上面头注点名的那个自满足形态。
 */
export const CALL_SITE_FIXTURE: ReadonlyArray<readonly [string, string, boolean]> = [
  // 成员调用形态认得出（本仓这几条腿的真实写法）。
  ['refreshProxyStatus', '  void store().refreshProxyStatus();', true],
  // 名字被包在更长的标识符里不算。
  ['refreshProxyStatus', '  void myrefreshProxyStatus();', false],
  // 🔴 声明行**不算**调用点（本文件存在的全部理由）。
  ['startMobileAppWiring', 'export function startMobileAppWiring(t: WiringT): () => void {', false],
  ['startMobileAppWiring', '      startMobileAppWiring((key, vars) =>', true],
  ['installMobileToastHost', 'export function installMobileToastHost(push: ToastPush): () => void {', false],
  ['installMobileToastHost', '    const restore = installMobileToastHost(push);', true],
  ['reconcileEntityCaches', 'export function reconcileEntityCaches(config: UserConfig | null): void {', false],
  ['reconcileEntityCaches', '    reconcileEntityCaches(config);', true],
  // `async function` 前缀也是声明。
  ['reportIfFails', 'async function reportIfFails(op: () => Promise<unknown>): Promise<void> {', false],
  ['reportIfFails', '      void reportIfFails(() => api.system.openExternal(url), msg);', true],
];
