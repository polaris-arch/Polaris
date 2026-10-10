/**
 * GitHub 下载加速（gh-proxy 类前缀）的预设清单：设置页用。
 * 加速默认关闭（''=直连，可选开关）。URL 改写与下载由 Rust 侧实现。
 */

export const GH_PROXY_PRESETS = [
  'https://gh-proxy.org/',
  'https://v4.gh-proxy.org/',
  'https://cdn.gh-proxy.org/',
] as const;
