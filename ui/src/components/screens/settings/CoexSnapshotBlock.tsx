import { CoexRuntimeDecodeError, type CoexRuntimeState } from '@/contracts/coex-runtime';
/** Read-only runtime observations. Lifecycle phase never supplies classifier evidence. */
import { useEffect, useRef, useState, type ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import { api } from '@/ipc';
import { CoexSnapshotDecodeError, type CoexSnapshot, type Fact } from '@/contracts/coex-snapshot';
import { SetBlock } from './Primitives';
import { CoexWindowsSources } from './CoexWindowsSources';

type Text = (key: string) => string;
type Pages = { addresses: number; routes: number; policyRules: number };
const PAGE_SIZE = 20;
const FIRST_PAGES: Pages = { addresses: 0, routes: 0, policyRules: 0 };

/** Pure display: keeps all producer facts separate and never resolves a route or owner. */
export function renderCoexFacts(
  snapshot: CoexSnapshot,
  t: Text,
  expanded: number | null = null,
  pages: Pages = FIRST_PAGES,
  select: (index: number | null) => void = () => {},
  paginate: (field: keyof Pages, page: number) => void = () => {},
): ReactNode {
  if (snapshot.schemaVersion === 2) return <CoexWindowsSources snapshot={snapshot} t={t} />;
  const fact = <T,>(label: string, value: Fact<T>, show: (known: T) => ReactNode) => (
    <div className="card-sub">
      <b>{label}: </b>
      {value.status === 'known' ? show(value.value) : <>
        <span>{t('settings.coex.unknown')}</span>
        <details><summary>{t('settings.coex.reason')}</summary><span className="mono">{value.reason}</span></details>
      </>}
    </div>
  );
  const table = (value: number | null) => value === null ? t('settings.coex.noTableNumber') : <span className="mono">{value}</span>;
  const boolean = (value: boolean) => value ? t('settings.coex.true') : t('settings.coex.false');
  const rows = <T,>(field: keyof Pages, label: string, value: Fact<T[]>, show: (row: T) => ReactNode) =>
    fact(label, value, (known) => {
      const page = Math.min(pages[field], Math.max(0, Math.ceil(known.length / PAGE_SIZE) - 1));
      const start = page * PAGE_SIZE;
      return <>
        <span>{t('settings.coex.total')}: {known.length}</span>
        {known.length === 0 ? <div>{t('settings.coex.observedEmpty')}</div> : <>
          <div>{t('settings.coex.displayed')}: {start + 1}–{Math.min(start + PAGE_SIZE, known.length)} / {known.length}</div>
          <ol start={start + 1}>{known.slice(start, start + PAGE_SIZE).map((row, index) => <li key={start + index}>{show(row)}</li>)}</ol>
          {known.length > PAGE_SIZE && <div>
            <button type="button" className="btn ghost sm" disabled={page === 0} onClick={() => paginate(field, page - 1)}>{t('settings.coex.previous')}</button>
            <button type="button" className="btn ghost sm" disabled={start + PAGE_SIZE >= known.length} onClick={() => paginate(field, page + 1)}>{t('settings.coex.next')}</button>
          </div>}
        </>}
      </>;
    });
  return <div data-coex-facts>
    <div className="card-sub">{t('settings.coex.platform')}: <span className="mono">{snapshot.platform}</span></div>
    {!['linux', 'darwin'].includes(snapshot.platform) && <div className="card-sub">{t('settings.coex.sourceUnavailable')}</div>}
    {fact(t('settings.coex.observation'), snapshot.observation, (value) => <>
      {t('settings.coex.elapsed')}: {value.elapsedMillis} · {t('settings.coex.atomic')}: {boolean(value.atomic)}
    </>)}
    {fact(t('settings.coex.cleanup'), snapshot.commandCleanup, () => t('settings.coex.noRetained'))}
    <div className="card-sub">{t('settings.coex.cleanupHint')}</div>
    {fact(t('settings.coex.classification'), snapshot.classification, () => null)}
    {fact(t('settings.coex.phase'), snapshot.context.observationPhase, () => null)}
    {fact(t('settings.coex.ownInterfaces'), snapshot.context.ownInterfaces, () => null)}
    {fact(t('settings.coex.criteria'), snapshot.context.criteria, () => null)}
    {fact(t('settings.coex.history'), snapshot.context.repairHistory, () => null)}
    {fact(t('settings.coex.interfaces'), snapshot.objects, (objects) => <>
      <span>{t('settings.coex.total')}: {objects.length}</span>
      {objects.length === 0 && <div>{t('settings.coex.observedEmpty')}</div>}
      {objects.map((object, index) => <details key={index} open={expanded === index} data-coex-interface>
        <summary onClick={(event) => { event.preventDefault(); select(expanded === index ? null : index); }}>
          <span className="mono">{object.interface}</span>
        </summary>
        {expanded === index && <>
          {fact(t('settings.coex.tunnel'), object.tunnel, boolean)}
          {fact(t('settings.coex.virtualization'), object.virtualization, boolean)}
          {fact(t('settings.coex.identity'), object.stableIdentity, (id) => id === null ? t('settings.coex.noIdentity') : <span className="mono">{id}</span>)}
          {rows('addresses', t('settings.coex.addresses'), object.addresses, (row) => <span className="mono">{row.address}/{row.prefixLen}</span>)}
          {rows('routes', t('settings.coex.routes'), object.routes, (row) => <>
            <span className="mono">{row.prefix}</span>
            {fact(t('settings.coex.table'), row.table, table)}
            {fact(t('settings.coex.scope'), row.scope, (v) => v === 'global' ? t('settings.coex.global') : t('settings.coex.interfaceScoped'))}
            {fact(t('settings.coex.role'), row.role, (v) => v === 'coverageDeclaration' ? t('settings.coex.coverageDeclaration') : t('settings.coex.resourceClaim'))}
          </>)}
          {rows('policyRules', t('settings.coex.rules'), object.policyRules, (row) => <>
            <div>{t('settings.coex.priority')}: {row.priority}</div>
            {fact(t('settings.coex.lookupTable'), row.lookupTable, table)}
            {fact(t('settings.coex.family'), row.addressFamily, (v) => <span className="mono">{v}</span>)}
            {fact(t('settings.coex.selector'), row.selectorScope, (v) => v.kind === 'global' ? t('settings.coex.global') : <>{t('settings.coex.limited')}: <span className="mono">{v.selector}</span></>)}
            {fact(t('settings.coex.association'), row.appliesToObject, boolean)}
          </>)}
        </>}
      </details>)}
    </>)}
  </div>;
}

export function CoexSnapshotBlock() {
  const { t } = useTranslation();
  const [state, setState] = useState<CoexRuntimeState | null>(null);
  const [error, setError] = useState<{ malformed: boolean; detail: string; subscription: boolean } | null>(null);
  const [expanded, setExpanded] = useState<number | null>(null);
  const [pages, setPages] = useState(FIRST_PAGES);
  const revision = useRef<bigint>(BigInt(-1));
  const alive = useRef(false);
  const inFlight = useRef(false);
  const epoch = useRef(0);
  const listener = useRef<{ mount: number; off: () => void } | null>(null);
  const registering = useRef<{ mount: number; promise: Promise<boolean> } | null>(null);
  const accept = (next: CoexRuntimeState) => {
    if (!alive.current || BigInt(next.reportRevision) <= revision.current) return;
    revision.current = BigInt(next.reportRevision);
    setState(next); setError(null);
  };
  const fail = (failure: unknown, subscription = false) => {
    if (alive.current) setError({ malformed: failure instanceof CoexRuntimeDecodeError || failure instanceof CoexSnapshotDecodeError, detail: failure instanceof Error ? failure.message : String(failure), subscription });
  };
  const ensureSubscribed = async (mount: number): Promise<boolean> => {
    if (listener.current?.mount === mount) return false;
    if (registering.current?.mount === mount) return registering.current.promise;
    const promise = (async () => {
      const off = await api.system.onCoexRuntimeState(
        next => { if (epoch.current === mount) accept(next); },
        failure => { if (epoch.current === mount) fail(failure); },
      );
      if (!alive.current || epoch.current !== mount) { off(); return false; }
      listener.current = { mount, off };
      setError(previous => previous?.subscription ? null : previous);
      return true;
    })();
    registering.current = { mount, promise };
    try { return await promise; }
    finally { if (registering.current?.promise === promise) registering.current = null; }
  };
  useEffect(() => {
    alive.current = true;
    const mount = ++epoch.current;
    void (async () => {
      try { await ensureSubscribed(mount); }
      catch (failure) { if (epoch.current === mount) fail(failure, true); return; }
      if (!alive.current || epoch.current !== mount) return;
      const beforeGet = revision.current;
      try {
        const initial = await api.system.coexRuntimeGetState();
        if (epoch.current === mount) accept(initial);
      }
      catch (failure) { if (epoch.current === mount && revision.current === beforeGet) fail(failure); }
    })();
    return () => {
      alive.current = false; epoch.current++;
      if (listener.current?.mount === mount) { listener.current.off(); listener.current = null; }
    };
  }, []);
  const collect = async () => {
    if (inFlight.current) return;
    inFlight.current = true;
    const mount = epoch.current;
    const beforeRequest = revision.current;
    setExpanded(null); setPages(FIRST_PAGES);
    let subscribed = false;
    try {
      // Retry restores the listener first, then reads state before scheduling refresh.
      const restored = await ensureSubscribed(mount);
      subscribed = true;
      if (!alive.current || epoch.current !== mount) return;
      const current = restored || !state ? await api.system.coexRuntimeGetState() : state;
      if (!alive.current || epoch.current !== mount) return;
      const next = await api.system.coexRuntimeRefresh(current.reportRevision);
      if (epoch.current === mount) accept(next);
    } catch (failure) { if (epoch.current === mount && revision.current === beforeRequest) fail(failure, !subscribed); }
    finally { inFlight.current = false; }
  };
  const snapshot = state?.report.status === 'known' ? state.report.value.snapshot : null;
  const loading = state?.activity === 'running' || state?.activity === 'runningWithPending';
  return <section data-coex-snapshot aria-label={t('settings.coex.title')} style={{ minWidth: 0, overflowWrap: 'anywhere' }}>
    <SetBlock header={t('settings.coex.title')}>
      <div className="card-sub">{t('settings.coex.hint')}</div>
      <button type="button" className="btn ghost sm" onClick={() => { void collect(); }} disabled={!state && !error}>
        {loading ? t('settings.coex.loading') : error || snapshot?.schemaVersion === 2 ? t('settings.coex.retry') : t('settings.coex.collect')}
      </button>
      <div className="card-sub" role="status" aria-live="polite">
        {error ? t('settings.coex.unavailable') : state ? t(`settings.coex.${state.freshness}`) : t('settings.coex.notCollected')}
        {state && state.activity !== 'idle' ? ` · ${t('settings.coex.busy')}` : null}
        {state?.pending.status === 'known' ? ` · ${t('settings.coex.queued')}` : null}
      </div>
      {state?.reason.status === 'known' && state.freshness !== 'latest' && <details className="card-sub"><summary>{t('settings.coex.reason')}</summary>{t(`settings.coex.runtimeReasons.${state.reason.value.code}`)}</details>}
      {error && <div className="card-sub" role="alert">
        {error.malformed ? t('settings.coex.malformed') : t('settings.coex.failed')}
        <details><summary>{t('settings.coex.reason')}</summary><span className="mono">{error.detail}</span></details>
      </div>}
      {snapshot && renderCoexFacts(snapshot, (key) => t(key), expanded, pages,
        (index) => { setExpanded(index); setPages(FIRST_PAGES); },
        (field, page) => setPages((previous) => ({ ...previous, [field]: page })))}
    </SetBlock>
  </section>;
}
