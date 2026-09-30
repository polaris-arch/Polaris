import { afterEach, describe, expect, it } from 'vitest';
import { filterByExactRule, useExactRuleFilter } from './exact-rule-filter';
import { installMobileNavigator, navigateMobile } from '../navigate';

afterEach(() => useExactRuleFilter.getState().setExactRule(null));

describe('named rule group drill-down', () => {
  const rows = [
    { id: 'a', ruleGroupKey: 'named:流媒体', rule: 'domain=video.example', host: 'video.example' },
    { id: 'b', ruleGroupKey: 'named:流媒体', rule: 'ip_cidr=1.2.3.4/32', host: 'cdn.example' },
    { id: 'c', ruleGroupKey: 'policy:direct', rule: 'domain=video.example', host: 'video.other' },
    { id: 'd', ruleGroupKey: 'named:直连', rule: 'rule_set=x', host: 'intranet' },
  ];

  it('includes all raw conditions in one name and keeps ordinary search independent', () => {
    const named = filterByExactRule(rows, { key: 'named:流媒体', label: '流媒体' });
    expect(named.map((row) => row.id)).toEqual(['a', 'b']);
    expect(named.filter((row) => row.host.includes('cdn')).map((row) => row.id)).toEqual(['b']);
    expect(filterByExactRule(rows, { key: 'policy:direct', label: '直连' }).map((row) => row.id)).toEqual(['c']);
    expect(filterByExactRule(rows, null)).toEqual(rows);
  });

  it('sets the one-off drill-down before navigation, then clears it on ordinary navigation', () => {
    const seen: Array<[string, string | null]> = [];
    const uninstall = installMobileNavigator((destination) => {
      seen.push([destination, useExactRuleFilter.getState().exactRule?.key ?? null]);
    });
    try {
      expect(navigateMobile('connections', undefined, { ruleGroup: { key: 'named:流媒体', label: '流媒体' } })).toBe(true);
      expect(navigateMobile('settings')).toBe(true);
      expect(navigateMobile('connections')).toBe(true);
      expect(seen).toEqual([
        ['connections', 'named:流媒体'], ['settings', null], ['connections', null],
      ]);
    } finally {
      uninstall();
    }
  });
});
