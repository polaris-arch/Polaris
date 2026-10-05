import { useEffect, useState } from 'react';

/** Only controls presentation. The update task and its progress subscription keep running. */
export function useProgressDisclosure(active: boolean) {
  const [hidden, setHidden] = useState(false);
  useEffect(() => {
    if (!active) setHidden(false);
  }, [active]);
  return {
    hidden: active && hidden,
    hide: () => setHidden(true),
    show: () => setHidden(false),
  };
}
