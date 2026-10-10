/** Source-local Windows observations. No interface join or safety verdict. */
import { useState, type ReactNode } from 'react';
import type { Fact } from '@/contracts/coex-snapshot';
import type { CoexWindowsSnapshot, WindowsReference, WindowsSource, WindowsRoute } from '@/contracts/coex-windows';
type Text = (key: string) => string;
function Field<T>({ label, value, show, t }: { label: string; value: Fact<T>; show: (v: T) => ReactNode; t: Text }) {
  return <div className="card-sub"><b>{label}: </b>{value.status === 'known' ? show(value.value) : <>
    <span>{t('settings.coex.unknown')}</span><details><summary>{t('settings.coex.reason')}</summary><span className="mono">{value.reason}</span></details>
  </>}</div>;
}
function Source<T>({ id, label, value, show, t }: { id: string; label: string; value: WindowsSource<T>; show: (v: T) => ReactNode; t: Text }) {
  const [open, setOpen] = useState(false); const [page, setPage] = useState(0);
  const count = value.rows.status === 'known' ? value.rows.value.length : 0;
  const current = Math.min(page, Math.max(0, Math.ceil(count / 20) - 1)); const start = current * 20;
  return <details data-coex-source={id} open={open} onToggle={(event) => setOpen(event.currentTarget.open)}>
    <summary>{label}</summary>{open && <>
      <Field label={t('settings.coex.completeness')} value={value.complete} t={t} show={(v) => t(v ? 'settings.coex.true' : 'settings.coex.false')} />
      <Field label={t('settings.coex.compartment')} value={value.compartment} t={t} show={(v) => <span className="mono">{v}</span>} />
      <Field label={t('settings.coex.sourceError')} value={value.error} t={t} show={(v) => v === null ? t('settings.coex.noSourceError') : <span className="mono">{v}</span>} />
      <Field label={t('settings.coex.rows')} value={value.rows} t={t} show={(rows) => <>
        <div>{t('settings.coex.total')}: {rows.length}</div>{rows.length === 0 ? <div>{t('settings.coex.observedEmpty')}</div> : <>
          <div>{t('settings.coex.displayed')}: {start + 1}–{Math.min(start + 20, rows.length)} / {rows.length}</div>
          <ol start={start + 1}>{rows.slice(start, start + 20).map((v, index) => <li key={start + index}>{show(v)}</li>)}</ol>
          {rows.length > 20 && <div>
            <button type="button" className="btn ghost sm" disabled={current === 0} onClick={() => setPage(current - 1)}>{t('settings.coex.previous')}</button>
            <button type="button" className="btn ghost sm" disabled={start + 20 >= rows.length} onClick={() => setPage(current + 1)}>{t('settings.coex.next')}</button>
          </div>}
        </>}
      </>} />
    </>}
  </details>;
}
export function CoexWindowsSources({ snapshot, t }: { snapshot: CoexWindowsSnapshot; t: Text }) {
  const text = (v: string | number) => <span className="mono">{v}</span>;
  const ref = (v: WindowsReference) => <>
    <Field label={t('settings.coex.interface')} value={v.alias} show={text} t={t} />
    <Field label={t('settings.coex.luid')} value={v.luid} show={text} t={t} />
    <Field label={t('settings.coex.index')} value={v.ifIndex} show={text} t={t} />
  </>;
  const route = (v: WindowsRoute) => <>
    {ref(v.interface)}<div className="mono">{v.family} · {v.prefix}</div>
    <Field label={t('settings.coex.nextHop')} value={v.nextHop} show={text} t={t} />
    <Field label={t('settings.coex.nextHopScopeId')} value={v.nextHopScopeId} show={text} t={t} />
    <Field label={t('settings.coex.routeMetric')} value={v.routeMetric} show={text} t={t} />
    <Field label={t('settings.coex.interfaceMetric')} value={v.interfaceMetric} show={text} t={t} />
  </>;
  return <div data-coex-facts data-coex-windows>
    <div className="card-sub">{t('settings.coex.platform')}: <span className="mono">win32</span></div>
    <div className="card-sub">{t('settings.coex.windowsHint')}</div>
    <Field label={t('settings.coex.observation')} value={snapshot.observation} t={t} show={(v) => <>{t('settings.coex.elapsed')}: {v.elapsedMillis} · {t('settings.coex.atomic')}: {t('settings.coex.false')}</>} />
    <Field label={t('settings.coex.cleanup')} value={snapshot.commandCleanup} t={t} show={() => null} />
    <Field label={t('settings.coex.interfaces')} value={snapshot.objects} t={t} show={() => null} />
    <Field label={t('settings.coex.classification')} value={snapshot.classification} t={t} show={() => null} />
    {Object.entries(snapshot.context).map(([key, value]) => <Field key={key} label={t(`settings.coex.${({ observationPhase: 'phase', ownInterfaces: 'ownInterfaces', criteria: 'criteria', repairHistory: 'history' })[key as keyof typeof snapshot.context]}`)} value={value} t={t} show={() => null} />)}
    <Source id="adapters" label={t('settings.coex.adaptersSource')} value={snapshot.sources.adapters} t={t} show={(v) => <>{ref(v.interface)}<Field label={t('settings.coex.ifType')} value={v.ifType} show={text} t={t} /><Field label={t('settings.coex.description')} value={v.description} show={text} t={t} /></>} />
    <Source id="addresses" label={t('settings.coex.addressesSource')} value={snapshot.sources.addresses} t={t} show={(v) => <>{ref(v.interface)}<div className="mono">{v.family} · {v.address}</div><Field label={t('settings.coex.prefixLength')} value={v.prefixLen} show={text} t={t} /><Field label={t('settings.coex.scopeId')} value={v.scopeId} show={text} t={t} /></>} />
    <Source id="routes4" label={t('settings.coex.routes4Source')} value={snapshot.sources.routes4} t={t} show={route} />
    <Source id="routes6" label={t('settings.coex.routes6Source')} value={snapshot.sources.routes6} t={t} show={route} />
    <Source id="ras" label={t('settings.coex.rasSource')} value={snapshot.sources.ras} t={t} show={(v) => <><Field label={t('settings.coex.rasName')} value={v.name} show={text} t={t} /><div>{t('settings.coex.allUsers')}: {t(v.allUsers ? 'settings.coex.true' : 'settings.coex.false')}</div><Field label={t('settings.coex.association')} value={v.interface} show={ref} t={t} /></>} />
  </div>;
}
