/**
 * 写腿的**绑定别名**解析 —— `lib/config-write-wiring.test.ts` 与
 * `mobile/write-failure-visibility.test.ts` 两道写路径门共用**同一份口径**。
 *
 * # 为什么必须有这层
 *
 * 两道门的判据面都按**字面调用形态**扫（`update({`、`saveConfig(`），而同一条写腿可以换绑定方式
 * 而语义分毫不变：
 *
 *  - ① `const { update } = useConfig(); update({ … })`   —— 字面形态，静态判据认得；
 *  - ② `const { update: u } = useConfig(); u({ … })`     —— **解构改名**；
 *  - ③ `const h = useConfig(); h.update({ … })`          —— **句柄成员访问**；
 *  - ④ `const save = useAppStore((s) => s.saveConfig); save({ … })` —— **选择器别名**。
 *
 * ②③④ 都能把整条写腿移出判据面而**没有任何门转红**，且都不是假想：
 *  - ② 实测发生过 —— 移动端「规则」屏 `const { config: dnsConfig, update: updateConfig } = useConfig()`，
 *    DNS 资源两个开关整条写腿在 `config-write-wiring` 的判据面之外；
 *  - ③ 是协调者独立变异 P2 打出来的真洞 —— 首页 `const cfgHandle = useConfig();`
 *    `void cfgHandle.update({ proxyModeType: v })` 脱开 `runWrite`，跨屏门 / T3 / 屏级门
 *    **三道全绿**，而一次配置写失败被彻底吞掉；
 *  - ④ 今天**零实例**（树上 30 处 `use*Store((s) => s.M)` 绑定全部与 `M` 同名，见下方「已验面」），
 *    形态却与 ②③ 同类 ⇒ 一并纳入，并由两道门各自的**合成源码正反对照**钉住（真实代码上空跑的
 *    判据必须自带对照，否则它是绿的还是死的分不出来）。
 *
 * # 已验面（2026-09-05，`ui/src/` 全树，排除 `*.test.*`）
 *
 * | 形态 | 判法 | 实测 |
 * |---|---|---|
 * | `= useConfig()` 句柄 | `grep -E "=\s*useConfig\s*\(\s*\)"` | 6 处，**全是解构**，句柄形态 0 |
 * | `} = api.…` / `} = api;` 解构 | `grep -E "\}\s*=\s*api(\s*\.\|\s*;)"` | 0 |
 * | `const X = api.<域>;` 句柄别名 | `grep -E "(const\|let)\s+\w+\s*=\s*api\s*\.\s*[a-zA-Z]+\s*;"` | 0 |
 * | `= api.<域>.<方法>` 方法提取（不带调用括号） | `grep -E "=\s*api\s*\.\s*[a-zA-Z]+\s*\.\s*[a-zA-Z]+\s*[;,)]"` | 0 |
 * | `api.<域>[…]` 计算成员访问 | `grep -E "api\s*\.\s*[a-zA-Z]+\s*\["` | 0 |
 * | `} = use*Store` 解构 | `grep -E "\}\s*=\s*use[A-Za-z]*Store"` | 1 处，是 `useDialogTopLayerStore`（非写腿来源） |
 * | `const N = use*Store((s) => s.M)` 选择器别名 | 同上正则 + 逐行比对 N 与 M | 30 处，**N ≡ M，改名 0**（`saveConfig` / `switchServer` / `startProxy` / `stopProxy` / `loadConfig` / `openDialog` / `closeDialog`） |
 *
 * 结论：今天真实存在的只有 ①②；③④ 零实例但形态成立，故判据面纳入、并靠合成源码证明它报得出。
 *
 * # 本模块自身不进判据面
 *
 * 它住在 `src/test/`（与 `ts-compiler.ts` 同处，是门的适配层不是产品代码）。写法上刻意让写方法名
 * 只出现在字符串/模板里、绝不与 `(` 相邻，故 `config-write-wiring` 扫它得零命中 —— 那道门有一条
 * 自检钉着这件事（判据被自己污染是本仓踩过的形态）。
 */

/** `useConfig()` 漏斗句柄的来源形态。 */
const CONFIG_SOURCE = String.raw`useConfig\s*\(\s*\)`;
/** 漏斗方法名。拆成常量是为了让它不与 `(` 相邻出现在本文件里（见文件头「不进判据面」）。 */
const FUNNEL = 'update';

const IDENT = String.raw`[A-Za-z_$][\w$]*`;

/**
 * 解析本文件里由绑定别名产生的写腿判据。
 *
 * 返回的每条正则都**只**匹配那条腿的调用形态；与静态判据面不重叠：
 *  - 句柄成员访问 `h.update(` 与解构改名 `u(` 的接收者已由绑定语句证明就是漏斗本身，
 *    故**不再要求实参是对象字面量**（静态的裸 `update(` 需要那一条来与列表编辑器的同名局部函数消歧，
 *    这里不需要——收窄实参形状反而会放走 `u(next)` 这种同样真实的写法）；
 *  - 选择器别名只在**改了名**时产生（`N !== M`），同名绑定由静态判据面原样覆盖，不重复计数。
 *
 * @param src        去注释后的源码（偏移与原文逐字符对齐）
 * @param storeMethods 本门关心的 store 写方法名（两道门的集合不同：配置门只管配置写，
 *                     跨屏门还要管 `switchServer` / `startProxy` / `stopProxy` 这些会失败的运行态写）
 */
export function aliasWriteLegs(src: string, storeMethods: readonly string[]): RegExp[] {
  const legs: RegExp[] = [];

  // ③ 句柄成员访问：`const h = useConfig()` ⇒ `h.update(`
  for (const m of src.matchAll(
    new RegExp(String.raw`(?:const|let|var)\s+(${IDENT})\s*=\s*${CONFIG_SOURCE}`, 'g')
  )) {
    legs.push(new RegExp(String.raw`\b${m[1]}\s*\.\s*${FUNNEL}\s*\(`, 'g'));
  }

  // ② 解构改名：`const { update: u } = useConfig()` ⇒ `u(`
  for (const m of src.matchAll(new RegExp(String.raw`\{([^}]*)\}\s*=\s*${CONFIG_SOURCE}`, 'g'))) {
    const renamed = new RegExp(String.raw`\b${FUNNEL}\s*:\s*(${IDENT})`).exec(m[1]);
    if (renamed) legs.push(new RegExp(String.raw`(?<![.\w$])${renamed[1]}\s*\(`, 'g'));
  }

  // ④ 选择器别名：`const N = useAppStore((s) => s.M)`，N ≠ M 且 M 是本门关心的写方法 ⇒ `N(`
  const selector = new RegExp(
    String.raw`(?:const|let|var)\s+(${IDENT})\s*=\s*use${IDENT}\s*\(\s*\(\s*${IDENT}\s*\)\s*=>\s*${IDENT}\s*\.\s*(${IDENT})\s*\)`,
    'g'
  );
  for (const m of src.matchAll(selector)) {
    if (m[1] !== m[2] && storeMethods.includes(m[2])) {
      legs.push(new RegExp(String.raw`(?<![.\w$])${m[1]}\s*\(`, 'g'));
    }
  }

  return legs;
}

/**
 * 合成夹具：**四种绑定形态各一条真写腿**，供两道门各自做正反对照。
 *
 * 放在这里而不是各门里逐份手抄：③④ 在真实代码上零实例，对照就是它们唯一的活体证明 ——
 * 两份手抄的对照会各自漂移，而漂移后没有任何东西会红。
 */
export const ALIAS_FIXTURE = [
  "const plain = useConfig();",
  "const { update } = useConfig();",
  "const { config: cfg, update: renamed } = useConfig();",
  "const saved = useAppStore((s) => s.saveConfig);",
  "const saveConfig = useAppStore((s) => s.saveConfig);",
  "void plain.update({ a: 1 });",
  "void update({ b: 2 });",
  "void renamed({ c: 3 });",
  "void saved({ d: 4 });",
  "void saveConfig({ e: 5 });",
].join('\n');

/** `ALIAS_FIXTURE` 上由**别名解析**（不含静态判据面）应当抓到的调用点，按出现顺序。 */
export const ALIAS_FIXTURE_LEGS = ['plain.update(', 'renamed(', 'saved('] as const;
