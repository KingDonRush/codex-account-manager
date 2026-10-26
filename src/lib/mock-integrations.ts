import { api } from './api';
import type {
  nativeIntegrationApi,
  AccountGroup,
  Application,
  ExecutionBinding,
  AccountPolicy,
  IntegrationSnapshot,
  AuthorizationRequest,
  AuthorizationGrant,
  ExecutionStatus,
} from './integrations';

/** UI fixture only; security and runtime conformance use the actual Rust backend. */
export function createMockIntegrationApi(): typeof nativeIntegrationApi {
  const groups = new Map<string, AccountGroup>();
  const apps = new Map<string, Application>();
  const bindings = new Map<string, ExecutionBinding>();
  const options = new Map<string, { policy: AccountPolicy; capacity: number }>();
  const sessions = new Set<string>();
  const pendingLogins = new Map<string, string>();
  const grants = new Map<string, AuthorizationGrant>();
  const fixture = new URLSearchParams(window.location.search);
  let snapshotFailure = false;
  let pending: AuthorizationRequest | null =
    fixture.get('fixtureConsent') === 'pending'
      ? {
          id: 'fixture-request',
          peerPid: 12345,
          executable: '/fixture/codex-account-manager-cli',
          confirmationCode: 'ABCD-EFGH',
          request: {
            applicationId: null,
            name: 'Aplicação de demonstração',
            slug: 'demo-application',
            requestedScope: {
              accountIds: [],
              groupIds: [],
              permissions: ['read_metadata', 'execute'],
              globalIncludingFuture: false,
              backgroundGroupIds: [],
              expiresAt: null,
              permanent: false,
            },
          },
        }
      : null;
  const serverHistory =
    new URLSearchParams(window.location.search).get('fixtureHistory') === 'closed'
      ? [
          {
            id: 'fixture-server',
            applicationId: 'fixture-app',
            bindingId: 'fixture-binding',
            bindingRevision: 1,
            accountId: 'account:profile-work',
            state: 'closed',
            persistHistory: true,
            background: false,
            errorCode: null as string | null,
          },
        ]
      : [];
  const recovery = new Set<string>(
    new URLSearchParams(window.location.search).get('fixtureRecovery') === 'pending'
      ? ['profile-work']
      : [],
  );
  const jobHistory: ExecutionStatus[] =
    fixture.get('fixtureExecutions') === 'pending'
      ? ['completed', 'failed', 'cancelled'].map((state, index) => ({
          id: `fixture-exec-${index}`,
          applicationId: 'fixture-app',
          bindingId: '',
          accountId: 'account:profile-work',
          state,
          errorCode: 'credential_recovery_pending',
          startedAt: new Date().toISOString(),
          finishedAt: new Date().toISOString(),
          background: false,
        }))
      : [];
  if (jobHistory.length) recovery.add('profile-work');
  function save<T extends { id: string; revision: number }>(map: Map<string, T>, input: T): T {
    const previous = map.get(input.id);
    if ((previous?.revision ?? 0) !== input.revision) throw new Error('Objeto mudou; releia.');
    const saved = { ...structuredClone(input), revision: input.revision + 1 };
    map.set(saved.id, saved);
    return structuredClone(saved);
  }
  return {
    clearHistory: async () => {
      const session = serverHistory[0];
      if (session) {
        session.persistHistory = false;
        session.errorCode = 'history_cleared';
      }
    },
    exportHistory: async () => 'fixture-history.zip',
    recoverCredential: async (profileId) => {
      recovery.delete(profileId);
      sessions.add(profileId);
    },
    deletionRequests: async () => [],
    requestDeletion: async () => {
      throw new Error('Destruição é testada no núcleo Rust.');
    },
    confirmDeletion: async () => {
      throw new Error('Destruição é testada no núcleo Rust.');
    },
    denyDeletion: async () => {},
    applicationServers: async () => structuredClone(serverHistory),
    closeApplicationServer: async () => {},
    snapshot: async (): Promise<IntegrationSnapshot> => {
      if (snapshotFailure) {
        snapshotFailure = false;
        throw new Error('Atualização sintética falhou depois do consentimento.');
      }
      const operations = await api.listOperations();
      for (const [profile, operation] of pendingLogins) {
        if (operations.some((item) => item.id === operation && item.state === 'completed')) {
          sessions.add(profile);
          pendingLogins.delete(profile);
        }
      }
      if (
        new URLSearchParams(window.location.search).get('fixturePrepareSwitch') ===
        'delayed_failure'
      ) {
        await new Promise((resolve) => window.setTimeout(resolve, 1000));
        throw Object.assign(
          new Error('Preparação sintética falhou; a conta ativa não foi alterada.'),
          { code: 'credential_unavailable' },
        );
      }
      return {
        accounts: (await api.listProfiles())
          .filter((profile) => profile.accountId)
          .map((profile) => ({
            id: `account:${profile.id}`,
            profileId: profile.id,
            name: profile.name,
            email: profile.email,
            plan: profile.planType,
            policy: options.get(`account:${profile.id}`)?.policy ?? 'desktop_only',
            capacity: options.get(`account:${profile.id}`)?.capacity ?? 1,
            applicationSessionConnected: sessions.has(profile.id),
            ...(fixture.get('fixtureAccountAuth') === 'shared'
              ? { accountCredentialState: profile.credentialState }
              : {}),
            applicationSessionRecoveryPending: recovery.has(profile.id),
          })),
        groups: [...groups.values()],
        applications: [...apps.values()],
        bindings: [...bindings.values()],
        grants: [...grants.values()],
      };
    },
    saveGroup: async (group) => save(groups, group),
    deleteGroup: async (id, revision) => {
      if (groups.get(id)?.revision !== revision) throw new Error('Grupo mudou.');
      groups.delete(id);
    },
    saveApplication: async (app) => save(apps, app),
    saveBinding: async (binding) => save(bindings, binding),
    accountPolicy: async (id, policy, capacity) => {
      options.set(id, { policy, capacity });
    },
    mailPreference: async () => {},
    executions: async () => structuredClone(jobHistory),
    cancelExecution: async () => {},
    applicationLogin: async (profileId) => {
      const operation = await api.startLogin(profileId, true, null);
      pendingLogins.set(profileId, operation.id);
      return operation;
    },
    requests: async () => (pending ? [structuredClone(pending)] : []),
    approve: async (id, scope, setup) => {
      if (!pending || pending.id !== id) throw new Error('Pedido indisponível.');
      const app = save(apps, {
        id: 'fixture-app',
        name: pending.request.name,
        slug: pending.request.slug,
        paused: false,
        revision: 0,
      });
      const grant: AuthorizationGrant = {
        id: 'fixture-grant',
        applicationId: app.id,
        scope: structuredClone(scope),
        revoked: false,
        createdAt: new Date().toISOString(),
      };
      grants.set(grant.id, grant);
      for (const account of setup?.sharedAccountIds ?? []) {
        const previous = options.get(account);
        if (!previous || previous.policy === 'desktop_only')
          options.set(account, { policy: 'shared', capacity: previous?.capacity ?? 1 });
      }
      pending = null;
      snapshotFailure = fixture.get('fixtureConsentRefresh') === 'fail_once';
      return grant.id;
    },
    deny: async () => {
      pending = null;
    },
    revoke: async (id) => {
      const grant = grants.get(id);
      if (grant) grants.set(id, { ...grant, revoked: true });
    },
  };
}
