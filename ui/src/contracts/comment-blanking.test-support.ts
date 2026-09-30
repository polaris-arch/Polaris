/**
 * 「把注释抹掉、行列不变」这一个概念，全仓一份。
 *
 * 需要它的门有三处，口径必须一样，否则同一段文本在两道门里一个算注释、一个算代码：
 *  · `mobile/wiring-completeness.test.ts` —— 控件面按行取材（`// 这颗按钮 disabled` 不许被算成
 *    一处写死禁用的控件），以及处置表锚的事实核对；
 *  · `mobile/screen-parity.test.ts` —— 四屏动作 / 数据块 / 数据槽三个取材面，以及豁免锚的核对；
 *  · 后来者。
 *
 * # 为什么是**逐行**的状态机，而不是整文件正则、也不是整文件词法分析
 *
 * 两版前身各自在真实源码上炸过，且都是**静默吞掉大段代码**（比漏判更坏：判据看起来在跑）：
 *
 *  · 整文件正则版（`/\*[\s\S]*?\*\/` 非贪婪）—— 字符串里的 `/*` 照样算注释起点。
 *    实测 `connections-screen.test.tsx:348` 有一句 `expect(strip(...['**\/x\/**']...))`，
 *    那个字符串里含 `/` 与 `*` 相邻，于是从它一路抹到第 463 行的块注释结束符，
 *    中间 115 行代码（含一条被锚指着的 `describe` 标题）全成空白。
 *  · 整文件状态机版（识别字符串、遇引号整段保留）—— 一个落单的撇号（中文行文、`don't`）
 *    会开出一段假字符串，一直吃到下一个撇号，之后的 `/**` 被当成块注释开头，同样整块抹白。
 *
 * 现在的实现把**字符串状态限制在一行之内**：失步最多影响那一行，不会跨行传播。
 * 三条真实形态因此都判对了：
 *  · `{/* 此处曾有 … *\/}` 这种**墓碑注释**照样抹掉（它才是锚要防的那一档）；
 *  · `['**\/x\/**']` 这类字符串里的 `/` `*` 不再是注释起点；
 *  · JSX 文本里的 `https://…` 不再被当成行注释起点（`//` 前一个字符是 `:` 就放过）。
 *
 * 🔴 **射程自曝**：不做真正的词法分析。跨行模板字符串的**续行**会按普通代码处理，
 * 那里的 `//` 或 `/*` 会被当成注释起点。这个方向是**保守**的：判据会少认几条，不会多认。
 * 少认的后果是「锚找不到 / 控件没进面」⇒ 门红 ⇒ 有人来看一眼；多认才是假绿。
 */

/** 把注释内容抹成空格，**行数与列数都不变**（下游要按行号定位）。 */
export function blankComments(source: string): string {
  let inBlock = false;
  return source
    .split('\n')
    .map((line) => {
      let out = '';
      let i = 0;
      /** 字符串状态**每行重置** —— 这是「失步不跨行」的全部机制。 */
      let quote: string | null = null;
      while (i < line.length) {
        if (inBlock) {
          const end = line.indexOf('*/', i);
          if (end === -1) {
            out += ' '.repeat(line.length - i);
            i = line.length;
          } else {
            out += ' '.repeat(end + 2 - i);
            i = end + 2;
            inBlock = false;
          }
          continue;
        }
        const c = line[i]!;
        const n = line[i + 1];
        if (quote !== null) {
          out += c;
          if (c === '\\') {
            out += line[i + 1] ?? '';
            i += 2;
            continue;
          }
          if (c === quote) quote = null;
          i += 1;
          continue;
        }
        if (c === '/' && n === '*') {
          inBlock = true;
          continue;
        }
        // `//` 前一个字符是 `:` ⇒ 那是 `https://`，不是注释（JSX 文本里的 URL 会走到这条）。
        if (c === '/' && n === '/' && line[i - 1] !== ':') {
          out += ' '.repeat(line.length - i);
          i = line.length;
          continue;
        }
        if (c === "'" || c === '"' || c === '`') quote = c;
        out += c;
        i += 1;
      }
      return out;
    })
    .join('\n');
}
