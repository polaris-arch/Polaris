import { create } from 'zustand';
import type { ConnRowVM } from './view-model';

/** 点击首页圆环时冻结的组身份与可读标题。 */
export interface RuleGroupFilter { readonly key: string; readonly label: string }

interface ExactRuleFilterState {
  exactRule: RuleGroupFilter | null;
  setExactRule: (rule: RuleGroupFilter | null) => void;
}

export const useExactRuleFilter = create<ExactRuleFilterState>((set) => ({
  exactRule: null,
  setExactRule: (exactRule) => set({ exactRule }),
}));

/** 组身份精确相等，独立于普通文本搜索。 */
export function filterByExactRule<T extends Pick<ConnRowVM, 'ruleGroupKey'>>(
  rows: readonly T[],
  exactRule: RuleGroupFilter | null,
): T[] {
  return exactRule === null ? [...rows] : rows.filter((row) => row.ruleGroupKey === exactRule.key);
}
