import { beforeEach, expect, it, vi } from 'vitest';
const h = vi.hoisted(() => ({ states: [] as unknown[], si: 0, effects: [] as (() => void | (() => void))[], preview: vi.fn(), publication: null as unknown }));
vi.mock('react', async original => ({ ...await original<typeof import('react')>(),
  useState: (initial: unknown) => { const i=h.si++; if (!(i in h.states)) h.states[i]=initial; return [h.states[i],(value: unknown) => {h.states[i]=typeof value==='function'?value(h.states[i]):value;}]; },
  useEffect: (effect: () => void | (() => void)) => {h.effects.push(effect);},
}));
vi.mock('react-i18next', () => ({ useTranslation: () => ({t:(key:string)=>key}) }));
vi.mock('@/ipc', () => ({ api: {server:{identityConflicts:h.preview}} }));
vi.mock('@/store/app-store', () => ({useAppStore:(selector:(s:unknown)=>unknown)=>selector({config:null,servers:[],configPublication:h.publication})}));
import { IdentityRecoveryEntry } from './IdentityRecoveryEntry';
function render(){h.si=0;return IdentityRecoveryEntry({onOpen:vi.fn()});}
beforeEach(()=>{h.states=[];h.effects=[];h.publication=null;h.preview.mockReset();});
it.each(['duplicate IDs plus blank API secret','duplicate IDs plus pending privacy migration'])('exposes saved-disk recovery without config hydration: %s',async()=>{
 h.preview.mockResolvedValue({version:'opaque',groups:[{indices:[0,1],repairable:true,records:[]}]});
 expect(render()).toBeNull();h.effects[0]();await Promise.resolve();
 const entry=render();expect(entry?.props.role).toBe('alert');expect(h.preview).toHaveBeenCalledTimes(1);
 expect(JSON.stringify(entry)).toContain('identityRepair.review');expect(JSON.stringify(entry)).not.toContain('opaque');
});
it('rejects a late read after unmount and exposes safe retry on failure',async()=>{
 let finish!:(v:unknown)=>void;h.preview.mockImplementation(()=>new Promise(resolve=>{finish=resolve;}));render();const cleanup=h.effects[0]();cleanup?.();finish({groups:[{}]});await Promise.resolve();expect(render()).toBeNull();
 h.states=[];h.effects=[];h.preview.mockRejectedValue(new Error('private path'));render();h.effects[0]();await Promise.resolve();const entry=render();expect(JSON.stringify(entry)).toContain('identityRepair.reload');expect(JSON.stringify(entry)).not.toContain('private path');
});
