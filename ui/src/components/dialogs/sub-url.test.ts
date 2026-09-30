/**
 * 订阅 URL 语法门的**等价门**（2026-09-06 三条复审同时点名的 major）。
 *
 * 缺陷形态：这条判据原本是 `SubDialog.tsx` 里的一个局部闭包，移动端表单落地时被**重实现**成了
 * 正则 `/^https?:\/\/\S+$/i`，注释却声称「与桌面同一条」。两侧双向不等价 —— 桌面能加的订阅
 * 链接在手机上加不进去（错误提示只有一句「URL 格式不正确」，用户没有任何办法绕过去），
 * 而移动端又会放行一个 `new URL` 都解析不了的串。
 *
 * 本门两条**成对**：
 *  ① 行为腿 —— 那四组实测输入逐条钉住 `isSubscriptionUrl` 的答案（正反都有，不只是「不许崩」）；
 *  ② 接线腿 —— 两个客户端**都**消费这一份，且谁都没有再手写一条 `^https?:` 正则。
 * 只有 ① 的话，谁再抄一份出去它一样绿（那正是这次的形态）。
 */

import { describe, expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { isSubscriptionUrl } from './sub-url';

const HERE = dirname(fileURLToPath(import.meta.url));
const read = (abs: string): string => readFileSync(abs, 'utf8');
/** 剥注释：本门自己在注释里写着要断言其不存在的那条正则，不剥会自污染。 */
const strip = (src: string): string =>
  src.replace(/\/\*[\s\S]*?\*\//g, ' ').replace(/^[ \t]*\/\/[^\n]*/gm, ' ');

describe('① 行为腿：那四组实测输入的答案钉死', () => {
  it('两端此前给出相反结论的四组输入，今天只有一个答案', () => {
    /* 前三组：`new URL` 认得、旧正则不认 —— 用户从聊天窗口粘过来的链接常常长这样。 */
    expect(isSubscriptionUrl('https:example.com'), '省略 `//` 的合法写法被判非法').toBe(true);
    expect(
      isSubscriptionUrl('https://example.com/sub?token=a b'),
      'query 里带一个未编码空格就加不进去',
    ).toBe(true);
    expect(isSubscriptionUrl('https://example.com/一 二'), '路径里的非 ASCII 被判非法').toBe(true);
    /* 第四组反过来：旧正则放行一个连解析都解析不动的串，直送 `subscription.preview`。 */
    expect(isSubscriptionUrl('https://['), '解析不动的串被放行进了后端').toBe(false);
  });

  it('协议白名单：只认 http(s)，其余一律否（`new URL` 对它们全都解析得动）', () => {
    expect(isSubscriptionUrl('http://example.com/sub')).toBe(true);
    expect(isSubscriptionUrl('HTTPS://EXAMPLE.COM/sub'), '大写协议名').toBe(true);
    for (const bad of [
      'javascript:alert(1)',
      'file:///etc/passwd',
      'data:text/plain,aaa',
      'ftp://example.com/sub',
      'ss://YWVzLTI1Ni1nY206cHdk@1.2.3.4:8388',
    ]) {
      expect(isSubscriptionUrl(bad), `${bad} 被当成订阅 URL 放行了`).toBe(false);
    }
  });

  it('空 / 纯空白 / 不是 URL 一律否；两端的空白处理同源（`trim` 在谓词里）', () => {
    expect(isSubscriptionUrl('')).toBe(false);
    expect(isSubscriptionUrl('   \t\n ')).toBe(false);
    expect(isSubscriptionUrl('example.com/sub'), '没有 scheme').toBe(false);
    expect(isSubscriptionUrl('  https://example.com/sub  '), '前后空白应被 trim 掉').toBe(true);
  });
});

describe('② 接线腿：两个客户端消费的是同一份（抄一份 ⇒ 红）', () => {
  const CONSUMERS = [
    ['components/dialogs/SubDialog.tsx', join(HERE, 'SubDialog.tsx')],
    ['mobile/forms/SubFormPanel.tsx', join(HERE, '..', '..', 'mobile', 'forms', 'SubFormPanel.tsx')],
  ] as const;

  it('自检：两份源码都读得到且有量级（读空会让下面每条恒绿）', () => {
    for (const [name, path] of CONSUMERS) {
      expect(strip(read(path)).length, `${name} 源码读取失败`).toBeGreaterThan(1000);
    }
  });

  it.each(CONSUMERS)('%s：import 了共用谓词并真的在用它', (name, path) => {
    const src = strip(read(path));
    expect(src, `${name} 没有 import 共用的 \`isSubscriptionUrl\``).toMatch(
      /import\s*\{[^}]*isSubscriptionUrl[^}]*\}\s*from\s*'[^']*sub-url'/,
    );
    /* 提交与预检两处都要走它：只 import 不用等于门在但没牙。 */
    const uses = src.match(/isSubscriptionUrl\(/g) ?? [];
    expect(uses.length, `${name} 里 \`isSubscriptionUrl\` 的调用点少于两处（预检 + 提交）`)
      .toBeGreaterThanOrEqual(2);
  });

  it('🔴 两端都不许再手写一条 http(s) 语法正则（这次的复发形态就是它）', () => {
    for (const [name, path] of CONSUMERS) {
      const src = strip(read(path));
      expect(src, `${name} 里又出现了一条自写的 http(s) 正则 —— 两份判据必然漂开`).not.toMatch(
        /\/\^\\?\(?https\??/i,
      );
      expect(src, `${name} 里又出现了一次裸 \`new URL(\` 语法判定`).not.toContain('new URL(');
    }
    // 反向自检：同一条正则对着旧写法确实报得出来（否则上面两条是空话）。
    expect(/\/\^\\?\(?https\??/i.test("const ok = /^https?:\\/\\/\\S+$/i.test(v);")).toBe(true);
  });
});
