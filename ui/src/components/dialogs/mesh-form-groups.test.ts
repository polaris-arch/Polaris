import { describe, expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import {
  allFields,
  ND_SPEC,
  nodeFormGroups,
  PROTO_OPTIONS,
} from './node-spec';
import {
  meshTunnelDraftError,
} from './mesh-form-layout';

const readDialog = (name: string) =>
  readFileSync(fileURLToPath(new URL(`./${name}`, import.meta.url)), 'utf8');

const localeValue = (dict: unknown, key: string): unknown =>
  key.split('.').reduce<unknown>(
    (value, part) =>
      value && typeof value === 'object'
        ? (value as Record<string, unknown>)[part]
        : undefined,
    dict,
  );

describe('统一接入表单的信息架构', () => {
  it.each(['openconnect', 'openvpn-client', 'masque-client'] as const)('%s uses three task-oriented groups without duplicating fields', (protocol) => {
    const groups = ND_SPEC[protocol].groups;
    expect(groups?.map((group) => group.id)).toEqual(['basic', 'routing', 'advanced']);
    const keys = allFields(protocol).map((field) => field.k);
    expect(new Set(keys).size).toBe(keys.length);
  });

  it('所有协议表单字段都被统一语义分组完全覆盖，且没有重复', () => {
    for (const [protocol] of PROTO_OPTIONS) {
      const keys = nodeFormGroups(protocol).flatMap((group) => group.fields.map((field) => field.k));
      expect([...keys].sort(), protocol).toEqual(allFields(protocol).map((field) => field.k).sort());
      expect(new Set(keys).size, protocol).toBe(keys.length);
      const groups = nodeFormGroups(protocol);
      expect(groups[groups.length - 1]?.id, protocol).toBe('advanced');
    }
  });

  it('WARP 只保留一个可折叠高级区，不为内部协议字段强造路由分组', () => {
    const src = readDialog('WarpDialog.tsx');
    expect(src).toContain("title={t('node.formGroup.advanced')}");
    expect(src).not.toContain("title={t('node.formGroup.routing')}");
    for (const internalKey of ["k: 'route'", "k: 'allowedIPs'", "k: 'reserved'", "k: 'reverseMesh'"]) {
      expect(src).not.toContain(internalKey);
    }
  });

  it('required and JSON errors point to the group that can fix them', () => {
    expect(meshTunnelDraftError('openconnect', {},)).toEqual({ group: 'basic', key: 'required' });
    expect(meshTunnelDraftError('openvpn-client', { user: 'u', pwd: 'p', ovpnCa: 'CA', extraJson: '[]' }))
      .toEqual({ group: 'advanced', key: 'json' });
    expect(meshTunnelDraftError('openvpn-client', { user: 'u', pwd: 'p', ovpnCa: 'CA', extraJson: '{}' }))
      .toBeNull();
    // MASQUE 无必填项（Basic 可选、地址走通用校验），只剩透传袋的 JSON 形态门。
    expect(meshTunnelDraftError('masque-client', {})).toBeNull();
    expect(meshTunnelDraftError('masque-client', { extraJson: '[]' })).toEqual({ group: 'advanced', key: 'json' });
  });

  it('协议/WireGuard/Tailscale 复用同一页签原语，WARP 不强造页签', () => {
    for (const name of ['NodeDialog.tsx', 'WgDialog.tsx', 'TsSettingsDialog.tsx']) {
      const source = readDialog(name);
      expect(source, `${name} 未接入统一 FormTabs`).toContain('<FormTabs');
      expect(source, `${name} 页签切换不得写表单草稿`).toContain('onSelect={setFormTab}');
    }
    const warp = readDialog('WarpDialog.tsx');
    expect(warp).not.toContain('<FormTabs');
    expect(warp).toContain('<FormSection');
    expect(warp).toContain('collapsible');
  });

  it('Tailcat 三个密钥字段标记为 secret', () => {
    const secret = allFields('tailcat').filter((f) => (f.t === 'text' || f.t === 'textarea') && f.secret).map((f) => f.k);
    expect(secret.sort()).toEqual(['preSharedKey', 'privateKey', 'serverPublicKey']);
  });

  it('Tailcat DERP 字段按模式显隐：region 只见区域，customMap 加地图 URL，servers 只见服务器', () => {
    const shown = (derpMode: string) =>
      allFields('tailcat')
        .filter((f) => f.k.startsWith('derp') && f.k !== 'derpMode')
        .filter((f) => !f.when || f.when({ derpMode }))
        .map((f) => f.k);
    expect(shown('region')).toEqual(['derpRegion']);
    expect(shown('customMap')).toEqual(['derpRegion', 'derpMapUrl']);
    expect(shown('servers')).toEqual(['derpServers']);
    const mode = allFields('tailcat').find((f) => f.k === 'derpMode');
    expect(mode?.t === 'select' && mode.options.map(([v]) => v)).toEqual(['region', 'customMap', 'servers']);
  });

  it('订阅创建后先刷新配置再回调 onAdded', () => {
    const subOperation = readFileSync(
      fileURLToPath(new URL('./use-subscription-create-dialog-operation.ts', import.meta.url)),
      'utf8',
    );
    expect(subOperation.indexOf('await loadConfig(true);')).toBeLessThan(
      subOperation.indexOf('onAdded?.(activeOperation.result!.subscription.id);'),
    );
  });

  it('节点与组网接入表单只以五语言 locale 为文案真值，不保留内联默认值', () => {
    const formNames = [
      'MeshJoinDialog.tsx',
      'NodeDialog.tsx',
      'SubDialog.tsx',
      'TsSettingsDialog.tsx',
      'WarpDialog.tsx',
      'WgDialog.tsx',
    ];
    const nodeScreen = readFileSync(
      fileURLToPath(new URL('../screens/nodes/NodesScreen.tsx', import.meta.url)),
      'utf8',
    );
    for (const [name, source] of [
      ...formNames.map((name) => [name, readDialog(name)] as const),
      ['NodesScreen.tsx', nodeScreen] as const,
    ]) {
      expect(source, `${name} 仍有 t(key, 内联默认值)`).not.toMatch(
        /\bt\(\s*['"][A-Za-z0-9_.]+['"]\s*,\s*['"]/
      );
    }

    /* 🔴 取材面 2026-09-06 随字段表搬家一起挪：三张 FieldSpec 表从弹窗 `.tsx` 拆进了零 React 的
       `.ts`（`wg-spec` / `warp-spec` / `ts-spec`，两端共用）。读回弹窗会扫到 0 个键 ——
       下面那条「防空转」自检抓的正是这个形态，它报的是判据量错了地方，不是文案回退了。
       弹窗仍留在上面那个 `formNames` 循环里（那一条查的是 `t(key, 内联默认值)`，与表无关）。 */
    const fieldSources = ['wg-spec.ts', 'warp-spec.ts', 'ts-spec.ts'].map(readDialog).join('\n');
    expect(fieldSources, 'FieldSpec 不得保留中文 fallback 属性').not.toMatch(
      /\b(?:zh|hintZh|disabledHintZh)\s*:/,
    );

    const dynamicKeys = new Set(
      [...fieldSources.matchAll(/\b(?:label|hint|disabledHint):\s*'([A-Za-z0-9_.]+)'/g)]
        .map((match) => match[1]),
    );
    expect(dynamicKeys.size, '动态 FieldSpec 键扫描异常，防止测试空转').toBeGreaterThan(30);

    for (const locale of ['zh-CN', 'zh-TW', 'en-US', 'ru', 'fa']) {
      const dict = JSON.parse(
        readFileSync(
          fileURLToPath(new URL(`../../i18n/locales/${locale}.json`, import.meta.url)),
          'utf8',
        ),
      ) as unknown;
      for (const key of dynamicKeys) {
        expect(localeValue(dict, key), `${locale} 缺少动态表单键 ${key}`).not.toBeUndefined();
        expect(localeValue(dict, key), `${locale} 的动态表单键 ${key} 为空`).not.toBe('');
      }
    }
  });
});

describe('OpenVPN supported TLS controls and legacy repair visibility', () => {
  const visible = (draft: Record<string, string>) => allFields('openvpn-client')
    .filter((field) => !field.when || field.when(draft)).map((field) => field.k);
  it('offers TLS data choices by default, with no legacy cipher field on a new form', () => {
    expect(visible({})).toEqual(expect.arrayContaining(['ovpnDataCiphers', 'ovpnDataCiphersFallback']));
    expect(visible({})).not.toContain('cipher');
    expect(visible({ extraJson: '{"mode":"tls"}' })).not.toContain('cipher');
  });
  it('keeps imported incompatible cipher visible for correction in both TLS and malformed JSON drafts', () => {
    for (const extraJson of ['', '{"mode":"tls"}', 'invalid JSON']) {
      expect(visible({ extraJson, cipher: 'AES-256-GCM' })).toEqual(expect.arrayContaining(['cipher', 'ovpnDataCiphers', 'ovpnDataCiphersFallback']));
    }
  });
  it('distinguishes only an explicit static_key, without adding a mode or authentication option', () => {
    expect(visible({ extraJson: '{"mode":"static_key"}' })).toContain('cipher');
    expect(visible({ extraJson: '{"mode":"static_key"}' })).not.toContain('ovpnDataCiphers');
    expect(visible({ extraJson: '{"mode":"static_key"}' })).not.toContain('ovpnDataCiphersFallback');
    expect(allFields('openvpn-client').map((field) => field.k)).not.toContain('mode');
    expect(nodeFormGroups('openvpn-client').find((group) => group.id === 'basic')?.fields.map((field) => field.k))
      .toEqual(['user', 'pwd', 'ovpnCa', 'ovpnCert', 'ovpnKey']);
  });
});
