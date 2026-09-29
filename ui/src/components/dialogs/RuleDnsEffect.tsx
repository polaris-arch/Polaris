/**
 * DNS 效果那一格的**桌面呈现**。判据与状态全部住在纯 `.ts` 里，本文件只画字段。
 *
 * 两次下沉都不是整洁诉求，是依赖边：移动端规则表单要复用同一份提交腿 / 初值反解 / 候选构造 /
 * 动作联动，而从 `.tsx` 取它们会把整棵桌面下拉组件树拖进移动端闭包。桌面调用点一行未改
 * （两个符号在这里原样再导出）。
 */

import type { TFunction } from 'i18next';
import { BUILTIN_NETENV_DHCP_ID, probeReasonKey } from '@/domain/network-profile';
import { Csel } from './Csel';
import { dnsEffectLinkage, useRuleDnsEffect, type UseRuleDnsEffect } from './rule-effect-state';

export { splitDnsRecordLines } from './dns-action-options';
export { useRuleDnsEffect, type UseRuleDnsEffect };

interface RuleDnsEffectFieldsProps extends UseRuleDnsEffect {
  t: TFunction;
  touch: () => void;
}

/** DNS 效果字段（`.fld`：动作下拉 + hosts 兜底 + predefined 三段文本）。 */
export function RuleDnsEffectFields({
  t,
  touch,
  dnsAction,
  setDnsAction,
  setDnsAnswerMode,
  setDnsResolver,
  dnsActionGroups,
  dnsFallbackAction,
  setDnsFallbackAction,
  dnsFallbackGroups,
  dnsPredefinedRcode,
  setDnsPredefinedRcode,
  dnsPredefinedAnswer,
  setDnsPredefinedAnswer,
  dnsPredefinedNs,
  setDnsPredefinedNs,
  dnsPredefinedExtra,
  setDnsPredefinedExtra,
  netenvStatus,
}: RuleDnsEffectFieldsProps) {
  return (
    <div className="fld">
      <div className="fld-l">{t('rules.dnsEffect')}</div>
      <div className="card-sub">{t('rules.dnsEffectHint')}</div>
      <div style={{ display: 'grid', gap: 8, marginTop: 8 }}>
        <Csel
          id="rule-dns-action"
          ariaLabel={t('rules.dnsAction')}
          value={dnsAction}
          onChange={(value) => {
            setDnsAction(value);
            /* 联动判据住在 `rule-effect-state.ts#dnsEffectLinkage`，两端共用一份 ——
               少了它，「动作说返回 FakeIP、答案模式说给真实 IP」这种内部矛盾的效果会被存下去。 */
            const linked = dnsEffectLinkage(value);
            setDnsAnswerMode(linked.answerMode);
            setDnsResolver(linked.resolver);
            touch();
          }}
          options={dnsActionGroups}
        />
        {/* 已选中内置 DHCP 解析器、而它在本机不可用：值照常回显，原因写在下面（不悄悄清空）。 */}
        {dnsAction === `server:${BUILTIN_NETENV_DHCP_ID}` && netenvStatus?.available === false && (
          <div className="err-line">
            {t('rules.networkProfile.probeUnavailable', { reason: t(probeReasonKey(netenvStatus.reason)) })}
          </div>
        )}
        {dnsAction.startsWith('hosts:') && (
          <Csel
            id="rule-dns-hosts-fallback"
            ariaLabel={t('rules.dnsHostsFallback')}
            value={dnsFallbackAction}
            onChange={(value) => {
              setDnsFallbackAction(value);
              touch();
            }}
            options={dnsFallbackGroups}
          />
        )}
        {dnsAction === 'predefined' && (
          <div style={{ display: 'grid', gap: 8 }}>
            <Csel
              id="rule-dns-predefined-rcode"
              ariaLabel={t('rules.dnsPredefinedRcode')}
              value={dnsPredefinedRcode}
              onChange={(value) => {
                setDnsPredefinedRcode(value);
                touch();
              }}
              options={['NOERROR', 'FORMERR', 'SERVFAIL', 'NXDOMAIN', 'NOTIMP', 'REFUSED'].map(
                (value) => ({ value, label: value }),
              )}
            />
            {[
              ['answer', dnsPredefinedAnswer, setDnsPredefinedAnswer],
              ['ns', dnsPredefinedNs, setDnsPredefinedNs],
              ['extra', dnsPredefinedExtra, setDnsPredefinedExtra],
            ].map(([field, value, setValue]) => (
              <label key={field as string} style={{ display: 'grid', gap: 4 }}>
                <span className="card-sub">
                  {field === 'answer'
                    ? t('rules.dnsPredefinedAnswer')
                    : field === 'ns'
                      ? t('rules.dnsPredefinedNs')
                      : t('rules.dnsPredefinedExtra')}
                </span>
                <textarea
                  className="input mono"
                  rows={2}
                  value={value as string}
                  onChange={(event) => {
                    (setValue as (next: string) => void)(event.currentTarget.value);
                    touch();
                  }}
                  placeholder={t('rules.dnsPredefinedRecordsHint')}
                />
              </label>
            ))}
          </div>
        )}
      </div>
    </div>
  );
}
