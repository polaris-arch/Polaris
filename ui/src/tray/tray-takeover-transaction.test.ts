import { readFileSync } from 'node:fs';
import { describe, expect, it, vi } from 'vitest';
import { IpcError } from '../ipc/ipc-client';
import { withConfigWriteLock } from '../lib/config-write-lock';

// Execute the real production handler body, with boundary services supplied as
// fixtures. No rewritten test implementation of the tray transaction.
const source = readFileSync(new URL('./TrayMenu.tsx', import.meta.url), 'utf8');
const start = source.indexOf('  const setTakeover = async (modeType: ProxyModeType) => {');
const end = source.indexOf('\n  const switchExit =', start);
if (start < 0 || end <= start) throw new Error('Tray handler extraction failed');
const handler = source.slice(start, end).replace('(modeType: ProxyModeType)', '(modeType)');

describe('takeover tray transaction', () => {
  const make = new Function(
    'api', 'config', 'trayLifecycle', 'interactionEpoch', 'mounted', 'interactionActive', 'takeoverIntent',
    'withConfigWriteLock', 'hide', 'setNotice', 'notifyDesktop', 'noticeActionFailure', 't', 'IpcError',
    `${handler}; return setTakeover;`,
  );
  const takeoverFixture = (setTakeover: (mode: string) => Promise<unknown>, config: object) => {
    const hide = vi.fn(), notice = vi.fn(), notify = vi.fn(), fail = vi.fn();
    const change = make(
      { config: { setTakeover } }, config, { current: 1 }, { current: 1 }, { current: true }, { current: true },
      { current: 0 }, withConfigWriteLock, hide, notice, notify, fail, (key: string) => key, IpcError,
    );
    return { change, hide, notice, notify, fail };
  };

  it.each([
    ['manual', false, false, []],
    ['manual', true, false, [['tray.fakeIpAutoEnabled']]],
    ['manual', false, true, [['tray.manageInMain']]],
    ['manual', true, true, [['tray.fakeIpAutoEnabled tray.manageInMain']]],
    ['tun', false, true, [['tray.manageInMain']]],
  ] as const)('from %s preserves the latest config and reports correction %s and pending %s', async (previous, corrected, pendingApply, notices) => {
    const stale = {proxyModeType:'manual', dnsConfig:{server:'old'}, dnsDefaults:{pending:true}};
    const latest = {proxyModeType:previous as string, dnsConfig:{server:'new'}, dnsDefaults:{pending:false}};
    const setTakeover = vi.fn(async (mode: string) => {
      latest.proxyModeType = mode;
      return { corrected, pendingApply };
    });
    const { change, hide, notice, notify, fail } = takeoverFixture(setTakeover, stale);
    await change('tun');
    expect(setTakeover.mock.calls).toEqual([['tun']]);
    expect(latest.dnsConfig).toEqual({server:'new'});expect(latest.dnsDefaults).toEqual({pending:false});
    expect(notify).toHaveBeenCalledTimes(corrected?1:0);expect(notice.mock.calls).toEqual(notices);
    expect(hide).toHaveBeenCalledTimes(corrected || pendingApply ? 0 : 1);expect(fail).not.toHaveBeenCalled();
  });

  it('directs an edit blocked by existing pending work to the main window', async () => {
    const error = new IpcError('config_set_takeover', 'existing work', 'CONFIG_TAKEOVER_BLOCKED_BY_PENDING');
    const { change, hide, notice, notify, fail } = takeoverFixture(vi.fn().mockRejectedValue(error), {});
    await change('tun');
    expect(notice).toHaveBeenCalledWith('tray.manageInMain');
    expect(hide).not.toHaveBeenCalled();
    expect(notify).not.toHaveBeenCalled();
    expect(fail).not.toHaveBeenCalled();
  });

  it.each([false, true])('serializes a reversal and drops older hide or correction feedback (%s)', async corrected => {
    let finishFirst!: () => void;
    const firstReceipt = new Promise<void>(resolve => { finishFirst = resolve; });
    const setTakeover = vi.fn(async (mode: string) => {
      if (mode === 'tun') {
        await firstReceipt;
        return { corrected, pendingApply: false };
      }
      return { corrected: false, pendingApply: true };
    });
    const { change, hide, notice, notify } = takeoverFixture(setTakeover, {});
    const first = change('tun');
    await Promise.resolve();
    expect(setTakeover.mock.calls).toEqual([['tun']]);
    const second = change('manual');
    expect(setTakeover.mock.calls).toEqual([['tun']]);
    finishFirst();
    await Promise.all([first, second]);
    expect(setTakeover.mock.calls).toEqual([['tun'], ['manual']]);
    expect(notice.mock.calls).toEqual([['tray.manageInMain']]);
    expect(hide).not.toHaveBeenCalled();
    expect(notify).not.toHaveBeenCalled();
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
    const interaction = { current: 1 };
    const change = make(
      { config: { updateMode } }, { current: 1 }, interaction, { current: true }, { current: true },
      { current: 0 }, withConfigWriteLock, setNotice,
      [{ v: 'global', k: 'tray.modeGlobal' }, { v: 'smart', k: 'tray.modeSmart' }],
      vi.fn(), (key: string) => key,
    );
    return { change, setNotice, interaction };
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

  it.each([false, true])('preserves a queued reversal after the tray loses focus: %s', async (blur) => {
    let finishFirst!: () => void;
    const firstReceipt = new Promise<void>((resolve) => { finishFirst = resolve; });
    const updateMode = vi.fn(async (mode: string) => {
      if (mode === 'global') await firstReceipt;
      return { status: 'saved', savedOffline: true };
    });
    const { change, setNotice, interaction } = modeFixture(updateMode);
    const first = change('global');
    await Promise.resolve();
    expect(updateMode.mock.calls).toEqual([['global']]);
    const second = change('smart');
    expect(updateMode.mock.calls).toEqual([['global']]);
    if (blur) interaction.current += 1;
    finishFirst();
    await Promise.all([first, second]);
    expect(updateMode.mock.calls).toEqual([['global'], ['smart']]);
    expect(setNotice.mock.calls).toEqual(blur ? [] : [['tray.modeSmart']]);
  });
});
