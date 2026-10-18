import type { RuleSubject } from '@/domain/rules';
import { appendSubjectToRule, type RuleAppendTarget } from '@/components/dialogs/rule-append';
import { effectiveCollection, useAppStore } from '@/store/app-store';
import { useStagedConfigStore } from '@/store/staged-config-store';
import { stagingActive } from '@/lib/staged-config';

/** Resolve the owned picker intent at click time. Callers must pass base to backend CAS. */
export function prepareRuleAppend(target: RuleAppendTarget, subject: RuleSubject) {
  const state = useAppStore.getState();
  const staged = useStagedConfigStore.getState();
  const current = effectiveCollection('trafficRules', state.rules, staged.entries);
  const base = current.find(rule => rule.id === target.ruleId) ?? null;
  const next = base ? appendSubjectToRule(base, target, subject) : null;
  const stagingNow = stagingActive(staged.enabled,
    (state.proxyStatus?.running ?? false) || state.proxyStarting || state.proxyStopping,
    staged.entries.length);
  return { base, next, stagingNow };
}
