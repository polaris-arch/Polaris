/**
 * AppAddDialog 图标设定即缓存 —— iconProxySrc 的路由纯逻辑单测（vitest, node 环境）。
 *
 * 隐私第一性验证：本地缓存 ref（自定义应用设定图标后 preset.iconUrl 持有形态）必须原样透传路由到
 * 缓存服务（读本地副本，渲染零出站），绝不被二次包裹成远端代理 ref。
 * 真机行为（scheme handler 读盘 / 下载）在 Rust 层单测覆盖（`src-tauri/src/icon_cache.rs`）。
 */

import { describe, expect, it } from 'vitest';
import { ICON_PROXY_SCHEME, iconProxySrc } from '../../domain/icon-proxy';

describe('iconProxySrc — 本地缓存 ref 与远端 URL 路由', () => {
  it('本地缓存 ref 原样透传（渲染零网络，不二次包裹）', () => {
    const cacheRef = `${ICON_PROXY_SCHEME}://c/custom-abc.png`;
    expect(iconProxySrc(cacheRef)).toBe(cacheRef);
  });

  it('已是远端代理 ref 也原样透传（防重复包裹）', () => {
    const remoteRef = `${ICON_PROXY_SCHEME}://i/https%3A%2F%2Fa.com%2Fx.png`;
    expect(iconProxySrc(remoteRef)).toBe(remoteRef);
  });

  it('http(s) URL 包成远端代理 ref（预览 / 未缓存路径）', () => {
    const url = 'https://cdn.example.com/x.png';
    expect(iconProxySrc(url)).toBe(`${ICON_PROXY_SCHEME}://i/${encodeURIComponent(url)}`);
  });

  it('空 / null / 非网络 URL 处理不变', () => {
    expect(iconProxySrc('')).toBe('');
    expect(iconProxySrc(null)).toBe('');
    expect(iconProxySrc(undefined)).toBe('');
    expect(iconProxySrc('data:image/png;base64,AAAA')).toBe('data:image/png;base64,AAAA');
  });
});
