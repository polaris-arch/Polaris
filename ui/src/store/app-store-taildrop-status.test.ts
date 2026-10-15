import { beforeEach, describe, expect, it } from 'vitest';
import type { TailscaleStatusEvent } from '@/contracts/tailscale-status';
import { useAppStore } from './app-store';
const frame: TailscaleStatusEvent = { serverId: 'ts', backendState: 'Running', loggedIn: true, expired: false, tailscaleIPs: [], peers: [], canShareFiles: false, waitingFileCount: 0, receivingFileCount: 0, unreadFileCount: 0 };
beforeEach(() => useAppStore.setState({ tailscaleStatuses: {} }));
describe('independent Taildrop status publication', () => {
  it.each(['canShareFiles', 'waitingFileCount', 'receivingFileCount', 'unreadFileCount'] as const)('publishes only %s changed', (key) => {
    useAppStore.getState().setTailscaleStatus(frame);
    const before = useAppStore.getState().tailscaleStatuses;
    useAppStore.getState().setTailscaleStatus({ ...frame, [key]: key === 'canShareFiles' ? true : 1 });
    expect(useAppStore.getState().tailscaleStatuses).not.toBe(before);
    const after = useAppStore.getState().tailscaleStatuses;
    useAppStore.getState().setTailscaleStatus({ ...after.ts!, peers: [] });
    expect(useAppStore.getState().tailscaleStatuses).toBe(after);
  });
});
