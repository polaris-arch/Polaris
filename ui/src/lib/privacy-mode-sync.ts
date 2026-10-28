/** Privacy hydration is fail-closed and shared by desktop and mobile roots. */
export interface PrivacyModePort {
  onEnterPrivacyMode(listener: () => void): Promise<() => void>;
  onExitPrivacyMode(listener: () => void): Promise<() => void>;
  getPrivacyMode(): Promise<boolean>;
}

export function startPrivacyModeSync(port: PrivacyModePort, apply: (locked: boolean) => void): () => void {
  let disposed = false;
  let generation = 0;
  let retry: ReturnType<typeof setTimeout> | null = null;
  const offs: Array<() => void> = [];
  const release = () => { for (const off of offs.splice(0)) off(); };
  const later = (run: () => void) => {
    if (!disposed) retry = setTimeout(() => { retry = null; run(); }, 1000);
  };
  // Unknown state must never disclose a previously locked renderer.
  apply(true);
  const connect = () => {
    const attempt = ++generation;
    const active = () => !disposed && attempt === generation;
    let ready = false;
    let revision = 0;
    const receive = (locked: boolean) => {
      if (!active()) return;
      revision++;
      // Enter is safe during partial registration; exit needs the complete feed.
      if (locked || ready) apply(locked);
    };
    const read = async () => {
      const before = revision;
      try {
        const locked = await port.getPrivacyMode();
        if (typeof locked !== 'boolean') throw new Error('PRIVACY_SNAPSHOT_UNAVAILABLE');
        if (active() && before === revision) apply(locked);
      } catch (error) {
        if (!active()) return;
        // Preserve any authoritative event; otherwise remain fail-closed.
        console.error('[privacy] snapshot failed:', error);
        later(() => { if (active()) void read(); });
      }
    };
    const register = async (operation: Promise<() => void>) => {
      const off = await operation;
      if (!active()) off();
      else offs.push(off);
    };
    void Promise.all([
      register(port.onEnterPrivacyMode(() => receive(true))),
      register(port.onExitPrivacyMode(() => receive(false))),
    ]).then(() => {
      if (!active()) return;
      ready = true;
      void read();
    }).catch((error) => {
      if (!active()) return;
      generation++; // Late registration completions release themselves.
      release();
      apply(true);
      console.error('[privacy] listener registration failed:', error);
      later(connect);
    });
  };
  connect();
  return () => {
    disposed = true;
    generation++;
    if (retry !== null) clearTimeout(retry);
    release();
  };
}
