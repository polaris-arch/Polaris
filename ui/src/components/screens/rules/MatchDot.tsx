/**
 * 网络场景命中态圆点（N4）：桌面场景列表行 / 规则徽标与移动端二级页 / 规则行共用。
 *
 * 为什么不住在 `NetworkProfilePanel.tsx`：那份文件 import 桌面弹窗外壳，移动端入口不许 import
 * （契约 A1，同 `network-profile-probes.ts` 头注）。本文件只依赖 `domain/network-profile` 的类名表，
 * 两端各自的样式表给 `.dot` 这组类上色（桌面 `components.css`，移动端 `rules-screen.css` 限定在 `.mr-row` 内）。
 *
 * 形状 + 颜色 + 文案（`label`）三路区分命中 / 未命中 / 未知：未知是空心圆，不只靠颜色。
 */

import { PROFILE_MATCH_DOT_CLASS, type ProfileMatch } from '@/domain/network-profile';

export function MatchDot({ match, label }: { match: ProfileMatch; label: string }) {
  return <span className={PROFILE_MATCH_DOT_CLASS[match]} role="img" aria-label={label} />;
}
