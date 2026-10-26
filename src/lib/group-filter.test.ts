import { describe, expect, it } from 'vitest';
import type { IntegrationSnapshot } from './integrations';
import { groupAccounts } from './group-filter';
const accounts: IntegrationSnapshot['accounts'] = ['a', 'b', 'c'].map((id) => ({
  id: `public-${id}`,
  profileId: `profile-${id}`,
  name: 'Nome repetido',
  email: null,
  plan: null,
  policy: 'desktop_only',
  capacity: 1,
  applicationSessionConnected: false,
}));
const snapshot: IntegrationSnapshot = {
  accounts,
  applications: [],
  bindings: [],
  grants: [],
  groups: [
    {
      id: 'one',
      name: 'Um',
      accountIds: ['public-a', 'public-b'],
      archived: false,
      revision: 1,
      exclusiveApplicationId: null,
    },
    {
      id: 'two',
      name: 'Dois',
      accountIds: ['public-b', 'public-c'],
      archived: false,
      revision: 1,
      exclusiveApplicationId: null,
    },
  ],
};
describe('contextual group membership', () => {
  it('maps public membership to profiles without merging names', () => {
    expect(groupAccounts(snapshot, 'one').map((account) => account.profileId)).toEqual([
      'profile-a',
      'profile-b',
    ]);
    expect(groupAccounts(snapshot, 'two').map((account) => account.profileId)).toEqual([
      'profile-b',
      'profile-c',
    ]);
  });
  it('all groups includes each account once', () => {
    expect(groupAccounts(snapshot, '')).toEqual(accounts);
  });
  it('unknown or archived groups do not leak unrelated members into a filtered list', () => {
    expect(groupAccounts(snapshot, 'missing')).toEqual([]);
    expect(
      groupAccounts(
        { ...snapshot, groups: snapshot.groups.map((group) => ({ ...group, archived: true })) },
        'one',
      ),
    ).toEqual([]);
  });
});
