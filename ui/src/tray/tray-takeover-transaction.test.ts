import { readFileSync } from 'node:fs';
import { describe, expect, it, vi } from 'vitest';

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
