import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { IPC_CHANNELS } from '@/domain/ipc-channels';
import type { SubscriptionConfig } from '@/contracts/types';
const transport = vi.hoisted(() => vi.fn());
vi.mock('@tauri-apps/api/core', () => ({ invoke: transport }));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn() }));
const { subscriptionApi } = await import('./subscriptions');
const { IpcError } = await import('../ipc-client');
const subscription = {id:'fixture',name:'Fixture',url:'https://example.invalid/sub',autoUpdate:false} as SubscriptionConfig;
const editRequestId = '00000000-0000-4000-8000-000000000001';

describe('shared desktop/mobile async subscription edit transport', () => {
  beforeEach(() => { transport.mockReset(); vi.stubGlobal('window', {__TAURI_INTERNALS__:{}}); });
  afterEach(() => vi.unstubAllGlobals());
  it.each([null, {success:true,addedServers:1,updatedServers:0,deletedServers:0},
    {success:false,errorKind:'timeout',addedServers:0,updatedServers:0,deletedServers:0}])(
    'unwraps the original saved envelope without mistaking refresh outcome for save failure: %j', async refresh => {
      // Tauri resolves the Ok payload, not a JSON {Ok: ...} enum wrapper.
      const data = {saved:true,editRequestId,publication:'scheduled',refresh};
      transport.mockResolvedValueOnce({success:true,data});
      expect(await subscriptionApi.update(subscription,editRequestId)).toEqual(data);
      expect(transport).toHaveBeenCalledExactlyOnceWith(IPC_CHANNELS.SUBSCRIPTION_UPDATE,{subscription,editRequestId});
    });
  it('preserves a save failure envelope as IpcError without retrying or fabricating saved data', async () => {
    transport.mockResolvedValueOnce({success:false,error:'storage failed'});
    const error = await subscriptionApi.update(subscription,editRequestId).catch(error => error);
    expect(error).toBeInstanceOf(IpcError);
    expect(error.command).toBe(IPC_CHANNELS.SUBSCRIPTION_UPDATE);
    expect(error.message).toBe('storage failed');
    expect(error.data).toBeUndefined();
    expect(transport).toHaveBeenCalledTimes(1);
  });
  it('preserves transport rejection separately and never replays a mutation', async () => {
    const error = new Error('synthetic transport failure');
    transport.mockRejectedValueOnce(error);
    await expect(subscriptionApi.update(subscription,editRequestId)).rejects.toBe(error);
    expect(transport).toHaveBeenCalledTimes(1);
  });
  it('keeps legacy calls without an edit request identity compatible', async () => {
    const data = {saved:true,editRequestId:null,publication:'scheduled',refresh:null};
    transport.mockResolvedValueOnce({success:true,data});
    expect(await subscriptionApi.update(subscription)).toEqual(data);
    expect(transport).toHaveBeenCalledExactlyOnceWith(IPC_CHANNELS.SUBSCRIPTION_UPDATE,{subscription,editRequestId:undefined});
  });
});
