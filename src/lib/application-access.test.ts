import { expect, it } from 'vitest';
import type {
  AuthorizationGrant,
  GrantScope,
  IntegrationAccount,
  IntegrationSnapshot,
} from './integrations';
import {
  applicationAccounts,
  executableApplicationAccounts,
  applicationReadiness,
  applicationSessionPresentation,
  executionOutcomeLabel,
} from './application-access';
it('preservation warnings survive every execution outcome and do not invent current recovery status from history', () => {
  const current = { ...account('a'), applicationSessionRecoveryPending: true };
  for (const [state, label] of [
    ['completed', 'Concluído'],
    ['failed', 'Falhou'],
    ['cancelled', 'Cancelado'],
  ]) {
    const job = { state: state!, errorCode: 'credential_recovery_pending' };
    expect(executionOutcomeLabel(job, current)).toBe(`${label} · gravação no cofre pendente`);
    expect(executionOutcomeLabel(job, account('a'))).toBe(`${label} · houve pendência de sessão`);
    expect(executionOutcomeLabel(job, undefined)).toBe(`${label} · houve pendência de sessão`);
  }
  expect(executionOutcomeLabel({ state: 'future_state', errorCode: null }, undefined)).toBe(
    'Estado não reconhecido',
  );
});
const account = (id: string): IntegrationAccount => ({
  id,
  profileId: `profile-${id}`,
  name: id,
  email: null,
  plan: 'plus',
  policy: 'shared',
  capacity: 1,
  applicationSessionConnected: true,
});
const scope = (overrides: Partial<GrantScope> = {}): GrantScope => ({
  accountIds: [],
  groupIds: [],
  permissions: ['execute'],
  globalIncludingFuture: false,
  backgroundGroupIds: [],
  expiresAt: null,
  permanent: true,
  ...overrides,
});
const grant = (id: string, selected: GrantScope): AuthorizationGrant => ({
  id,
  applicationId: 'app',
  scope: selected,
  revoked: false,
  createdAt: '2026-10-09T12:00:00Z',
});
function snapshot(grants: AuthorizationGrant[]): IntegrationSnapshot {
  return {
    accounts: [account('a'), account('b'), account('future')],
    groups: [],
    applications: [],
    bindings: [],
    grants,
  };
}
it('read-only resources do not inherit execute from a different grant of the same app', () => {
  const data = snapshot([
    grant('read', scope({ accountIds: ['a'], permissions: ['read_metadata'] })),
    grant('exec', scope({ accountIds: ['b'] })),
  ]);
  expect(applicationAccounts(data, 'app').map((item) => item.id)).toEqual(['a', 'b']);
  expect(executableApplicationAccounts(data, 'app')).toEqual(['b']);
});
it('group access excludes paused groups, future members and revoked grants', () => {
  const data = snapshot([
    grant('group', scope({ accountIds: ['a'], directAccountIds: [], groupIds: ['g'] })),
  ]);
  data.groups = [
    {
      id: 'g',
      name: 'Group',
      accountIds: ['a', 'future'],
      archived: false,
      paused: false,
      revision: 1,
      exclusiveApplicationId: null,
    },
  ];
  expect(executableApplicationAccounts(data, 'app')).toEqual(['a']);
  data.groups[0]!.paused = true;
  expect(applicationAccounts(data, 'app')).toEqual([]);
  data.grants[0]!.scope.directAccountIds = ['a'];
  expect(applicationAccounts(data, 'app').map((item) => item.id)).toEqual(['a']);
  data.grants[0]!.revoked = true;
  expect(executableApplicationAccounts(data, 'app')).toEqual([]);
});
it('stored login metadata is not presented as live authentication or quota verification', () => {
  expect(applicationReadiness(account('a'))).toMatchObject({
    label: 'Sessão registrada',
    kind: 'saved',
  });
  expect(applicationReadiness({ ...account('a'), applicationSessionConnected: false }).kind).toBe(
    'login',
  );
  expect(
    applicationReadiness({ ...account('a'), applicationSessionRecoveryPending: true }).kind,
  ).toBe('recovery');
});

it('a connected account is reusable without a separate application login, but is validated at execution', () => {
  const state = applicationReadiness({
    ...account('a'),
    applicationSessionConnected: false,
    accountCredentialState: 'ready',
  });
  expect(state.kind).toBe('saved');
  expect(state.label).toBe('Usa a conta conectada');
  expect(state.actionLabel).toBeNull();
  expect(state.guidance).toContain('validada na execução');
  expect(
    applicationReadiness({
      ...account('a'),
      applicationSessionConnected: false,
      accountCredentialState: 'unavailable',
    }).kind,
  ).toBe('unverified');
});

it('unknown registration metadata is neither ready nor evidence that another login is needed', () => {
  const unknown = JSON.parse(
    JSON.stringify({
      ...account('a'),
      applicationSessionConnected: undefined,
    }),
  ) as IntegrationAccount;
  expect(applicationReadiness(unknown)).toMatchObject({
    label: 'Sessão não verificada',
    kind: 'unverified',
  });
  expect(applicationSessionPresentation(unknown).actionLabel).toBeNull();
  const unknownPolicy = JSON.parse(
    JSON.stringify({ ...account('a'), policy: 'future_policy' }),
  ) as IntegrationAccount;
  expect(applicationReadiness(unknownPolicy).kind).toBe('unverified');
});
