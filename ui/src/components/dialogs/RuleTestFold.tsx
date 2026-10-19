import { useEffect, useMemo, useState } from 'react';
import type { TFunction } from 'i18next';
import { computeTestMatchAsync, type Cond, type TestResult } from './rule-cond';
import { revealOnToggle } from '@/components/reveal';

function Chevron() {
  return (
    <svg className="rule-test-caret" viewBox="0 0 24 24" width={14} fill="none" stroke="currentColor" strokeWidth={1.8}>
      <path d="M6 9l6 6 6-6" />
    </svg>
  );
}

/** Bounded asynchronous preview; unavailable context is never reported as a match. */
export function useRuleTestFold(conds: readonly Cond[], logic: 'and' | 'or', test: string): TestResult {
  const query = useMemo(() => ({ conds, logic, test }), [conds, logic, test]);
  const [reply, setReply] = useState<{ query: typeof query; result: TestResult } | null>(null);
  useEffect(() => {
    let current = true;
    const timer = setTimeout(() => {
      void computeTestMatchAsync(query.conds, query.logic, query.test)
        .then((result) => { if (current) setReply({ query, result }); })
        .catch(() => { if (current) setReply({ query, result: 'untestable' }); });
    }, 200);
    return () => { current = false; clearTimeout(timer); };
  }, [query]);
  // Identity, rather than equal text, prevents an A→B→A edit from reviving an old result.
  return reply?.query === query ? reply.result : test.trim() ? 'untestable' : 'empty';
}

interface RuleTestFoldProps {
  t: TFunction;
  test: string;
  setTest: (v: string) => void;
  testResult: TestResult;
}

/** 测试匹配（折叠；缺少上下文或不支持的表达式明确显示无法判断）。 */
export function RuleTestFold({ t, test, setTest, testResult }: RuleTestFoldProps) {
  return (
    <div className="fld">
      <details className="rule-test-det" onToggle={revealOnToggle}>
        <summary className="fld-l" style={{ display: 'flex', alignItems: 'center', gap: 6, cursor: 'pointer' }}>
          <span>{t('rules.testMatch')}</span>
          <Chevron />
        </summary>
        <label className="input" style={{ display: 'flex', alignItems: 'center', gap: 8, padding: '0 11px', marginTop: 8 }}>
          <svg viewBox="0 0 24 24" width={14} fill="none" stroke="currentColor" strokeWidth={1.8} style={{ color: 'hsl(var(--fg-faint))', flex: 'none' }}>
            <circle cx="11" cy="11" r="7" />
            <path d="M20 20l-3-3" />
          </svg>
          <input
            value={test}
            onChange={(e) => setTest(e.target.value)}
            placeholder={t('rules.testPh')}
            aria-label={t('rules.testMatch')}
            style={{ border: 0, background: 'none', outline: 'none', flex: 1, padding: '8px 0', font: 'inherit', color: 'inherit' }}
          />
        </label>
        <div className="card-sub" style={{ marginTop: 6 }}>
          {testResult === 'hit' && (
            <span style={{ color: 'hsl(var(--ok))' }}>✓ {t('rules.testHit')}</span>
          )}
          {testResult === 'miss' && (
            <span style={{ color: 'hsl(var(--fg-faint))' }}>{t('rules.testMiss')}</span>
          )}
          {testResult === 'untestable' && (
            <span style={{ color: 'hsl(var(--fg-faint))' }}>
              {t('rules.testUntestable')}
            </span>
          )}
        </div>
      </details>
    </div>
  );
}
