import { useEffect, useRef } from 'react';

/** Same-input request ownership; cancelling the view does not cancel a native mutation. */
export function createInputRequest(initialKey: string) {
  let key = initialKey;
  let revision = 0;
  let mounted = true;
  return {
    update(next: string) { if (key !== next) { key = next; revision += 1; } },
    mount() { mounted = true; },
    dispose() { mounted = false; revision += 1; },
    begin() {
      const observed = ++revision;
      return () => mounted && revision === observed;
    },
  };
}

/** A reply belongs to one exact input revision and mounted view, including A→B→A edits. */
export function useInputRequest(inputKey: string) {
  const owner = useRef<ReturnType<typeof createInputRequest> | null>(null);
  if (!owner.current) owner.current = createInputRequest(inputKey);
  const current = owner.current;
  current.update(inputKey);
  useEffect(() => { current.mount(); return () => current.dispose(); }, [current]);
  return current.begin;
}
