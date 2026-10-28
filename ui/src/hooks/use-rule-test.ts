import { useEffect, useMemo, useState } from 'react';
import { computeTestMatchAsync, type Cond, type TestResult } from '@/components/dialogs/rule-cond';

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
