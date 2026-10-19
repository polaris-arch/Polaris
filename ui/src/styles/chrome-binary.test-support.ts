// Browser location only: importing this helper does not probe or launch a browser.
import { existsSync, statSync } from 'node:fs';

/** 按顺序探这些路径；一个都没有就抛。**没有环境变量后门** —— 后门等于一个静默的 skip。 */
export const CHROME_CANDIDATES: readonly string[] = [
  '/usr/bin/google-chrome',
  '/usr/bin/google-chrome-stable',
  '/usr/bin/chromium',
  '/usr/bin/chromium-browser',
  '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome',
];

/** 纯函数形态，便于对「缺席 ⇒ 抛」这条本身做正向对照（见 `css-oracle.test.ts` ⑥）。 */
export function resolveChromeBinary(candidates: readonly string[] = CHROME_CANDIDATES): string {
  for (const c of candidates) if (existsSync(c) && statSync(c).isFile()) return c;
  throw new Error(
    'CSS 裁判找不到浏览器 —— 探过：\n' +
      candidates.map((c) => `    ${c}`).join('\n') +
      '\n本模块**不 skip**：「没检查」与「检查通过」的输出必须可区分。装一个 Chrome，或改这份候选表。',
  );
}

