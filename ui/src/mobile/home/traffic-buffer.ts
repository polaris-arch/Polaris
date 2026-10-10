/**
 * 首页流量图的**采样与成曲线**，纯函数。
 *
 * 规格：`~/docs/polaris/design/mobile-kit/component-specs/traffic-chart.md`「Sampling contract」
 * + `mobile-kit/data-contract.json#cards.traffic-chart.samplingContract`（2026-09-02 定案）。
 *
 * # 一条规则同时产出曲线点与它上面那个数
 *
 * **按秒差分 `TrafficStats.totalUpload` / `totalDownload`，窗口 60 点。**
 * 曲线上的点与卡片上印的当前速率是**同一个值**，故两者结构上不可能互相矛盾 ——
 * 这正是把采样写成一条规则而不是两条的理由。
 *
 * 三件被这条规则一并settle 的事（原文在 spec，此处只记结论与后果）：
 *  · **不读 `uploadSpeed` / `downloadSpeed`**：它们本身已是客户端在一个未言明的窗口上做的差分，
 *    而 relay 的聚合发射闸门是 250ms ⇒ 一秒里最多四帧。只留每秒最后一帧等于扔掉四分之三的测量。
 *  · **不按 250ms 采**：60 点铺在 358 宽的 compact 卡上约 6px 一点，平滑吃得下；240 点是过采样，
 *    且一秒重绘四次是拿电量换没人看得见的细节。
 *  · **绝不读 sing-box 的 `Status.uplink` / `downlink`**：那不是速率（订阅循环往里写的是每 tick 的
 *    字节增量，首帧恒零，server 的 tick 间隔不在线上）。
 *
 * # 计数器归零 = 新的一条生命线，不是负速率
 *
 * `totalUpload` / `totalDownload` 只在**一条核生命线内**单调（`contracts/types/runtime.ts:320` 的
 * 头注写死了这件事）。停核 / 换节点重启核之后它们从 0 重来，此时首个差分**不是速率**。
 * 处置是**丢掉它并重启缓冲**，不是画一根负的或一根冲天的尖峰 —— 后两者都是在断言一件没发生的事。
 */

/** 一个采样点：两条序列各一个 bytes/s。 */
export interface RateSample {
  readonly up: number;
  readonly down: number;
}

/** 60 点窗口（与 -60s / -45s / -30s / -15s / now 这条轴同源）。 */
export const TRAFFIC_WINDOW = 60;

/** 采样边界：累计到满一秒才出一个点。 */
export const TRAFFIC_INTERVAL_MS = 1000;

/**
 * 差分缓冲。`baseline` 是上一个**已消费**的累计值与它的时刻；
 * `null` = 还没有基线（冷启动，或刚被一次计数器归零重置过）。
 */
export interface TrafficBuffer {
  /** 最新在**尾**。长度 ≤ `TRAFFIC_WINDOW`。 */
  readonly samples: readonly RateSample[];
  readonly baseline: { readonly up: number; readonly down: number; readonly at: number } | null;
}

export const EMPTY_TRAFFIC_BUFFER: TrafficBuffer = { samples: [], baseline: null };

/**
 * 吃一帧累计值，返回新缓冲。
 *
 * 四条分支，每条对应一个真实形态：
 *  1. 无基线 ⇒ 只记基线，不出点（第一帧没有可差分的对象）。
 *  2. 累计值**变小** ⇒ 核重启了。清空样本 + 换基线，**不出点**（那个差分不是速率）。
 *  3. 距基线不足一秒 ⇒ 原样返回（引用不变，避免无谓重渲染）。
 *  4. 满一秒 ⇒ 出一个点，速率 = 增量 ÷ 实测 Δt（不是 ÷ 1000 —— 帧不会精准落在整秒上）。
 */
export function pushTotals(
  buf: TrafficBuffer,
  totalUp: number,
  totalDown: number,
  atMs: number,
): TrafficBuffer {
  const base = buf.baseline;
  if (base === null) {
    return { samples: buf.samples, baseline: { up: totalUp, down: totalDown, at: atMs } };
  }
  // 计数器归零：新生命线。样本一并清掉 —— 留着会把两条生命线的速率画进同一根曲线。
  if (totalUp < base.up || totalDown < base.down) {
    return { samples: [], baseline: { up: totalUp, down: totalDown, at: atMs } };
  }
  const dt = atMs - base.at;
  if (dt < TRAFFIC_INTERVAL_MS) return buf;
  const perSec = 1000 / dt;
  const next = [
    ...buf.samples,
    { up: (totalUp - base.up) * perSec, down: (totalDown - base.down) * perSec },
  ];
  return {
    samples: next.length > TRAFFIC_WINDOW ? next.slice(next.length - TRAFFIC_WINDOW) : next,
    baseline: { up: totalUp, down: totalDown, at: atMs },
  };
}

/**
 * Y 轴刻度上限。
 *
 * 取两条序列的峰值向上取到一个「读得出来」的整数档，并给一个下限 ——
 * 全零窗口若让上限也为 0，除零会让整条曲线变成 NaN 路径（SVG 里表现为**整条线消失**，
 * 而不是一条贴底的直线，那是最容易被读成「图坏了」的失效形态）。
 */
export function scaleMax(samples: readonly RateSample[]): number {
  let peak = 0;
  for (const s of samples) {
    if (s.up > peak) peak = s.up;
    if (s.down > peak) peak = s.down;
  }
  if (peak <= 0) return 1024; // 1 KiB/s 的空窗底座
  // 取 1/2/5 × 10^n 档：刻度只印上限与 0 两个数，档位读起来必须是整的。
  const mag = 10 ** Math.floor(Math.log10(peak));
  for (const step of [1, 2, 5, 10]) {
    if (peak <= mag * step) return mag * step;
  }
  return mag * 10;
}

/**
 * 采样点 → 平滑曲线的 SVG path（Catmull-Rom 转三次贝塞尔）。
 *
 * spec「Curve rendering」：原始点直线相连会在每个采样处留下肉眼可见的硬角，读作噪声而不是速率。
 * 平滑之后**必须夹取**：Catmull-Rom 的控制点会冲出原始极值，不夹取就会在 0 附近把曲线推到负值
 * （视觉上是穿到轴下面去），或在峰值处顶穿刻度上限。
 *
 * 点数 < 2 时返回空串 —— 一个点不成曲线，调用方据此渲染「无样本」态而不是画一个孤立的点。
 */
export function splinePath(
  values: readonly number[],
  width: number,
  height: number,
  max: number,
): string {
  const n = values.length;
  if (n < 2 || max <= 0) return '';
  const x = (i: number): number => (i / (n - 1)) * width;
  // 值域翻转：SVG 的 y 向下。夹取在**取点时**做一次，控制点再夹一次。
  const clampY = (v: number): number => Math.min(height, Math.max(0, height - (v / max) * height));
  const y = (i: number): number => clampY(values[Math.min(n - 1, Math.max(0, i))]);
  let d = `M ${x(0).toFixed(2)} ${y(0).toFixed(2)}`;
  for (let i = 0; i < n - 1; i += 1) {
    const p0 = { x: x(i - 1 < 0 ? 0 : i - 1), y: y(i - 1) };
    const p1 = { x: x(i), y: y(i) };
    const p2 = { x: x(i + 1), y: y(i + 1) };
    const p3 = { x: x(i + 2 > n - 1 ? n - 1 : i + 2), y: y(i + 2) };
    const c1x = p1.x + (p2.x - p0.x) / 6;
    const c1y = Math.min(height, Math.max(0, p1.y + (p2.y - p0.y) / 6));
    const c2x = p2.x - (p3.x - p1.x) / 6;
    const c2y = Math.min(height, Math.max(0, p2.y - (p3.y - p1.y) / 6));
    d += ` C ${c1x.toFixed(2)} ${c1y.toFixed(2)}, ${c2x.toFixed(2)} ${c2y.toFixed(2)}, ${p2.x.toFixed(2)} ${p2.y.toFixed(2)}`;
  }
  return d;
}
