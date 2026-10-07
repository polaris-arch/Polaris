/**
 * 设置 → 网络（移动端）。桌面对照：`components/screens/settings/SettingsNetwork.tsx`（六块）。
 *
 * 保留三块、去掉三块，逐条都在 IA §4 有登记：
 *
 *  ✔ 出站网卡（直连 / 代理）
 *  ✔ 高级流量（Block QUIC / WebRTC 防泄露 / TLS 分片 / 节点故障切换 / 组网登录让位 / 切节点断旧连接 /
 *    节点变更自动重启）
 *  ✔ 更新与测速（更新走代理 + 测速端点）
 *
 *  ✘ **本地端口 + 允许局域网**（§4.16）。两者在移动端**都还没有真机结论**：Android 下绑在本地的端口
 *    在 `VpnService` 里是否仍对局域网可达是推断、未验证；iOS 侧 `includeAllNetworks` 还会再改一次
 *    局域网行为。画一个可能什么都不控制的开关，正是这份登记要防的静默失败。
 *    ⚠️ 这一条与 §2.4 的正文（「网络：保留本地端口」）**互相矛盾**；本批按 §4 的处置执行并把冲突
 *    抛给主会话（见设计文档的「留给主会话的分叉」）。
 *  ✘ **系统代理旁路**（§4.2）。移动端没有系统代理；旁路清单的另一个消费方 TUN `route_exclude`
 *    只在 `platform == "win32"` 那一支非空（`crates/config-engine/src/builder/inbounds.rs:234`），
 *    移动端落最后一支（只有回环）⇒ 这份清单在移动端一个消费方都没有。
 *  ✘ **管理面板 / 局域网访问管理 API**（§4.7 + 裁定 #9，用户 2026-09-04 亲自定「取严」）。
 *    这是**产品边界**不是能力上限：`ui-manifest.json#productScope.dashboard` 四个标志全 false。
 *    写在这里是为了让后来的人不要把这份缺席读成「做不到」而试图加回来。
 *  ✘ **终端代理环境变量**（§4.8）：没有用户可达的 shell。
 *
 * 判据侧：`MobileSettings.test.tsx` 对本页的开关集合做**逐值全等**断言（不是「不含某某」的纯否定），
 * 任何一个把管理 API / 局域网开关加回来的改动都会当场把那个集合撑大。
 */

import { useEffect, useRef, useState, type ReactElement } from 'react';
import { useTranslation } from 'react-i18next';
import { buildNetworkInterfaceChoices, useNetworkInterfaces } from '@/hooks/use-network-interfaces';
import { useSpeedTestSchedule } from '@/hooks/use-speed-test-schedule';
import { globalPeriodicSpeedTestEnabled, parseConcurrency } from '@/domain/periodic-speed-test';
import type { SpeedTestMeteredPolicy } from '@/contracts/types';
import { MobileSelect, MobileSwitch, MobileTextInput, SettingsGroup, SettingsRow } from './SettingsChrome';
import type { MobileSettingsPageProps } from './settings-pages';

type WebRtcMode = 'off' | 'proxy' | 'block';

/** 与后端 `icon_cache.rs::is_http_url` 逐字同口径（只看前缀）。`new URL()` 会放行一批后端随即
 *  回落默认的写法，两侧口径分叉就变成「填了没生效还不报错」。 */
function isHttpUrl(value: string): boolean {
  const v = value.trim().toLowerCase();
  return v.startsWith('http://') || v.startsWith('https://');
}

/** 后端 `commands/speedtest.rs` 的 DEFAULT_SPEED_TEST_URL；留空即回落到它。 */
const DEFAULT_SPEED_TEST_URL = 'http://www.gstatic.com/generate_204';

export function NetworkPage({ config, update, commit }: MobileSettingsPageProps): ReactElement {
  const { t } = useTranslation();
  const interfaces = useNetworkInterfaces();
  const directInterface = config.networkInterfaces?.direct ?? '';
  const proxyInterface = config.networkInterfaces?.proxy ?? '';

  /* 测速端点：草稿 + 失焦提交。逐键写盘会让 `h` / `ht` / `htt` 这些中间态真的落进配置。
     外部改动（备份导入 / 另一处保存 → useConfig 静默重拉）要能回填草稿，但不能打断正在输入的用户，
     故用种子快照守卫：草稿 ≠ 上次种子 = 用户改过，保留。 */
  const [speedUrl, setSpeedUrl] = useState(config.speedTestUrl ?? '');
  const [speedUrlInvalid, setSpeedUrlInvalid] = useState(false);
  const seeded = useRef(config.speedTestUrl ?? '');
  useEffect(() => {
    const next = config.speedTestUrl ?? '';
    setSpeedUrl((cur) => (cur !== seeded.current ? cur : next));
    seeded.current = next;
  }, [config.speedTestUrl]);

  /* 周期测速的三个全局项。取值范围与缺省值由后端给出（并发上限随平台不同），不在前端另写一份：
     拿到之前计费策略不选中任何一档、并发不显示上限。 */
  const speedLimits = useSpeedTestSchedule()?.limits;
  const savedConcurrency =
    typeof config.speedTestConcurrency === 'number' ? config.speedTestConcurrency : undefined;
  const [concurrency, setConcurrency] = useState(String(savedConcurrency ?? ''));
  const [concurrencyInvalid, setConcurrencyInvalid] = useState(false);
  useEffect(() => {
    setConcurrency(String(savedConcurrency ?? ''));
  }, [savedConcurrency]);
  function commitConcurrency(): void {
    const next = parseConcurrency(concurrency, speedLimits);
    setConcurrencyInvalid(next === null);
    if (next === null || next === savedConcurrency) return;
    commit('speed-test-concurrency', update({ speedTestConcurrency: next }));
  }

  /** 行 id 由调用点**逐字给**，不由 `kind` 拼出来：拼出来的 id 判据面扫不到（模板串不是字面量），
   *  而「这一行的写失败挂在哪一行上」正是要能被逐条对拍的东西。 */
  function setInterface(rowId: string, kind: 'direct' | 'proxy', value: string): void {
    const next = { ...(config.networkInterfaces ?? {}) };
    if (value) next[kind] = value;
    else delete next[kind];
    commit(rowId, update({ networkInterfaces: Object.keys(next).length > 0 ? next : undefined }));
  }

  function commitSpeedTestUrl(): void {
    const v = speedUrl.trim();
    if (v && !isHttpUrl(v)) {
      setSpeedUrlInvalid(true);
      return;
    }
    setSpeedUrlInvalid(false);
    const next = v || undefined;
    setSpeedUrl(v);
    if (next === (config.speedTestUrl ?? undefined)) return;
    commit('speed-test-url', update({ speedTestUrl: next }));
  }

  function interfaceOptions(current: string): ReactElement[] {
    return buildNetworkInterfaceChoices(interfaces.items, current, {
      defaultLabel: t('settings.network.interfaceAuto'),
      unavailable: (value) => t('settings.network.interfaceUnavailable', { name: value }),
      down: t('settings.network.interfaceDown'),
    }).map((option) => (
      <option key={option.value || 'auto'} value={option.value} disabled={option.disabled}>
        {option.label}
      </option>
    ));
  }

  /**
   * 开关行的公用装配（本页七个开关形状完全一致，逐个手抄只会长出漂移）。
   *
   * `patch` 收的是**发起写之后的那个 promise**（调用点仍逐字写着 `update({ … })`），
   * 由这里统一挂上 `commit(id, …)` ⇒ 七行不可能有哪一行忘了接失败回显。
   */
  function toggleRow(
    id: string,
    labelKey: string,
    descKey: string,
    checked: boolean,
    patch: (v: boolean) => Promise<void>,
    first?: boolean,
  ): ReactElement {
    return (
      <SettingsRow
        first={first}
        id={id}
        label={t(labelKey)}
        desc={t(descKey)}
        control={
          <MobileSwitch
            checked={checked}
            ariaLabel={t(labelKey)}
            onChange={(v) => commit(id, patch(v))}
          />
        }
      />
    );
  }

  return (
    <>
      <SettingsGroup header={t('settings.network.interfaceBlock')}>
        <SettingsRow
          first
          stacked
          id="direct-interface"
          label={t('settings.network.directInterface')}
          desc={t('settings.network.directInterfaceDesc')}
          problem={interfaces.failed ? t('settings.network.interfaceListFailed') : undefined}
          control={
            <MobileSelect
              value={directInterface}
              ariaLabel={t('settings.network.directInterface')}
              disabled={interfaces.loading && interfaces.items.length === 0}
              onChange={(v) => setInterface('direct-interface', 'direct', v)}
            >
              {interfaceOptions(directInterface)}
            </MobileSelect>
          }
        />
        <SettingsRow
          stacked
          id="proxy-interface"
          label={t('settings.network.proxyInterface')}
          desc={t('settings.network.proxyInterfaceDesc')}
          control={
            <MobileSelect
              value={proxyInterface}
              ariaLabel={t('settings.network.proxyInterface')}
              disabled={interfaces.loading && interfaces.items.length === 0}
              onChange={(v) => setInterface('proxy-interface', 'proxy', v)}
            >
              {interfaceOptions(proxyInterface)}
            </MobileSelect>
          }
        />
      </SettingsGroup>

      <SettingsGroup header={t('settings.network.advancedTraffic')}>
        {toggleRow(
          'block-quic',
          'settings.advanced.blockQuic',
          'settings.network.blockQuicDescFull',
          !!config.blockQuic,
          (v) => update({ blockQuic: v }),
          true,
        )}
        {/* WebRTC 防泄露只在 TUN 下有义。桌面靠 `.webrtc-row.disabled` 的 CSS 门控 + 一行说明，
            那行说明（`webrtcTunOnlyNote`）**移动端不出现**：Android 恒 TUN（接管方式不是一个选项，
            见 `mobile/home/MobileHomeScreen.tsx` 的 `MOBILE_TAKEOVER`）⇒ 前提恒成立，那句话既永远
            不会显示，措辞还叫用户「在主页把接管方式切到 TUN」—— 主页已经没有那颗控件了。 */}
        <SettingsRow
          stacked
          id="webrtc-leak"
          label={t('settings.network.webrtcLeakProtection')}
          desc={t('settings.network.webrtcLeakDesc')}
          control={
            <MobileSelect
              value={config.webrtcLeakProtection ?? 'off'}
              ariaLabel={t('settings.network.webrtcLeakProtection')}
              onChange={(v) => commit('webrtc-leak', update({ webrtcLeakProtection: v as WebRtcMode }))}
            >
              <option value="off">{t('settings.network.webrtcLeakOff')}</option>
              <option value="proxy">{t('settings.network.webrtcLeakProxy')}</option>
              <option value="block">{t('settings.network.webrtcLeakBlock')}</option>
            </MobileSelect>
          }
        />
        {toggleRow(
          'tls-fragment',
          'settings.advanced.tlsFragment',
          'settings.advanced.tlsFragmentDesc',
          !!config.tlsFragment,
          (v) => update({ tlsFragment: v }),
        )}
        {toggleRow(
          'auto-switch-node',
          'settings.advanced.autoSwitchNode',
          'settings.advanced.autoSwitchNodeDesc',
          !!config.autoSwitchNode,
          (v) => update({ autoSwitchNode: v }),
        )}
        {toggleRow(
          'mesh-login-fallback',
          'settings.network.meshLoginFallback',
          'settings.network.meshLoginFallbackDesc',
          config.meshLoginFallbackDirect !== false,
          (v) => update({ meshLoginFallbackDirect: v }),
        )}
        {toggleRow(
          'interrupt-on-switch',
          'settings.network.interruptOnSwitch',
          'settings.network.interruptOnSwitchDesc',
          config.interruptConnectionsOnSwitch !== false,
          (v) => update({ interruptConnectionsOnSwitch: v }),
        )}
        {toggleRow(
          'restart-on-node-change',
          'settings.network.restartOnNodeChange',
          'settings.network.restartOnNodeChangeDesc',
          !!config.restartOnNodeChange,
          (v) => update({ restartOnNodeChange: v }),
        )}
      </SettingsGroup>

      <SettingsGroup header={t('settings.network.updateAndSpeedTest')}>
        {toggleRow(
          'main-session-via-proxy',
          'settings.advanced.mainSessionViaProxy',
          'settings.advanced.mainSessionViaProxyDesc',
          config.mainSessionViaProxy !== false,
          (v) => update({ mainSessionViaProxy: v }),
          true,
        )}
        <SettingsRow
          stacked
          id="speed-test-url"
          label={t('settings.network.speedTestUrl')}
          desc={t('settings.network.speedTestUrlDesc')}
          problem={speedUrlInvalid ? t('settings.network.speedTestUrlInvalid') : undefined}
          control={
            <MobileTextInput
              mono
              inputMode="url"
              value={speedUrl}
              invalid={speedUrlInvalid}
              placeholder={DEFAULT_SPEED_TEST_URL}
              ariaLabel={t('settings.network.speedTestUrl')}
              onChange={(v) => {
                setSpeedUrl(v);
                if (speedUrlInvalid) setSpeedUrlInvalid(false);
              }}
              onCommit={commitSpeedTestUrl}
            />
          }
        />
        {toggleRow(
          'periodic-speed-test',
          'settings.network.periodicSpeedTest',
          'settings.network.periodicSpeedTestDesc',
          globalPeriodicSpeedTestEnabled(config.periodicSpeedTestEnabled, speedLimits),
          (v) => update({ periodicSpeedTestEnabled: v }),
        )}
        <SettingsRow
          id="speed-test-metered-policy"
          label={t('settings.network.meteredPolicy')}
          desc={t('settings.network.meteredPolicyDesc')}
          control={
            <MobileSelect
              value={config.speedTestMeteredPolicy ?? speedLimits?.meteredPolicyDefault ?? ''}
              ariaLabel={t('settings.network.meteredPolicy')}
              onChange={(v) =>
                commit(
                  'speed-test-metered-policy',
                  update({ speedTestMeteredPolicy: v as SpeedTestMeteredPolicy }),
                )
              }
            >
              <option value="reduced">{t('settings.network.meteredReduced')}</option>
              <option value="pause">{t('settings.network.meteredPause')}</option>
              <option value="normal">{t('settings.network.meteredNormal')}</option>
            </MobileSelect>
          }
        />
        <SettingsRow
          stacked
          id="speed-test-concurrency"
          label={t('settings.network.speedTestConcurrency')}
          desc={t('settings.network.speedTestConcurrencyDesc')}
          hint={
            speedLimits
              ? t('settings.network.speedTestConcurrencyMax', { max: speedLimits.concurrencyMax })
              : undefined
          }
          problem={
            !concurrencyInvalid
              ? undefined
              : speedLimits
                ? t('settings.network.speedTestConcurrencyInvalid', {
                    min: speedLimits.concurrencyMin,
                    max: speedLimits.concurrencyMax,
                  })
                : t('sub.speedTestIntervalInvalid')
          }
          control={
            <MobileTextInput
              inputMode="numeric"
              value={concurrency}
              invalid={concurrencyInvalid}
              placeholder={t('settings.network.speedTestConcurrencyAuto')}
              ariaLabel={t('settings.network.speedTestConcurrency')}
              onChange={(v) => {
                setConcurrency(v);
                if (concurrencyInvalid) setConcurrencyInvalid(false);
              }}
              onCommit={commitConcurrency}
            />
          }
        />
      </SettingsGroup>
    </>
  );
}
