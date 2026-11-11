// Browser location only: importing this helper does not probe or launch a browser.
import { existsSync, statSync } from 'node:fs';
import { homedir } from 'node:os';
import { win32 } from 'node:path';

/** 只探标准安装位置；一个都没有就抛，不以自定义浏览器路径或 skip 绕过缺席。 */
export function chromeCandidates(platform: NodeJS.Platform = process.platform): readonly string[] {
  if (platform === 'win32') {
    // Standard installation roots only; no caller-selected browser executable.
    const roots = [
      process.env.ProgramFiles || 'C:\\Program Files',
      process.env['ProgramFiles(x86)'] || 'C:\\Program Files (x86)',
      process.env.LOCALAPPDATA || win32.join(homedir(), 'AppData', 'Local'),
    ];
    return roots.flatMap((root) => [
      win32.join(root, 'Google', 'Chrome', 'Application', 'chrome.exe'),
      win32.join(root, 'Microsoft', 'Edge', 'Application', 'msedge.exe'),
    ]);
  }
  return [
    '/usr/bin/google-chrome',
    '/usr/bin/google-chrome-stable',
    '/usr/bin/chromium',
    '/usr/bin/chromium-browser',
    '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome',
  ];
}

export const CHROME_CANDIDATES = chromeCandidates();

/** 纯函数形态，便于对「缺席 ⇒ 抛」这条本身做正向对照（见 `css-oracle.test.ts` ⑥）。 */
export function resolveChromeBinary(candidates: readonly string[] = CHROME_CANDIDATES): string {
  for (const c of candidates) if (existsSync(c) && statSync(c).isFile()) return c;
  throw new Error(
    'CSS 裁判找不到浏览器 —— 探过：\n' +
      candidates.map((c) => `    ${c}`).join('\n') +
      '\n本模块**不 skip**：「没检查」与「检查通过」的输出必须可区分。装一个 Chrome，或改这份候选表。',
  );
}

