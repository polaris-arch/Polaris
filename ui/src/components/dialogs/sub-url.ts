/**
 * 订阅 URL 的**语法门** —— 平台无关纯谓词，桌面 `SubDialog` 与移动端 `SubFormPanel` 共用一份。
 *
 * # 为什么要有这份文件（2026-09-06 三条复审同时点名）
 *
 * 这条判据此前是 `SubDialog.tsx` 里的一个局部闭包，移动端表单落地时被**重实现**成了正则
 * `/^https?:\/\/\S+$/i`，而注释声称「与桌面同一条」。两侧**双向**不等价，实测四组输入结论相反：
 *
 * | 输入 | `new URL` 版（桌面） | 正则版（移动） |
 * |---|---|---|
 * | `https:example.com`（省略 `//`）| true | false |
 * | `https://e.com/sub?token=a b`（未编码空格）| true | false |
 * | `https://e.com/一 二` | true | false |
 * | `https://[` | false | **true** |
 *
 * 前三行的后果是**同一条订阅链接在桌面能加、在手机上加不进去**（从聊天窗口粘过来的链接经常带
 * 一个未编码空格），而错误提示只有一句 `sub.errUrl`「URL 格式不正确」，用户没有任何办法绕过去 ——
 * 这正是本屏此前被诟病的「请在桌面端操作」原样复发。第四行反过来：正则版把一个 `new URL` 都
 * 解析不了的串放行进 `api.subscription.preview`。
 *
 * 收口成一个零 import 的 `.ts`（同一批把 `field-spec.ts` 从 `FieldSpec.tsx` 里拆出来的手法）：
 * 判据只有一处定义，两个客户端各自 import，「抄一份」这条复发路径从此不存在。
 */

/**
 * 是不是一条可用的订阅 URL —— **只认 http / https**。
 *
 * 判据用 `new URL()` 而不是正则：URL 的语法由平台的解析器拥有（它认得 `https:example.com`
 * 这种省略 `//` 的合法写法、认得未编码的空格与非 ASCII，也认得 `https://[` 这种非法写法），
 * 自己写一条正则等于给同一个问题造第二个答案，而两个答案必然在某处不等 —— 那正是本文件的由来。
 *
 * 协议白名单是**必要**的一半：`new URL` 对 `javascript:` / `file:` / `data:` 一律解析得动，
 * 而订阅拉取只走 HTTP(S)（后端 `subscription_preview` 用的是 reqwest）。
 */
export function isSubscriptionUrl(raw: string): boolean {
  const s = raw.trim();
  if (s === '') return false;
  try {
    const p = new URL(s);
    return p.protocol === 'http:' || p.protocol === 'https:';
  } catch {
    return false;
  }
}
