/**
 * 网络检测卡的**服务表**（品牌名 + 分组序）。
 *
 * ⚠️ **这是一份登记在案的重复。** 桌面同一张表住在
 * `ui/src/components/screens/home/HomeScreen.tsx:103` 的 `UNLOCK_SVC_META` —— 一个模块内私有、
 * **未导出**的 `const`，且文件是 `.tsx`（契约 A1 + 节点屏那道门都禁止移动端引入桌面 `.tsx`）。
 * 于是移动端只有两条路：抄一份，或者先做一次桌面侧改动把它上移到 `contracts/unlock-detection.ts`。
 * 本批的硬约束是**桌面屏只读不改**，故取前者，并把「上移到 contracts」登记为后续线
 * （见 `~/docs/polaris/design/polaris-mobile-screen-home-2026-09-04.md` §后续）。
 *
 * 重复的具体代价：上线 / 停飞一个服务要改两处。**门把这条代价变成可见的**——
 * 下面这张表的 id 集合与 `ENABLED_SERVICE_IDS` 逐项对拍，桌面那边改了上线集而这边没跟，当场红。
 *
 * # 分组序必须保留
 *
 * 桌面按 `UNLOCK_SVCS.grp` 分成 AI 行（chatgpt / claude / gemini）与流媒体行
 * （netflix / disney / tiktok / spotify）。移动端可以走一个连续的两列网格，但**组序要保留**
 * （`unlock-detection.md`「Data」）：同类相邻是这张卡唯一的分组信号。
 *
 * # `grok` 不在这里，且不许加回来
 *
 * 它实现了但**刻意不上线**：判据弱、地区封锁规则集为空 ⇒ 在受限地区会报一个假绿。
 * `ENABLED_SERVICE_IDS` = `SERVICE_IDS` 去掉 grok，本表照它取。
 */
import { ENABLED_SERVICE_IDS, type ServiceId } from '@/contracts/unlock-detection';
import { BRAND_SVGS } from '@/components/brand-icons/brand-svgs';

export interface UnlockServiceMeta {
  readonly id: ServiceId;
  /** 品牌名。品牌名是专名，不进 i18n。 */
  readonly name: string;
  readonly group: 'ai' | 'stream';
}

/** 与桌面 `UNLOCK_SVC_META` 逐条同形（去掉 grok 与它的弱检测提示键）。 */
const META: readonly UnlockServiceMeta[] = [
  { id: 'chatgpt', name: 'ChatGPT', group: 'ai' },
  { id: 'claude', name: 'Claude', group: 'ai' },
  { id: 'gemini', name: 'Gemini', group: 'ai' },
  { id: 'netflix', name: 'Netflix', group: 'stream' },
  { id: 'disney', name: 'Disney+', group: 'stream' },
  { id: 'tiktok', name: 'TikTok', group: 'stream' },
  { id: 'spotify', name: 'Spotify', group: 'stream' },
];

/**
 * 上线集，按组序。
 *
 * **服务条数是数据，不是版式**（`unlock-detection.md`）：七项排两列会让最后一行只剩一个，
 * 那是对的，不是要设计掉的 bug。grok 哪天上线就是八项，网格必须容得下任意条数。
 */
export const UNLOCK_SERVICES: readonly UnlockServiceMeta[] = META.filter((m) =>
  ENABLED_SERVICE_IDS.includes(m.id),
);

/**
 * 品牌徽章图源。
 *
 * 与桌面 `brandIcon()` 同一份 `BRAND_SVGS`（那是 `.ts`，可复用；解析器住在 `.tsx` 故不复用）。
 * 渲染成 `<img data:svg>` 而不是内联 SVG：品牌 viewBox 千差万别，`object-fit` 对**内联 svg 在
 * WebKit 不可靠**，对 replaced 元素才稳定 —— 这条是桌面踩过的，逐字沿用。
 */
export function unlockBadgeSrc(id: ServiceId): string | null {
  const body = (BRAND_SVGS as Record<string, string | undefined>)[id];
  return body === undefined ? null : `data:image/svg+xml,${encodeURIComponent(body)}`;
}
