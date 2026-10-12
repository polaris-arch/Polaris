import { describe, expect, it } from 'vitest';
import { groupServersBySubscription, nodeViewGroups } from './server-grouping';
import { projectVisibleServers } from '@/components/screens/nodes/nodes-list-projection';
import { initialNodesTab } from '@/components/screens/nodes/initial-tab';
import { measurementServer } from '@/store/measurement-fixture.test-support';

describe('all view is a projection, not a second node or automatic group', () => {
  it('keeps same-name node IDs and source labels while selectors retain the original grouping', () => {
    const servers = [{ ...measurementServer('a'), name: 'Same', subscriptionId: 'one' },
      { ...measurementServer('b'), name: 'Same', subscriptionId: 'two' }, measurementServer('c')];
    const subscriptions = [{ id: 'one', name: 'Alpha', url: '' }, { id: 'two', name: 'Beta', url: '' }, { id: 'empty', name: 'Empty', url: '' }];
    const groups = nodeViewGroups(servers, subscriptions);
    const all = groups.find(group => group.isAll)!;
    expect(all.servers).toBe(servers); expect(all.servers.map(server => server.id)).toEqual(['a','b','c']);
    expect(all.sources?.a.name).toBe('Alpha'); expect(all.sources?.b.name).toBe('Beta');
    expect(groups.find(group => group.id === 'empty')?.servers).toEqual([]);
    expect(groupServersBySubscription(servers, subscriptions).some(group => group.isAll)).toBe(false);
    expect(initialNodesTab(groups, 'a')).toBe('one'); expect(initialNodesTab(groups, undefined)).toBe('manual');
    expect(projectVisibleServers(all, 'beta', '', 'default', {})).toEqual([servers[1]]);
    expect(projectVisibleServers(all, '', '', 'lat', { a: 40, b: 10 }).map(server => server.id)).toEqual(['b','a','c']);
  });
  it('cannot shadow a subscription whose stable ID happens to match the view namespace', () => {
    const groups = nodeViewGroups([], [{ id: '__all_nodes__', name: 'Existing', url: '' }]);
    expect(new Set(groups.map(group => group.id)).size).toBe(groups.length);
    expect(groups.find(group => group.id === '__all_nodes__')?.isAll).not.toBe(true);
  });
});
