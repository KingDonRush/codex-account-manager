import type { AccountManagerApi } from './api';
import type {
  AccountCard,
  DashboardSnapshot,
  LoginOperation,
  MailProvider,
  ManagedProfile,
  ManagerSettings,
  OperationEvent,
  OperationStage,
  OperationState,
  RuntimeStatus,
  SaveMailProviderInput,
} from './types';

const now = new Date().toISOString();

export function createMockApi(): AccountManagerApi {
  let profiles = initialProfiles();
  const fixture = new URLSearchParams(window.location.search);
  let switchRequests = 0;
  let desktopRequests = 0;
  const recordDesktopRequests = () => {
    document.documentElement.dataset.mockSwitchRequests = String(switchRequests);
    document.documentElement.dataset.mockDesktopRequests = String(desktopRequests);
  };
  if (fixture.get('fixtureLabels') === 'long') {
    profiles = profiles.map((profile) =>
      profile.id === 'profile-work'
        ? {
            ...profile,
            name: 'Identidade de teste com nome extenso para validar limites da linha',
            email: 'identidade.com.nome.extenso.para.testes@example.test',
          }
        : profile,
    );
  }
  if (fixture.get('fixtureCheck') === 'renewable') {
    profiles = profiles.map((profile) =>
      profile.id === 'profile-work' ? { ...profile, credentialState: 'quarantined' } : profile,
    );
  }
  if (new URLSearchParams(window.location.search).get('fixtureCoffer') === 'unavailable') {
    profiles = profiles.map((profile) =>
      profile.isActiveAccount
        ? profile
        : {
            ...profile,
            credentialState: profile.accountId ? 'unavailable' : profile.credentialState,
          },
    );
  }
  const fixtureAccounts = Number(
    new URLSearchParams(window.location.search).get('fixtureAccounts') ?? 3,
  );
  if (fixtureAccounts > 3 && fixtureAccounts <= 30) {
    const template = profiles[1];
    if (template)
      profiles = [
        ...profiles,
        ...Array.from({ length: fixtureAccounts - 3 }, (_, index) => ({
          ...structuredClone(template),
          id: `fixture-${index}`,
          accountId: `fixture-account-${index}`,
          name: `Conta de teste ${index + 1}`,
          planType: fixture.get('fixturePlans') === 'mixed' ? ['pro','free','unknown_plan'][index % 3]! : template.planType,
          email: `fixture${index}@example.test`,
        })),
      ];
  }
  let providers = initialProviders();
  let operations: LoginOperation[] = [];
  let settings: ManagerSettings = { rotationStrategy: 'before_reset' };
  const runtime = initialRuntime();
  let operationListener: ((event: OperationEvent) => void) | null = null;
  let dashboardListener: (() => void) | null = null;

  return {
    retryBrowserCleanup: async (operationId) => {
      document.documentElement.dataset.mockBrowserCleanupRequests = String(Number(document.documentElement.dataset.mockBrowserCleanupRequests ?? '0') + 1);
      operations = operations.map(operation => operation.id === operationId ? { ...operation, step: operation.state === 'completed' ? 'Conta conectada; navegador fechado' : 'Operação cancelada; navegador fechado', errorCode: null, errorMessage: null } : operation);
    },
    recoverSavedAccounts: async () => 0,
    focusCodex: async (operationId) => {
      desktopRequests += 1;
      recordDesktopRequests();
      await new Promise(resolve => window.setTimeout(resolve, 200));
      if (fixture.get('fixtureDesktopOpen') === 'failure') throw Object.assign(new Error('A abertura sintética não foi confirmada.'), { code: 'desktop_launch_failed' });
      operations = operations.map(item => item.id === operationId && item.state === 'completed' ? { ...item, step: 'Sessão ativada e abertura do Codex confirmada', errorCode: null, errorMessage: null } : item);
    },
    selectCodexDesktop: async () => true,
    confirmCodexClose: async () => false,
    inspectSharedRuntime: async () => ({
      state: 'idle',
      pid: 12345,
      threads: [
        {
          id: '01a0dabe-0cc8-7aa0-a327-dd24d42869a3',
          name: 'Chat de teste',
          state: 'idle',
          activeGoal: false,
          backgroundTerminals: 0,
        },
      ],
      externalCalls: [],
      externalInspectionSupported: true,
      explanation: 'Servidor sem trabalho ativo.',
    }),
    recoverOriginalChat: async (_thread, consent) => ({
      recovered: consent !== null,
      needsConfirmation: consent === null,
      affectedThreads: 2,
      confirmation: consent
        ? null
        : { pid: 12345, startedTicks: 1, threadIds: ['original', 'second'] },
      explanation: consent
        ? 'Chat original liberado, sem fork.'
        : 'O servidor tem trabalho ativo. Aguarde ou confirme a interrupção de 2 chats.',
    }),
    openOriginalChat: async () => {},
    subscribeToShutdown: async () => () => {},
    unlinkProfile: async (profileId) => {
      updateProfile(profileId, {
        accountId: null,
        email: null,
        credentialState: 'not_connected',
        rateLimits: null,
        planType: null,
        mailUrl: null,
        mailProviderId: null,
      });
    },
    logoutProfile: async (profileId) => {
      updateProfile(profileId, { isActiveAccount: false });
    },
    startReplacement: async (profileId) => {
      const profile = profiles.find((item) => item.id === profileId);
      if (profile?.isActiveAccount) throw new Error('Use outra conta antes de substituir.');
      const operation = newOperation(profileId, false, 'replace');
      operations = [operation, ...operations];
      queueOperationProgress(operation, (event) => {
        if (operations.find((item) => item.id === operation.id)?.state === 'cancelled') return;
        operations = operations.map((item) =>
          item.id === operation.id ? { ...item, state: event.state, step: event.step } : item,
        );
        operationListener?.(event);
      });
      return structuredClone(operation);
    },
    replacementPreview: async (operationId) => {
      const operation = operations.find((item) => item.id === operationId);
      if (operation?.state !== 'awaiting_confirmation') return null;
      return {
        previousEmail: profiles.find((item) => item.id === operation.profileId)?.email ?? null,
        nextEmail: 'nova.conta@proton.me',
        nextPlan: 'plus',
      };
    },
    confirmReplacement: async (operationId) => {
      const operation = operations.find((item) => item.id === operationId);
      if (operation?.state !== 'awaiting_confirmation')
        throw new Error('Confirmação indisponível.');
      updateProfile(operation.profileId, {
        accountId: 'replacement-account',
        email: 'nova.conta@proton.me',
        mailProviderId: 'proton',
        mailUrl: null,
        rateLimits: null,
        usageUpdatedAt: null,
        credentialState: 'ready',
        lastBrowserAuthenticatedAt: new Date().toISOString(),
      });
      operations = operations.map((item) =>
        item.id === operationId
          ? { ...item, state: 'completed', step: 'Conta substituída', cancelable: false }
          : item,
      );
      operationListener?.({
        operationId,
        profileId: operation.profileId,
        kind: 'replace',
        state: 'completed',
        step: 'Conta substituída',
        message: 'Conta substituída',
        stageDurationMs: 100,
        cancelable: false,
      });
    },
    settings: async () => structuredClone(settings),
    saveSettings: async (value) => {
      settings = value;
      dashboardListener?.();
    },
    saveProfileOptions: async (profileId, options) => {
      updateProfile(profileId, options);
    },
    openProfileBrowser: async (profileId) => {
      updateProfile(profileId, { lastBrowserOpenedAt: new Date().toISOString() });
    },
    checkProfile: async (profileId) => {
      if (fixture.get('fixtureCheck') === 'changed') return {
        confirmed: false,
        canRenew: false,
        title: 'A conta ou sessão mudou',
        guidance: 'A amostra anterior foi preservada. Confira a associação atual do perfil antes de verificar novamente; nenhuma conta diferente foi confirmada por esta consulta.',
        errorCode: 'query_context_changed',
        checkedAt: new Date().toISOString(),
      };
      if (fixture.get('fixtureCheck') === 'renewable' && profileId === 'profile-work')
        return {
          confirmed: false,
          canRenew: true,
          title: 'Consulta não autenticada',
          guidance: 'Fixture de sessão a verificar; nenhuma conta real é utilizada.',
          errorCode: 'rpc_error',
          checkedAt: new Date().toISOString(),
        };
      if (profiles.find((profile) => profile.id === profileId)?.credentialState === 'unavailable') {
        return {
          confirmed: false,
          canRenew: false,
          title: 'Cofre indisponível',
          guidance: 'A sessão salva não foi invalidada. Desbloqueie o cofre e tente novamente.',
          errorCode: 'credential_unavailable',
          checkedAt: new Date().toISOString(),
        };
      }
      updateProfile(profileId, { usageUpdatedAt: new Date().toISOString() });
      return {
        confirmed: true,
        canRenew: false,
        title: 'Conexão e saldo confirmados',
        guidance: 'A conta ativa não foi alterada.',
        errorCode: null,
        checkedAt: new Date().toISOString(),
      };
    },
    listProfiles: async () => structuredClone(profiles),
    dashboardSnapshot: async () => structuredClone(mockDashboard(profiles, settings)),
    listProviders: async () => structuredClone(providers),
    listOperations: async () => structuredClone(operations),
    operationStages: async (operationId) => {
      const kind = operations.find((operation) => operation.id === operationId)?.kind ?? 'login';
      return structuredClone(mockOperationStages(kind));
    },
    runtimeStatus: async () => structuredClone(runtime),
    createProfile: async (name, providerId, mailUrl) => {
      const profile = newProfile(name, providerId, mailUrl);
      profiles = [...profiles, profile];
      dashboardListener?.();
      return structuredClone(profile);
    },
    renameProfile: async (profileId, name) => updateProfile(profileId, { name }),
    updateProfileMailUrl: async (profileId, mailUrl) => updateProfile(profileId, { mailUrl }),
    archiveProfile: async (profileId) => updateProfile(profileId, { status: 'archived' }),
    restoreProfile: async (profileId) => updateProfile(profileId, { status: 'active' }),
    deleteProfile: async (profileId, permanently) => {
      profiles = permanently
        ? profiles.filter((profile) => profile.id !== profileId)
        : profiles.map((profile) =>
            profile.id === profileId ? { ...profile, status: 'archived' } : profile,
          );
    },
    openMail: async () => undefined,
    openProfileFolder: async () => undefined,
    startLogin: async (profileId, reconnect) => {
      const failure = new URLSearchParams(window.location.search).get('fixtureStartError');
      if (failure)
        throw Object.assign(new Error('Falha de início sintética, sem alteração de conta.'), {
          code: failure,
        });
      const operation = newOperation(profileId, reconnect);
      operations = [operation, ...operations];
      queueOperationProgress(operation, (event) => {
        if (event.state === 'completed' && fixture.get('fixtureBrowserCleanup') === 'pending') event = { ...event, step: 'Conta conectada; o fechamento do navegador ainda não foi confirmado', message: 'Conta conectada; o fechamento do navegador ainda não foi confirmado', errorCode: 'browser_cleanup_pending' };
        if (operations.find((item) => item.id === operation.id)?.state === 'cancelled') return;
        if (event.state === 'completed') {
          const previous = profiles.find((profile) => profile.id === profileId);
          updateProfile(profileId, {
            accountId: previous?.accountId ?? `synthetic-${profileId}`,
            email: previous?.email ?? 'fixture@gmail.com',
            mailProviderId: previous?.mailProviderId ?? 'gmail',
            credentialState: 'ready',
            lastBrowserAuthenticatedAt: new Date().toISOString(),
          });
        }
        operations = operations.map((item) =>
          item.id === event.operationId
            ? {
                ...item,
                state: event.state,
                step: event.step,
                errorCode: event.errorCode ?? null,
                errorMessage: event.errorCode ? event.message : null,
                lastStageDurationMs: event.stageDurationMs,
                finishedAt:
                  event.state === 'completed' ? new Date().toISOString() : item.finishedAt,
                cancelable: event.cancelable,
              }
            : item,
        );
        operationListener?.(event);
      });
      return structuredClone(operation);
    },
    startSwitchAccount: async (profileId) => {
      switchRequests += 1;
      recordDesktopRequests();
      const operation = newOperation(profileId, false, 'switch');
      operations = [operation, ...operations];
      queueOperationProgress(operation, (event) => {
        const sessionError = fixture.get('fixtureSwitchError');
        if (event.state === 'completed' && sessionError && switchRequests === 1)
          event = { ...event, state: 'failed', step: 'Não foi possível confirmar a sessão após a tentativa de renovação. A sessão salva foi preservada.', errorCode: sessionError, message: 'Não foi possível confirmar a sessão após a tentativa de renovação. A sessão salva foi preservada.' };
        if (event.state === 'completed' && fixture.get('fixtureDesktop') === 'reopen_pending') event = { ...event, step: 'Sessão ativada; a abertura do Codex não foi confirmada', errorCode: 'desktop_reopen_pending', message: 'A sessão foi ativada, mas a abertura sintética falhou.' };
        if (event.state === 'completed' && fixture.get('fixtureDesktop') === 'discovery_failed') event = { ...event, state: 'failed', step: 'Instalação do Codex não encontrada; a conta anterior foi preservada', errorCode: 'desktop_discovery_failed', message: 'Descoberta sintética indisponível.' };
        operations = operations.map((item) =>
          item.id === event.operationId
            ? {
                ...item,
                state: event.state,
                step: event.step,
                errorCode: event.errorCode ?? null,
                errorMessage: event.errorCode ? event.message : null,
                lastStageDurationMs: event.stageDurationMs,
                finishedAt:
                  event.state === 'completed' || event.state === 'failed' ? new Date().toISOString() : item.finishedAt,
                cancelable: event.cancelable,
              }
            : item,
        );
        if (event.state === 'completed') {
          profiles = profiles.map((profile) => ({
            ...profile,
            isActiveAccount: profile.id === profileId,
            credentialState: profile.id === profileId ? 'ready' : profile.credentialState,
            lastUsedAt: profile.id === profileId ? new Date().toISOString() : profile.lastUsedAt,
            lastActivatedAt:
              profile.id === profileId ? new Date().toISOString() : profile.lastActivatedAt,
            lastDisconnectedAt: profile.isActiveAccount
              ? new Date().toISOString()
              : profile.lastDisconnectedAt,
          }));
          dashboardListener?.();
        }
        operationListener?.(event);
      });
      return structuredClone(operation);
    },
    cancelOperation: async (operationId) => {
      const target = operations.find((operation) => operation.id === operationId);
      operations = operations.map((operation) =>
        operation.id === operationId
          ? {
              ...operation,
              state: 'cancelled',
              step: 'Cancelado',
              finishedAt: new Date().toISOString(),
              cancelable: false,
            }
          : operation,
      );
      if (target) {
        operationListener?.({
          operationId,
          profileId: target.profileId,
          state: 'cancelled',
          step: 'Cancelado',
          message: 'Cancelado',
          kind: target.kind,
          stageDurationMs: 0,
          cancelable: false,
        });
      }
    },
    exportDiagnostics: async () => 'diagnostics-sanitized.jsonl',
    checkForUpdate: async () => ({
      currentVersion: '0.4.0',
      version: '0.4.1',
      body: 'Melhorias de robustez e diagnóstico.',
      publishedAt: now,
    }),
    installUpdate: async () => undefined,
    saveProvider: async (input) => {
      const provider = providerFromInput(input);
      providers = [...providers.filter((item) => item.id !== provider.id), provider];
      return structuredClone(provider);
    },
    subscribeToOperations: async (handler) => {
      operationListener = handler;
      return () => {
        operationListener = null;
      };
    },
    subscribeToDashboardChanges: async (handler) => {
      dashboardListener = handler;
      return () => {
        dashboardListener = null;
      };
    },
    subscribeToUpdateProgress: async () => () => undefined,
  };

  function updateProfile(profileId: string, changes: Partial<ManagedProfile>): ManagedProfile {
    let updated: ManagedProfile | null = null;
    profiles = profiles.map((profile) => {
      if (profile.id !== profileId) return profile;
      updated = { ...profile, ...changes, updatedAt: now };
      return updated;
    });
    if (!updated) throw new Error(`Perfil não encontrado: ${profileId}`);
    dashboardListener?.();
    return structuredClone(updated);
  }
}

function mockOperationStages(kind: LoginOperation['kind']): OperationStage[] {
  return kind === 'switch'
    ? [
        { name: 'desktop_process_discovery', durationMs: 18 },
        { name: 'desktop_graceful_wait', durationMs: 420 },
        { name: 'app_server_spawn_initialize', durationMs: 310 },
        { name: 'app_server_account_read', durationMs: 280 },
        { name: 'app_server_rate_limits_read', durationMs: 190 },
        { name: 'desktop_restart', durationMs: 12 },
      ]
    : [
        { name: 'browser_runtime_ready', durationMs: 2 },
        { name: 'app_server_spawn_initialize', durationMs: 310 },
        { name: 'browser_process_launch', durationMs: 95 },
      ];
}

function initialProfiles(): ManagedProfile[] {
  const timestamp = Math.floor(Date.now() / 1000);
  return [
    {
      id: 'profile-yahoo',
      rotationEnabled: true,
      preferred: false,
      name: 'Conta pessoal',
      accountId: 'account-yahoo',
      email: 'conta@yahoo.com',
      mailProviderId: 'yahoo',
      mailUrl: null,
      status: 'active',
      lastUsedAt: now,
      lastActivatedAt: now,
      lastDisconnectedAt: null,
      lastBrowserOpenedAt: '2026-09-23T14:20:00Z',
      lastBrowserAuthenticatedAt: '2026-09-23T14:21:00Z',
      lastError: null,
      planType: 'plus',
      rateLimits: {
        primary: { usedPercent: 100, windowDurationMins: 300, resetsAt: timestamp - 60 },
        secondary: { usedPercent: 21, windowDurationMins: 10080, resetsAt: timestamp + 86400 },
        rateLimitReachedType: null,
        resetCredits: { availableCount: 2 },
      },
      usageUpdatedAt: new Date(Date.now() - 120_000).toISOString(),
      isActiveAccount: true,
      needsActivation: false,
      credentialState: 'ready',
      createdAt: now,
      updatedAt: now,
    },
    {
      id: 'profile-work',
      rotationEnabled: true,
      preferred: false,
      name: 'Trabalho',
      accountId: 'account-work',
      email: 'equipe@gmail.com',
      mailProviderId: 'gmail',
      mailUrl: null,
      status: 'active',
      lastUsedAt: '2026-09-22T19:10:00Z',
      lastActivatedAt: '2026-09-22T19:10:00Z',
      lastDisconnectedAt: '2026-09-23T10:00:00Z',
      lastBrowserOpenedAt: '2026-09-22T18:55:00Z',
      lastBrowserAuthenticatedAt: '2026-09-22T19:00:00Z',
      lastError: null,
      planType: 'team',
      rateLimits: {
        primary: { usedPercent: 74, windowDurationMins: 300, resetsAt: timestamp + 300 },
        secondary: { usedPercent: 46, windowDurationMins: 10080, resetsAt: timestamp + 86400 },
        rateLimitReachedType: null,
        resetCredits: { availableCount: 1 },
      },
      usageUpdatedAt: now,
      isActiveAccount: false,
      needsActivation: false,
      credentialState: 'ready',
      createdAt: now,
      updatedAt: now,
    },
    {
      id: 'profile-new',
      rotationEnabled: true,
      preferred: false,
      name: 'Conta nova',
      accountId: null,
      email: null,
      mailProviderId: 'outlook',
      mailUrl: null,
      status: 'active',
      lastUsedAt: null,
      lastActivatedAt: null,
      lastDisconnectedAt: null,
      lastBrowserOpenedAt: null,
      lastBrowserAuthenticatedAt: null,
      lastError: null,
      planType: null,
      rateLimits: null,
      usageUpdatedAt: null,
      isActiveAccount: false,
      needsActivation: false,
      credentialState: 'not_connected',
      createdAt: now,
      updatedAt: now,
    },
  ];
}

function initialProviders(): MailProvider[] {
  return [
    provider(
      'gmail',
      'Gmail / Google Workspace',
      ['gmail.com', 'googlemail.com'],
      'https://mail.google.com/',
    ),
    provider(
      'yahoo',
      'Yahoo',
      ['yahoo.com', 'yahoo.com.br', 'ymail.com'],
      'https://mail.yahoo.com/',
    ),
    provider('outlook', 'Outlook.com', ['outlook.com'], 'https://outlook.live.com/mail/'),
    provider(
      'hotmail',
      'Hotmail / Live',
      ['hotmail.com', 'live.com', 'msn.com'],
      'https://outlook.live.com/mail/',
    ),
    provider('proton', 'Proton Mail', ['proton.me', 'protonmail.com'], 'https://mail.proton.me/'),
    provider('tuta', 'Tuta', ['tuta.com', 'tutanota.com'], 'https://mail.tuta.com/'),
    provider(
      'icloud',
      'iCloud Mail',
      ['icloud.com', 'me.com', 'mac.com'],
      'https://www.icloud.com/mail/',
    ),
    provider('aol', 'AOL', ['aol.com'], 'https://mail.aol.com/'),
    provider('zoho', 'Zoho Mail', ['zoho.com', 'zohomail.com'], 'https://mail.zoho.com/'),
    provider('gmx', 'GMX', ['gmx.com', 'gmx.net', 'gmx.de'], 'https://www.gmx.com/'),
    provider('mail.com', 'Mail.com', ['mail.com'], 'https://www.mail.com/'),
    provider('fastmail', 'Fastmail', ['fastmail.com'], 'https://app.fastmail.com/'),
    provider('mailbox.org', 'Mailbox.org', ['mailbox.org'], 'https://mailbox.org/'),
    provider('posteo', 'Posteo', ['posteo.de', 'posteo.net'], 'https://posteo.de/'),
    provider('runbox', 'Runbox', ['runbox.com'], 'https://app.runbox.com/'),
    provider('migadu', 'Migadu', ['migadu.com', 'migadu.org'], 'https://web.migadu.com/'),
    provider('yandex', 'Yandex Mail', ['yandex.com', 'yandex.ru'], 'https://mail.yandex.com/'),
    provider('naver', 'Naver', ['naver.com'], 'https://mail.naver.com/'),
    provider('daum', 'Daum / Kakao', ['daum.net', 'hanmail.net'], 'https://mail.daum.net/'),
    provider('qq', 'QQ Mail', ['qq.com'], 'https://mail.qq.com/'),
    provider('163', '163 Mail', ['163.com'], 'https://mail.163.com/'),
    provider('126', '126 Mail', ['126.com'], 'https://mail.126.com/'),
    provider('sina', 'Sina Mail', ['sina.com', 'sina.cn'], 'https://mail.sina.com.cn/'),
    provider('webde', 'WEB.DE', ['web.de'], 'https://web.de/'),
    provider('orange', 'Orange Mail', ['orange.fr'], 'https://mail.orange.fr/'),
  ];
}

function provider(
  id: string,
  displayName: string,
  domains: string[],
  defaultWebmailUrl: string,
): MailProvider {
  return { id, displayName, domains, defaultWebmailUrl, aliases: [], isCustom: false };
}

function initialRuntime(): RuntimeStatus {
  return {
    runtimeKind: 'managed',
    version: '153.0.8010.12',
    installed: true,
    repairRequired: false,
    downloadSizeBytes: null,
  };
}

function newProfile(
  name: string,
  providerId: string | null,
  mailUrl: string | null,
): ManagedProfile {
  const id = `profile-${Date.now()}`;
  return {
    rotationEnabled: true,
    preferred: false,
    id,
    name,
    accountId: null,
    email: null,
    mailProviderId: providerId,
    mailUrl,
    status: 'active',
    lastUsedAt: null,
    lastActivatedAt: null,
    lastDisconnectedAt: null,
    lastBrowserOpenedAt: null,
    lastBrowserAuthenticatedAt: null,
    lastError: null,
    planType: null,
    rateLimits: null,
    usageUpdatedAt: null,
    isActiveAccount: false,
    needsActivation: false,
    credentialState: 'not_connected',
    createdAt: now,
    updatedAt: now,
  };
}

function newOperation(
  profileId: string,
  reconnect: boolean,
  kind: LoginOperation['kind'] = reconnect ? 'reconnect' : 'login',
): LoginOperation {
  return {
    id: `operation-${Date.now()}`,
    profileId,
    kind,
    reconnect,
    state: 'preparing',
    step: 'Preparando o ambiente isolado',
    startedAt: new Date().toISOString(),
    stageStartedAt: new Date().toISOString(),
    lastStageDurationMs: null,
    finishedAt: null,
    errorCode: null,
    errorMessage: null,
    browserPid: 4242,
    runtimeKind: 'managed',
    durationMs: null,
    cancelable: true,
  };
}

function queueOperationProgress(
  operation: LoginOperation,
  listener: (event: OperationEvent) => void,
): void {
  const states: Array<{ state: OperationState; step: string }> =
    operation.kind === 'replace'
      ? [
          { state: 'browser_launching', step: 'Abrindo outra conta no navegador' },
          { state: 'awaiting_login', step: 'Aguardando login no navegador' },
          { state: 'verifying', step: 'Validando a nova conta' },
          { state: 'awaiting_confirmation', step: 'Confirme a substituição da conta' },
        ]
      : operation.kind === 'switch'
        ? [
            { state: 'verifying', step: 'Validando a credencial e reiniciando o Codex' },
            { state: 'persisting', step: 'Atualizando quota e histórico' },
            { state: 'completed', step: 'Conta ativada no Codex' },
          ]
        : [
            { state: 'browser_launching', step: 'Abrindo OAuth e webmail' },
            { state: 'awaiting_login', step: 'Aguardando login no navegador' },
          ];
  if (
    operation.kind !== 'replace' &&
    operation.kind !== 'switch' &&
    new URLSearchParams(window.location.search).get('fixtureLogin') === 'success'
  ) {
    states.push(
      { state: 'verifying', step: 'Validando identidade' },
      { state: 'persisting', step: 'Salvando sessão' },
      { state: 'completed', step: 'Conta conectada' },
    );
  }
  states.forEach((next, index) => {
    window.setTimeout(
      () => {
        listener({
          operationId: operation.id,
          profileId: operation.profileId,
          kind: operation.kind,
          state: next.state,
          step: next.step,
          message: next.step,
          stageDurationMs: 100,
          cancelable: operationCancelable(operation.kind, next.state),
        });
      },
      100 * (index + 1),
    );
  });
}

function operationCancelable(kind: LoginOperation['kind'], state: OperationState): boolean {
  if (['completed', 'cancelled', 'failed', 'timed_out', 'persisting'].includes(state)) {
    return false;
  }
  return kind !== 'switch' || ['idle', 'preparing'].includes(state);
}

function providerFromInput(input: SaveMailProviderInput): MailProvider {
  return {
    id: input.id ?? `custom-${Date.now()}`,
    displayName: input.displayName,
    domains: input.domains,
    defaultWebmailUrl: input.defaultWebmailUrl,
    aliases: input.aliases,
    isCustom: input.id === null,
  };
}

function mockDashboard(profiles: ManagedProfile[], settings: ManagerSettings): DashboardSnapshot {
  const ordered = [...profiles].sort(
    (left, right) =>
      left.createdAt.localeCompare(right.createdAt) || left.id.localeCompare(right.id),
  );
  const accounts: AccountCard[] = ordered.map((profile) => {
    const health =
      profile.status === 'archived'
        ? 'archived'
        : !profile.accountId || profile.credentialState !== 'ready'
          ? 'attention'
          : 'connected';
    const primaryQuota = mockQuota(profile.rateLimits?.primary ?? null, profile.usageUpdatedAt);
    const secondaryQuota = mockQuota(profile.rateLimits?.secondary ?? null, profile.usageUpdatedAt);
    const quotas = [primaryQuota, secondaryQuota].filter(
      (quota): quota is NonNullable<typeof quota> => quota !== null,
    );
    const remaining = quotas.length
      ? Math.min(...quotas.map((quota) => quota.remaining ?? 0))
      : null;
    return {
      profile,
      health,
      attentionReason: health === 'attention' ? 'A conexão precisa ser validada.' : null,
      recommendation: null,
      effectiveRemaining: Number.isFinite(remaining) ? remaining : null,
      projectedReset: quotas.some((quota) => quota.projected),
      primaryQuota,
      secondaryQuota,
      nextResetAt: nextMockReset(quotas),
      availableAt: null,
    };
  });
  const eligible = accounts.filter(
    (card) =>
      card.health === 'connected' &&
      !card.profile.isActiveAccount &&
      card.profile.rotationEnabled &&
      (card.effectiveRemaining ?? 0) > 0,
  );
  eligible.sort(
    (a, b) =>
      Number(b.profile.preferred) - Number(a.profile.preferred) ||
      (settings.rotationStrategy === 'largest_balance'
        ? (b.effectiveRemaining ?? 0) - (a.effectiveRemaining ?? 0)
        : Date.parse(a.nextResetAt ?? '9999-01-01') - Date.parse(b.nextResetAt ?? '9999-01-01')) ||
      a.profile.id.localeCompare(b.profile.id),
  );
  const recommendation = eligible[0];
  if (recommendation && recommendation.effectiveRemaining !== null) {
    recommendation.recommendation = {
      label: 'Melhor próxima conta',
      effectiveRemaining: recommendation.effectiveRemaining,
      projectedReset: recommendation.projectedReset,
      reason: 'Saldo utilizável e renovação mais próxima.',
      nextResetAt: recommendation.nextResetAt,
    };
  }
  return {
    accounts,
    counts: {
      connected: accounts.filter((card) => card.health === 'connected').length,
      attention: accounts.filter((card) => card.health === 'attention').length,
      archived: accounts.filter((card) => card.health === 'archived').length,
    },
    generatedAt: new Date().toISOString(),
  };
}

function nextMockReset(quotas: import('./types').QuotaPresentation[]): string | null {
  const future = quotas
    .map((quota) => quota.resetsAt)
    .filter((at): at is number => at !== null && at * 1000 > Date.now());
  return future.length ? new Date(Math.min(...future) * 1000).toISOString() : null;
}

function mockQuota(
  window: import('./types').UsageWindow | null,
  observedAt: string | null,
): import('./types').QuotaPresentation | null {
  if (!window) return null;
  const projected =
    window.resetsAt !== null &&
    window.resetsAt * 1000 <= Date.now() &&
    (!observedAt || Date.parse(observedAt) < window.resetsAt * 1000);
  return {
    remaining: projected
      ? 100
      : window.usedPercent === null
        ? null
        : Math.max(0, 100 - window.usedPercent),
    projected,
    windowDurationMins: window.windowDurationMins,
    resetsAt: window.resetsAt,
  };
}
