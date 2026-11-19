import { readFileSync } from 'node:fs';
import { describe, expect, it, vi } from 'vitest';
import { withConfigWriteLock } from '../lib/config-write-lock';

// Execute the real production handler body, with boundary services supplied as
// fixtures. No rewritten test implementation of the tray transaction.
const source = readFileSync(new URL('./TrayMenu.tsx', import.meta.url), 'utf8');
const start = source.indexOf('  const setTakeover = async (modeType: ProxyModeType) => {');
const end = source.indexOf('\n  const switchExit =', start);
if (start < 0 || end <= start) throw new Error('Tray handler extraction failed');
const handler = source.slice(start, end).replace('(modeType: ProxyModeType)', '(modeType)');

describe('UIP03 stale tray snapshot cannot roll back concurrent DNS edits', () => {
  for (const corrected of [false, true]) it(`latest-config transaction, corrected=${corrected}`, async () => {
    const stale = {proxyModeType:'manual', dnsConfig:{server:'old'}, dnsDefaults:{pending:true}};
    const latest = {proxyModeType:'manual', dnsConfig:{server:'new'}, dnsDefaults:{pending:false}};
    const hide=vi.fn(), notice=vi.fn(), notify=vi.fn(), fail=vi.fn();
    const api={config:{setTakeover:vi.fn(async (mode:string)=>{latest.proxyModeType=mode; return {corrected};})}};
    const make = new Function('api','config','trayLifecycle','interactionEpoch','mounted','interactionActive','hide','setNotice','notifyDesktop','noticeActionFailure','t',`${handler}; return setTakeover;`);
    const apply=make(api,stale,{current:1},{current:1},{current:true},{current:true},hide,notice,notify,fail,(s:string)=>s);
    await apply('tun');
    expect(api.config.setTakeover.mock.calls).toEqual([['tun']]);
    expect(latest.dnsConfig).toEqual({server:'new'});expect(latest.dnsDefaults).toEqual({pending:false});
    expect(notify).toHaveBeenCalledTimes(corrected?1:0);expect(notice).toHaveBeenCalledTimes(corrected?1:0);
    expect(hide).toHaveBeenCalledTimes(corrected?0:1);expect(fail).not.toHaveBeenCalled();
  });
});

describe('tray proxy mode feedback', () => {
  const modeStart = source.indexOf('  const setMode = async (mode: ProxyMode) => {');
  const modeEnd = source.indexOf('\n  };', modeStart);
  if (modeStart < 0 || modeEnd < 0) throw new Error('Tray mode handler extraction failed');
  const modeHandler = source.slice(modeStart, modeEnd + '\n  };'.length).replace('(mode: ProxyMode)', '(mode)');
  const make = new Function(
    'api', 'trayLifecycle', 'interactionEpoch', 'mounted', 'interactionActive', 'modeIntent',
    'withConfigWriteLock', 'setNotice', 'MODES', 'noticeActionFailure', 't',
    `${modeHandler}\nreturn setMode;`,
  );
  const modeFixture = (updateMode: (mode: string) => Promise<unknown>) => {
    const setNotice = vi.fn();
    const change = make(
      { config: { updateMode } }, { current: 1 }, { current: 1 }, { current: true }, { current: true },
      { current: 0 }, withConfigWriteLock, setNotice,
      [{ v: 'global', k: 'tray.modeGlobal' }, { v: 'smart', k: 'tray.modeSmart' }],
      vi.fn(), (key: string) => key,
    );
    return { change, setNotice };
  };

  it.each([
    [{ status: 'saved', savedOffline: true }, [['tray.modeGlobal']]],
    [{ status: 'saved' }, []],
    [{ status: 'saved', runtimeApply: { status: 'hotApplied', configurationDurable: true } }, [['tray.modeGlobal']]],
    [{ status: 'saved', runtimeApply: { status: 'hotApplied', configurationDurable: false } }, []],
    [{ status: 'saved', runtimeApply: { status: 'pending', pendingApply: true } }, [['tray.manageInMain']]],
    [{ status: 'needsManual' }, [['tray.manageInMain']]],
  ])('shows the result for %o', async (outcome, expected) => {
    const { change, setNotice } = modeFixture(async () => outcome);
    await change('global');
    expect(setNotice.mock.calls).toEqual(expected);
  });

  it('saves a reversal after the in-flight mode without showing the older result', async () => {
    let finishFirst!: () => void;
    const firstReceipt = new Promise<void>((resolve) => { finishFirst = resolve; });
    const updateMode = vi.fn(async (mode: string) => {
      if (mode === 'global') await firstReceipt;
      return { status: 'saved', savedOffline: true };
    });
    const { change, setNotice } = modeFixture(updateMode);
    const first = change('global');
    await Promise.resolve();
    expect(updateMode.mock.calls).toEqual([['global']]);
    const second = change('smart');
    expect(updateMode.mock.calls).toEqual([['global']]);
    finishFirst();
    await Promise.all([first, second]);
    expect(updateMode.mock.calls).toEqual([['global'], ['smart']]);
    expect(setNotice.mock.calls).toEqual([['tray.modeSmart']]);
  });
});
