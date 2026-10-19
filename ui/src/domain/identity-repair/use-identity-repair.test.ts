import { beforeEach, describe, expect, it, vi } from 'vitest';
const h = vi.hoisted(() => ({ states: [] as unknown[], refs: [] as {current:unknown}[], si:0, ri:0,
  effect: null as (()=>void|(()=>void)) | null, cleanup: null as (()=>void)|null,
  preview: vi.fn(), repair: vi.fn(), refresh: vi.fn() }));
vi.mock('react', async importOriginal => ({ ...await importOriginal<typeof import('react')>(),
  useState: (initial:unknown) => { const i=h.si++; if (!(i in h.states)) h.states[i]=initial; return [h.states[i],(value:unknown)=> { h.states[i]=typeof value==='function'?value(h.states[i]):value; }]; },
  useRef: (initial:unknown) => { const i=h.ri++; return h.refs[i]??(h.refs[i]={current:initial}); },
  useCallback: (fn:unknown)=>fn,
  useEffect: (effect:()=>void|(()=>void)) => { h.effect??=effect; },
}));
vi.mock('@/ipc',()=>({ api:{server:{identityConflicts:h.preview,identityRepair:h.repair}}, IpcError:class extends Error { constructor(_command:string,message:string,public code?:string){super(message);} } }));
vi.mock('@/store/app-store',()=>({refreshConfig:h.refresh}));
import { IpcError } from '@/ipc';
import { useIdentityRepair } from './use-identity-repair';
function render(){h.si=0;h.ri=0;return useIdentityRepair();}
const preview={version:'v1',groups:[{indices:[0,1],repairable:true,records:[]}]};
async function mount(){render();h.cleanup=h.effect?.()??null;await Promise.resolve();return render();}
function choose(){const form=render();form.setKeepers({0:1});form.setAcknowledged(true);return render();}
beforeEach(()=>{h.states=[];h.refs=[];h.effect=null;h.cleanup=null;h.preview.mockReset().mockResolvedValue(preview);h.repair.mockReset().mockResolvedValue({status:'saved',requiresApply:true});h.refresh.mockReset().mockResolvedValue(undefined);});
describe('identity repair interaction',()=>{
 it('does not write without explicit keeper and reference confirmation',async()=>{const form=await mount();await form.save();expect(h.repair).not.toHaveBeenCalled();form.setKeepers({0:1});await render().save();expect(h.repair).not.toHaveBeenCalled();});
 it('submits exactly once and reports saved-pending instead of applied',async()=>{await mount();const form=choose();await Promise.all([form.save(),form.save()]);expect(h.repair).toHaveBeenCalledTimes(1);expect(h.repair).toHaveBeenCalledWith({version:'v1',choices:[{indices:[0,1],keeperIndex:1}]});expect(render().message).toBe('identityRepair.savedPending');expect(render().done).toBe(true);});
 it('invalidates choices after stale rejection and never automatically retries',async()=>{await mount();h.repair.mockRejectedValue(new IpcError('server_identity_repair','stale','serverIdentityRevisionChanged'));await choose().save();expect(render().preview).toBeNull();expect(render().keepers).toEqual({});expect(render().acknowledged).toBe(false);expect(render().message).toBe('identityRepair.stale');expect(h.repair).toHaveBeenCalledTimes(1);});
 it('keeps successful save distinct from readback failure',async()=>{await mount();h.refresh.mockRejectedValue(new Error('offline'));await choose().save();expect(render().done).toBe(true);expect(render().message).toBe('identityRepair.savedRefreshFailed');await render().save();expect(h.repair).toHaveBeenCalledTimes(1);});
 it('retries hydration after saved repair without repeating the repair',async()=>{await mount();h.refresh.mockRejectedValueOnce(new Error('maintenance failed'));await choose().save();expect(render().message).toBe('identityRepair.savedRefreshFailed');await render().retryRefresh();expect(h.repair).toHaveBeenCalledTimes(1);expect(h.refresh).toHaveBeenLastCalledWith({fresh:true,reason:'identity-repair-retry'});expect(render().message).toBe('identityRepair.savedPending');});
 it('does not overwrite UI after unmount',async()=>{await mount();let finish!:(value:unknown)=>void;h.repair.mockImplementation(()=>new Promise(resolve=>{finish=resolve;}));const pending=choose().save();h.cleanup?.();finish({status:'saved',requiresApply:false});await pending;expect(render().done).toBe(false);expect(h.refresh).not.toHaveBeenCalled();});
});
