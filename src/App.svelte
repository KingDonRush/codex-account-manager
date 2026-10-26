<script lang="ts">
  import { onMount } from 'svelte';
  import DiagnosticsPanel from './components/DiagnosticsPanel.svelte';
  import GroupsView from './components/GroupsView.svelte';
  import ApplicationsView from './components/ApplicationsView.svelte';
  import AuthorizationModal from './components/AuthorizationModal.svelte';
  import DeletionModal from './components/DeletionModal.svelte';
  import type { DeletionRequest } from './lib/integrations';
  import type { AuthorizationRequest } from './lib/integrations';
  import { integrationApi, type IntegrationSnapshot } from './lib/integrations';
  import LoginModal from './components/LoginModal.svelte';
  import SharedAccountModal from './components/SharedAccountModal.svelte';
  import CommandPalette from './components/CommandPalette.svelte';
  import type { PaletteCommand } from './lib/command-palette';
  import { profilePresentation } from './lib/view';
  import MailSettingsModal from './components/MailSettingsModal.svelte';
  import ProfileFormModal from './components/ProfileFormModal.svelte';
  import ProfilesView from './components/ProfilesView.svelte';
  import ProviderCatalog from './components/ProviderCatalog.svelte';
  import ProviderModal from './components/ProviderModal.svelte';
  import RenameModal from './components/RenameModal.svelte';
  import Sidebar from './components/Sidebar.svelte';
  import TopBar from './components/TopBar.svelte';
  import AccountManagementModal from './components/AccountManagementModal.svelte';
  import ConnectionCheckModal from './components/ConnectionCheckModal.svelte';
  import { api } from './lib/api';
  import {
    operationError,
    type OperationError,
    type RecoveryAction,
  } from './lib/operation-recovery';
  import type { IntegrationAccount } from './lib/integrations';
  import { RefreshCoordinator, type RefreshScope } from './lib/refresh-coordinator';
  import { reconcileOperation } from './lib/operation-reconciliation';
  import type {
    DashboardSnapshot,
    LoginOperation,
    MailProvider,
    ManagedProfile,
    ManagerSettings,
    ProfileOptions,
    ReplacementPreview,
    OperationEvent,
    OperationKind,
    RuntimeStatus,
    UpdateInfo,
    UpdateProgress,
    View,
    ShutdownState,
  } from './lib/types';

  let view: View = 'dashboard';
  let showPalette = false;
  let integrations: IntegrationSnapshot = {
    accounts: [],
    groups: [],
    applications: [],
    bindings: [],
    grants: [],
  };
  let authorizationRequest: AuthorizationRequest | null = null;
  let deletionRequest: DeletionRequest | null = null;
  let showGroups = false;
  let preparedApplicationId: string | null = null;
  async function applicationPrepared(id: string | null): Promise<void> {
    preparedApplicationId = id;
    selectView('applications');
    await refreshIntegrations();
    notice = 'Acesso autorizado. As contas já estão associadas à aplicação, sem criar vínculo.';
  }
  async function requestDeletion(selected: ManagedProfile[]): Promise<void> {
    if (import.meta.env.VITE_USE_MOCK_API === 'true' && selected.length === 1 && selected[0]) {
      openManagement(selected[0], true);
      return;
    }
    try {
      const snapshot = await integrationApi.snapshot();
      const ids = snapshot.accounts
        .filter((account) => selected.some((profile) => profile.id === account.profileId))
        .map((account) => account.id);
      deletionRequest = await integrationApi.requestDeletion(ids);
    } catch (cause) {
      showError(cause);
    }
  }
  let applicationLogin = false;
  let launchError: OperationError | null = null;
  let recoveringDesktop = false;
  let sharingAccount: IntegrationAccount | null = null;
  let closeOnSwitchSuccess = false;
  let switchPreparationId = 0;
  let preparingSwitch = false;
  async function connectApplication(profileId: string): Promise<void> {
    const profile = profiles.find((profile) => profile.id === profileId);
    if (!profile) return;
    selectedProfile = profile;
    applicationLogin = true;
    operationIntent = 'reconnect';
    reconnect = true;
    showLogin = true;
    selectedOperation = null;
    closeOnSwitchSuccess = false;
    await startAccountOperation();
  }
  async function refreshAuthorizations(): Promise<void> {
    try {
      const [requests, deletions] = await Promise.all([
        integrationApi.requests(),
        integrationApi.deletionRequests(),
      ]);
      // The open dialog owns its result. A backend publication/removal event
      // must not dismiss it in the middle of consent or refresh-error recovery.
      if (!authorizationRequest) authorizationRequest = requests[0] ?? null;
      deletionRequest = deletions[0] ?? null;
      if (authorizationRequest) await refreshIntegrations();
    } catch (cause) {
      showError(cause);
    }
  }
  async function refreshIntegrations(): Promise<void> {
    await integrationRefresh.request('dashboard');
  }
  const integrationRefresh = new RefreshCoordinator(
    loadIntegrationSnapshot,
    loadIntegrationSnapshot,
  );
  async function loadIntegrationSnapshot(): Promise<void> {
    try {
      integrations = await integrationApi.snapshot();
    } catch (cause) {
      showError(cause);
    }
  }
  function selectView(selected: View): void {
    view = selected;
    if (selected === 'dashboard' || selected === 'applications') void refreshIntegrations();
  }
  let settings: ManagerSettings = { rotationStrategy: 'before_reset' };
  let profiles: ManagedProfile[] = [];
  let dashboard: DashboardSnapshot | null = null;
  let providers: MailProvider[] = [];
  let operations: LoginOperation[] = [];
  let runtime: RuntimeStatus | null = null;
  let loading = true;
  let recoveringAccounts = false;
  let error = '';
  let notice = '';
  let showCreate = false;
  let showRename = false;
  let showLogin = false;
  let showMailSettings = false;
  let showProvider = false;
  let managementProfile: ManagedProfile | null = null;
  let checkProfileTarget: ManagedProfile | null = null;
  let managementDelete = false;
  let replacementPreview: ReplacementPreview | null = null;
  let editingProfile: ManagedProfile | null = null;
  let selectedProfile: ManagedProfile | null = null;
  let mailSettingsProfile: ManagedProfile | null = null;
  let selectedOperation: LoginOperation | null = null;
  let newName = '';
  let newProviderId = '';
  let newMailUrl = '';
  let mailSettingsUrl = '';
  let reconnect = false;
  let operationIntent: OperationKind = 'login';
  let cancellingLogin = false;
  let editingProvider: MailProvider | null = null;
  let providerDisplayName = '';
  let providerDomains = '';
  let providerUrl = '';
  let providerAliases = '';
  let availableUpdate: UpdateInfo | null = null;
  let updateProgress: UpdateProgress | null = null;
  let checkingUpdate = false;
  let installingUpdate = false;
  let unlistenOperations: (() => void) | null = null;
  let unlistenDashboard: (() => void) | null = null;
  let unlistenUpdate: (() => void) | null = null;
  let unlistenShutdown: (() => void) | null = null;
  let shutdown: ShutdownState | null = null;
  const refreshCoordinator = new RefreshCoordinator(loadDashboardSnapshot, loadFullSnapshot);

  $: visibleProfiles = dashboard?.accounts.map((card) => card.profile) ?? profiles;
  $: paletteCommands = buildPaletteCommands(visibleProfiles);
  function buildPaletteCommands(values: ManagedProfile[]): PaletteCommand[] {
    const areas: Array<{ view: View; name: string }> = [
      { view: 'dashboard', name: 'Contas' },
      { view: 'applications', name: 'Aplicações' },
      { view: 'providers', name: 'E-mail' },
      { view: 'diagnostics', name: 'Diagnóstico' },
    ];
    return [
      ...areas.map((area) => ({
        id: area.view,
        label: `Abrir ${area.name}`,
        detail: 'Navegação',
        keywords: [area.name],
        perform: () => selectView(area.view),
      })),
      {
        id: 'create-profile',
        label: 'Adicionar conta',
        detail: 'Criar um perfil próprio',
        keywords: ['novo perfil'],
        perform: openCreate,
      },
      ...values
        .filter((profile) => profile.status !== 'deleted')
        .map((profile) => ({
          id: `profile:${profile.id}`,
          label: profile.name,
          detail: `${profilePresentation(profile).actionLabel} · ${profile.email ?? 'Sem e-mail'}`,
          keywords: [profile.email ?? ''],
          perform: () => activateFromPalette(profile),
        })),
    ];
  }
  function activateFromPalette(profile: ManagedProfile): void {
    const action = profilePresentation(profile).action;
    if (action === 'switch') switchAccount(profile);
    else if (action === 'check') checkProfile(profile);
    else if (action === 'connect' || action === 'reauthenticate')
      openLogin(profile, action === 'reauthenticate');
    else if (action === 'restore') void restoreProfile(profile);
    else notice = `${profile.name} já está em uso no Codex.`;
  }

  onMount(() => {
    void initialize();
    const shortcuts = (event: KeyboardEvent): void => {
      if (
        (event.ctrlKey || event.metaKey) &&
        event.key.toLowerCase() === 'k' &&
        (!document.querySelector('[aria-modal="true"]') || showPalette)
      ) {
        event.preventDefault();
        showPalette = !showPalette;
        return;
      }
      if (
        (event.ctrlKey || event.metaKey) &&
        event.key.toLowerCase() === 'f' &&
        !document.querySelector('[aria-modal="true"]')
      ) {
        event.preventDefault();
        document
          .querySelector<HTMLInputElement>(
            'main [data-area-search], main input[aria-label^="Buscar"]',
          )
          ?.focus();
      }
    };
    document.addEventListener('keydown', shortcuts);
    const clock = window.setInterval(() => {
      if (view === 'dashboard' && document.visibilityState === 'visible')
        void requestRefresh('dashboard');
    }, 30_000);
    return () => {
      unlistenOperations?.();
      unlistenDashboard?.();
      unlistenUpdate?.();
      unlistenShutdown?.();
      window.clearInterval(clock);
      document.removeEventListener('keydown', shortcuts);
    };
  });

  async function initialize(): Promise<void> {
    [unlistenOperations, unlistenDashboard, unlistenUpdate, unlistenShutdown] = await Promise.all([
      api.subscribeToOperations(applyOperationEvent),
      api.subscribeToDashboardChanges(() => {
        void requestRefresh('dashboard');
        void refreshAuthorizations();
        if (view === 'applications' || view === 'dashboard' || showGroups)
          void refreshIntegrations();
      }),
      api.subscribeToUpdateProgress((progress) => (updateProgress = progress)),
      api.subscribeToShutdown((state) => (shutdown = state)),
    ]);
    await requestRefresh('full');
    void refreshIntegrations();
    void refreshAuthorizations();
  }

  function applyOperationEvent(update: OperationEvent): void {
    operations = operations.map((operation) =>
      operation.id === update.operationId
        ? {
            ...operation,
            kind: update.kind,
            state: update.state,
            step: update.step,
            errorCode: update.errorCode ?? null,
            lastStageDurationMs: update.stageDurationMs,
            cancelable: update.cancelable,
          }
        : operation,
    );
    if (selectedOperation?.id === update.operationId) {
      selectedOperation = {
        ...selectedOperation,
        kind: update.kind,
        state: update.state,
        step: update.step,
        errorCode: update.errorCode ?? null,
        errorMessage: update.errorCode ? update.message : null,
        lastStageDurationMs: update.stageDurationMs,
        cancelable: update.cancelable,
      };
      if (update.state === 'awaiting_confirmation') void loadReplacementPreview(update.operationId);
    }
    if (['completed', 'cancelled', 'failed', 'timed_out'].includes(update.state)) {
      cancellingLogin = false;
      void refreshAfterOperation(update);
    }
  }

  async function refreshAfterOperation(update: OperationEvent): Promise<void> {
    const refreshOpenModal = selectedOperation?.id === update.operationId;
    await requestRefresh('full');
    if (refreshOpenModal) {
      selectedOperation =
        operations.find((operation) => operation.id === update.operationId) ?? selectedOperation;
      selectedProfile =
        profiles.find((profile) => profile.id === update.profileId) ?? selectedProfile;
    }
    if (update.state === 'completed') {
      notice =
        update.kind === 'replace'
          ? 'Conta substituída após validação e confirmação.'
          : update.kind === 'switch'
            ? (showLogin ? '' : update.step)
            : applicationLogin
              ? 'Sessão própria para aplicações conectada. Seu Codex não foi trocado.'
              : 'Conta validada e perfil atualizado.';
      if (
        refreshOpenModal &&
        update.kind === 'switch' &&
        closeOnSwitchSuccess &&
        !update.errorCode
      ) {
        try {
          await api.focusCodex();
          showLogin = false;
          closeOnSwitchSuccess = false;
        } catch (cause) {
          if (selectedOperation)
            selectedOperation = {
              ...selectedOperation,
              errorCode: 'desktop_reopen_pending',
              errorMessage: operationError(cause).message,
            };
        }
      }
    }
  }

  function requestRefresh(scope: RefreshScope): Promise<void> {
    return refreshCoordinator.request(scope);
  }

  async function loadFullSnapshot(): Promise<void> {
    error = '';
    try {
      const results = await Promise.allSettled([
        loadDashboardSnapshot(),
        api.listProviders().then((value) => {
          providers = value;
        }),
        api.listOperations().then((value) => {
          operations = value;
          selectedOperation = reconcileOperation(selectedOperation, operations);
          if (selectedOperation?.state === 'awaiting_confirmation')
            void loadReplacementPreview(selectedOperation.id);
        }),
        api.runtimeStatus().then((value) => {
          runtime = value;
        }),
        api.settings().then((value) => {
          settings = value;
        }),
      ]);
      const failure = results.find((result) => result.status === 'rejected');
      if (failure?.status === 'rejected') showError(failure.reason);
    } catch (cause) {
      showError(cause);
    } finally {
      loading = false;
    }
  }

  async function loadDashboardSnapshot(): Promise<void> {
    try {
      const next = await api.dashboardSnapshot();
      if ((next.revision ?? 0) < (dashboard?.revision ?? 0)) return;
      dashboard = next;
      profiles = dashboard.accounts.map((card) => card.profile);
    } catch (cause) {
      showError(cause);
    }
  }

  function showError(cause: unknown): void {
    error = cause instanceof Error ? cause.message : String(cause);
  }

  async function saveSettings(value: ManagerSettings): Promise<void> {
    await runVoidAction(async () => {
      await api.saveSettings(value);
      settings = value;
      await requestRefresh('dashboard');
    });
  }

  async function saveOptions(profile: ManagedProfile, options: ProfileOptions): Promise<void> {
    await runVoidAction(() => api.saveProfileOptions(profile.id, options));
    await requestRefresh('dashboard');
  }

  function checkProfile(profile: ManagedProfile): void {
    checkProfileTarget = profile;
  }

  function providerName(providerId: string | null): string {
    if (!providerId) return 'Não definido';
    return providers.find((provider) => provider.id === providerId)?.displayName ?? providerId;
  }

  function openCreate(): void {
    newName = `Perfil ${profiles.length + 1}`;
    newProviderId = '';
    newMailUrl = '';
    showCreate = true;
  }

  async function createProfile(): Promise<void> {
    if (!newName.trim()) return;
    try {
      await api.createProfile(newName.trim(), newProviderId || null, newMailUrl.trim() || null);
      showCreate = false;
      notice = 'Perfil criado no diretório gerenciado do aplicativo.';
      await requestRefresh('full');
    } catch (cause) {
      showError(cause);
    }
  }

  function openRename(profile: ManagedProfile): void {
    editingProfile = profile;
    newName = profile.name;
    showRename = true;
  }

  async function renameProfile(): Promise<void> {
    if (!editingProfile || !newName.trim()) return;
    try {
      await api.renameProfile(editingProfile.id, newName.trim());
      showRename = false;
      editingProfile = null;
      await requestRefresh('full');
    } catch (cause) {
      showError(cause);
    }
  }

  async function archiveProfile(profile: ManagedProfile): Promise<void> {
    await runProfileAction(() => api.archiveProfile(profile.id));
  }

  async function restoreProfile(profile: ManagedProfile): Promise<void> {
    await runProfileAction(() => api.restoreProfile(profile.id));
  }

  async function runProfileAction(action: () => Promise<ManagedProfile>): Promise<void> {
    try {
      await action();
      await requestRefresh('full');
    } catch (cause) {
      showError(cause);
    }
  }

  function permanentlyRemove(profile: ManagedProfile): void {
    if (profile.accountId) void requestDeletion([profile]);
    else openManagement(profile, true);
  }

  function openManagement(profile: ManagedProfile, deleteOnly = false): void {
    managementProfile = profile;
    managementDelete = deleteOnly;
  }

  function openReplacement(profile: ManagedProfile): void {
    managementProfile = null;
    selectedProfile = profile;
    replacementPreview = null;
    operationIntent = 'replace';
    reconnect = false;
    applicationLogin = false;
    closeOnSwitchSuccess = false;
    selectedOperation = null;
    cancellingLogin = false;
    showLogin = true;
    void startAccountOperation();
  }

  async function loadReplacementPreview(id: string): Promise<void> {
    try {
      const candidate = await api.replacementPreview(id);
      if (selectedOperation?.id === id) replacementPreview = candidate;
    } catch (cause) {
      showError(cause);
    }
  }

  async function confirmReplacement(): Promise<void> {
    const id = selectedOperation?.id;
    if (id) await runVoidAction(() => api.confirmReplacement(id));
  }

  async function openMail(profile: ManagedProfile): Promise<void> {
    await runVoidAction(() => api.openMail(profile.id));
  }

  function openMailSettings(profile: ManagedProfile): void {
    mailSettingsProfile = profile;
    mailSettingsUrl = profile.mailUrl ?? '';
    showMailSettings = true;
  }

  async function saveMailSettings(): Promise<void> {
    await persistMailSettings(mailSettingsUrl.trim() || null);
  }

  async function clearMailSettings(): Promise<void> {
    await persistMailSettings(null);
  }

  async function persistMailSettings(mailUrl: string | null): Promise<void> {
    if (!mailSettingsProfile) return;
    try {
      await api.updateProfileMailUrl(mailSettingsProfile.id, mailUrl);
      showMailSettings = false;
      notice = mailUrl
        ? 'Endereço de webmail salvo para esta conta.'
        : 'A conta voltou a usar a sugestão do catálogo.';
      await requestRefresh('full');
    } catch (cause) {
      showError(cause);
    }
  }

  async function openProfileFolder(profile: ManagedProfile): Promise<void> {
    await runVoidAction(() => api.openProfileFolder(profile.id));
  }

  async function runVoidAction(action: () => Promise<void>): Promise<void> {
    try {
      await action();
    } catch (cause) {
      showError(cause);
    }
  }

  function switchAccount(profile: ManagedProfile): void {
    void prepareSwitch(profile, false);
  }

  async function prepareSwitch(profile: ManagedProfile, closeAfter: boolean): Promise<void> {
    const preparationId = ++switchPreparationId;
    checkProfileTarget = null;
    selectedProfile = profile;
    applicationLogin = false;
    reconnect = false;
    operationIntent = 'switch';
    selectedOperation = null;
    launchError = null;
    showLogin = true;
    preparingSwitch = true;
    closeOnSwitchSuccess = closeAfter;
    try {
      const snapshot = await integrationApi.snapshot();
      if (preparationId !== switchPreparationId) return;
      integrations = snapshot;
      const account = snapshot.accounts.find((account) => account.profileId === profile.id);
      if (account?.policy === 'applications_only') {
        showLogin = false;
        sharingAccount = account;
        return;
      }
      beginSwitch(profile);
    } catch (cause) {
      if (preparationId === switchPreparationId) launchError = operationError(cause);
    } finally {
      if (preparationId === switchPreparationId) preparingSwitch = false;
    }
  }

  function closeLogin(): void {
    if (preparingSwitch) ++switchPreparationId;
    preparingSwitch = false;
    showLogin = false;
  }

  async function confirmSharedAccount(): Promise<void> {
    if (!sharingAccount || !selectedProfile) return;
    await integrationApi.accountPolicy(sharingAccount.id, 'shared', sharingAccount.capacity);
    sharingAccount = null;
    beginSwitch(selectedProfile);
  }

  function beginSwitch(profile: ManagedProfile): void {
    selectedProfile = profile;
    applicationLogin = false;
    reconnect = false;
    operationIntent = 'switch';
    selectedOperation = null;
    cancellingLogin = false;
    showLogin = true;
    void startAccountOperation();
  }

  function openLogin(profile: ManagedProfile, shouldReconnect: boolean): void {
    applicationLogin = false;
    selectedProfile = profile;
    reconnect = shouldReconnect;
    operationIntent = shouldReconnect ? 'reconnect' : 'login';
    closeOnSwitchSuccess = false;
    selectedOperation = null;
    cancellingLogin = false;
    showLogin = true;
    void startAccountOperation();
  }

  async function startAccountOperation(): Promise<void> {
    if (!selectedProfile) return;
    launchError = null;
    selectedOperation = null;
    cancellingLogin = false;
    try {
      selectedOperation =
        operationIntent === 'replace'
          ? await api.startReplacement(selectedProfile.id)
          : operationIntent === 'switch'
            ? await api.startSwitchAccount(selectedProfile.id)
            : applicationLogin
              ? await integrationApi.applicationLogin(selectedProfile.id)
              : await api.startLogin(selectedProfile.id, operationIntent === 'reconnect', null);
      operations = [
        selectedOperation,
        ...operations.filter((item) => item.id !== selectedOperation?.id),
      ];
      // A fast local switch can finish before the first progress event reaches
      // the WebView. Re-read persisted state so the modal cannot stay on Idle.
      void requestRefresh('full');
    } catch (cause) {
      launchError = operationError(cause);
    }
  }

  async function cancelLogin(): Promise<void> {
    if (!selectedOperation) return;
    cancellingLogin = true;
    try {
      await api.cancelOperation(selectedOperation.id);
      selectedOperation = { ...selectedOperation, step: 'Cancelamento solicitado' };
    } catch (cause) {
      cancellingLogin = false;
      showError(cause);
    }
  }

  function recoverOperation(action: RecoveryAction): void {
    if (!selectedProfile) return;
    if (action === 'retry')
      operationIntent === 'switch'
        ? void prepareSwitch(selectedProfile, closeOnSwitchSuccess)
        : void startAccountOperation();
    if (action === 'reconnect')
      applicationLogin
        ? void connectApplication(selectedProfile.id)
        : openLogin(selectedProfile, !!selectedProfile.accountId);
    if (action === 'replace_account') openReplacement(selectedProfile);
    if (action === 'check_connection') {
      showLogin = false;
      checkProfile(selectedProfile);
    }
    if (action === 'diagnostics') {
      showLogin = false;
      selectView('diagnostics');
    }
    if (action === 'open_desktop') void openDesktopRecovery();
    if (action === 'select_desktop') void selectDesktopExecutable();
    if (action === 'confirm_close') void confirmDesktopInterruption();
    if (action === 'close_browser') void closeBrowserRecovery();
  }

  async function closeBrowserRecovery(): Promise<void> {
    if (!selectedOperation || recoveringDesktop) return;
    const operationId = selectedOperation.id;
    recoveringDesktop = true;
    try {
      await api.retryBrowserCleanup(operationId);
      await requestRefresh('full');
    } catch (cause) {
      if (selectedOperation?.id === operationId) selectedOperation = { ...selectedOperation, errorMessage: operationError(cause).message };
    } finally {
      recoveringDesktop = false;
    }
  }

  async function openDesktopRecovery(): Promise<void> {
    if (recoveringDesktop) return;
    recoveringDesktop = true;
    const operationId = selectedOperation?.id;
    if (selectedOperation) selectedOperation = { ...selectedOperation, step: 'Detectando e abrindo o Codex…' };
    try {
      await api.focusCodex(operationId);
      if (selectedOperation?.id === operationId && selectedOperation) selectedOperation = { ...selectedOperation, step: 'Abertura do Codex confirmada', errorCode: null, errorMessage: null };
      launchError = null;
      await requestRefresh('dashboard');
    } catch (cause) {
      if (selectedOperation?.id === operationId && selectedOperation) selectedOperation = { ...selectedOperation, step: 'A abertura do Codex não foi confirmada', errorCode: operationError(cause).code, errorMessage: operationError(cause).message };
      else launchError = operationError(cause);
    } finally {
      recoveringDesktop = false;
    }
  }

  async function confirmDesktopInterruption(): Promise<void> {
    if (!selectedOperation || !selectedProfile) return;
    if (recoveringDesktop) return;
    recoveringDesktop = true;
    const operationId = selectedOperation.id;
    const profile = selectedProfile;
    try {
      if (await api.confirmCodexClose(operationId)) {
        if (selectedOperation?.id === operationId) await prepareSwitch(profile, closeOnSwitchSuccess);
      }
    } catch (cause) {
      if (selectedOperation?.id === operationId) selectedOperation = { ...selectedOperation, errorCode: operationError(cause).code, errorMessage: operationError(cause).message };
    } finally { recoveringDesktop = false; }
  }

  async function selectDesktopExecutable(): Promise<void> {
    if (recoveringDesktop) return;
    recoveringDesktop = true;
    try {
      if (await api.selectCodexDesktop()) notice = 'Executável selecionado. Detecte e abra o Codex ou tente a troca novamente.';
    } catch (cause) {
      if (selectedOperation) selectedOperation = { ...selectedOperation, errorCode: operationError(cause).code, errorMessage: operationError(cause).message };
      else launchError = operationError(cause);
    } finally { recoveringDesktop = false; }
  }

  async function recoverSavedAccounts(): Promise<void> {
    recoveringAccounts = true;
    error = '';
    try {
      const count = await api.recoverSavedAccounts();
      notice =
        count > 0
          ? `${count} ${count === 1 ? 'conta recuperada' : 'contas recuperadas'} do armazenamento anterior. Verifique a sessão pelo card antes de usar.`
          : 'Nenhuma conta adicional encontrada. As contas já cadastradas foram preservadas.';
      await requestRefresh('full');
    } catch (cause) {
      showError(cause);
    } finally {
      recoveringAccounts = false;
    }
  }

  async function exportDiagnostics(): Promise<void> {
    try {
      const fileName = await api.exportDiagnostics();
      notice = `Diagnóstico sanitizado exportado: ${fileName}`;
    } catch (cause) {
      showError(cause);
    }
  }

  async function checkForUpdate(): Promise<void> {
    checkingUpdate = true;
    error = '';
    try {
      availableUpdate = await api.checkForUpdate();
      notice = availableUpdate
        ? `Atualização ${availableUpdate.version} disponível e pronta para verificação criptográfica.`
        : 'Você já está usando a versão mais recente.';
    } catch (cause) {
      showError(cause);
    } finally {
      checkingUpdate = false;
    }
  }

  async function installUpdate(): Promise<void> {
    if (!availableUpdate) return;
    installingUpdate = true;
    error = '';
    updateProgress = { downloadedBytes: 0, totalBytes: null };
    try {
      await api.installUpdate(availableUpdate.version);
      notice = 'Atualização verificada e instalada. Reinicie o aplicativo para concluir.';
    } catch (cause) {
      showError(cause);
    } finally {
      installingUpdate = false;
    }
  }

  function openProvider(provider: MailProvider | null): void {
    editingProvider = provider;
    providerDisplayName = provider?.displayName ?? '';
    providerDomains = provider?.domains.join(', ') ?? '';
    providerUrl = provider?.defaultWebmailUrl ?? '';
    providerAliases = provider?.aliases.join(', ') ?? '';
    showProvider = true;
  }

  async function saveProvider(): Promise<void> {
    try {
      await api.saveProvider({
        id: editingProvider?.id ?? null,
        displayName: providerDisplayName,
        domains: commaSeparatedValues(providerDomains),
        defaultWebmailUrl: providerUrl,
        aliases: commaSeparatedValues(providerAliases),
      });
      showProvider = false;
      notice = editingProvider ? 'Provedor atualizado.' : 'Provedor personalizado adicionado.';
      await requestRefresh('full');
    } catch (cause) {
      showError(cause);
    }
  }

  function commaSeparatedValues(value: string): string[] {
    return value
      .split(',')
      .map((item) => item.trim())
      .filter(Boolean);
  }
</script>

<svelte:window oncontextmenu={(event) => event.preventDefault()} />

<div class="shell">
  <Sidebar {view} {runtime} {selectView} />
  <main class="content">
    <TopBar {view} createProfile={openCreate} />
    {#if shutdown}<div class="notice" role="status">{shutdown.message}</div>{/if}

    {#if error}<div class="notice error" role="alert">{error}</div>{/if}
    {#if notice}<div class="notice">{notice}</div>{/if}
    {#if loading}
      <div class="empty">Carregando o estado local…</div>
    {:else if view === 'dashboard'}
      <ProfilesView
        accounts={dashboard?.accounts ?? []}
        now={dashboard ? Date.parse(dashboard.generatedAt) : Date.now()}
        counts={dashboard?.counts ?? { connected: 0, attention: 0, archived: 0 }}
        {providerName}
        connect={openLogin}
        {switchAccount}
        {openMail}
        configureMail={openMailSettings}
        rename={openRename}
        openFolder={openProfileFolder}
        archive={archiveProfile}
        restore={restoreProfile}
        {permanentlyRemove}
        requestBatchDeletion={(profiles) => void requestDeletion(profiles)}
        {settings}
        {saveSettings}
        {saveOptions}
        {integrations}
        manageGroups={() => {
          showGroups = true;
          void refreshIntegrations();
        }}
        check={checkProfile}
        openBrowser={(profile) => void runVoidAction(() => api.openProfileBrowser(profile.id))}
        manage={openManagement}
      />
    {:else if view === 'providers'}
      <ProviderCatalog {providers} edit={openProvider} />
    {:else if view === 'applications'}
      <ApplicationsView
        focusApplicationId={preparedApplicationId}
        snapshot={integrations}
        changed={refreshIntegrations}
        connectSession={connectApplication}
        manageGroups={() => {
          showGroups = true;
          void refreshIntegrations();
        }}
      />
    {:else}
      <DiagnosticsPanel
        {operations}
        {profiles}
        {exportDiagnostics}
        {recoverSavedAccounts}
        {recoveringAccounts}
        {availableUpdate}
        {updateProgress}
        {checkingUpdate}
        {installingUpdate}
        loadOperationStages={api.operationStages}
        {checkForUpdate}
        {installUpdate}
        inspectRuntime={api.inspectSharedRuntime}
        recoverChat={api.recoverOriginalChat}
        openChat={api.openOriginalChat}
      />
    {/if}
  </main>
</div>

{#if showGroups}<GroupsView
    snapshot={integrations}
    changed={refreshIntegrations}
    close={() => (showGroups = false)}
  />{/if}

{#if showPalette}<CommandPalette
    commands={paletteCommands}
    close={() => (showPalette = false)}
  />{/if}

{#if showCreate}
  <ProfileFormModal
    name={newName}
    providerId={newProviderId}
    mailUrl={newMailUrl}
    {providers}
    updateName={(value) => (newName = value)}
    updateProviderId={(value) => (newProviderId = value)}
    updateMailUrl={(value) => (newMailUrl = value)}
    submit={createProfile}
    close={() => (showCreate = false)}
  />
{/if}

{#if showProvider}
  <ProviderModal
    provider={editingProvider}
    displayName={providerDisplayName}
    domains={providerDomains}
    url={providerUrl}
    aliases={providerAliases}
    updateDisplayName={(value) => (providerDisplayName = value)}
    updateDomains={(value) => (providerDomains = value)}
    updateUrl={(value) => (providerUrl = value)}
    updateAliases={(value) => (providerAliases = value)}
    submit={saveProvider}
    close={() => (showProvider = false)}
  />
{/if}

{#if showRename && editingProfile}
  <RenameModal
    name={newName}
    updateName={(value) => (newName = value)}
    submit={renameProfile}
    close={() => (showRename = false)}
  />
{/if}

{#if showMailSettings && mailSettingsProfile}
  <MailSettingsModal
    profileName={mailSettingsProfile.name}
    providerName={providerName(mailSettingsProfile.mailProviderId)}
    url={mailSettingsUrl}
    updateUrl={(value) => (mailSettingsUrl = value)}
    save={saveMailSettings}
    clear={clearMailSettings}
    close={() => (showMailSettings = false)}
  />
{/if}

{#if showLogin && selectedProfile}
  <LoginModal
    profile={selectedProfile}
    {reconnect}
    intent={operationIntent}
    operation={selectedOperation}
    cancelling={cancellingLogin}
    {providers}
    applicationSession={applicationLogin}
    {launchError}
    {recoveringDesktop}
    {preparingSwitch}
    recover={recoverOperation}
    useNow={() => selectedProfile && void prepareSwitch(selectedProfile, true)}
    savePreference={(enabled, id) =>
      selectedProfile &&
      void runVoidAction(async () => {
        await integrationApi.mailPreference(selectedProfile!.id, enabled, id);
        await requestRefresh('dashboard');
      })}
    cancel={cancelLogin}
    close={closeLogin}
    preview={replacementPreview}
    {confirmReplacement}
  />
{/if}

{#if sharingAccount}<SharedAccountModal
    account={sharingAccount}
    confirm={confirmSharedAccount}
    close={() => {
      sharingAccount = null;
      closeOnSwitchSuccess = false;
    }}
  />{/if}

{#if managementProfile}<AccountManagementModal
    profile={managementProfile}
    integrationAccount={integrations.accounts.find(account => account.profileId === managementProfile?.id) ?? null}
    profiles={visibleProfiles}
    initiallyDelete={managementDelete}
    replace={() => managementProfile && openReplacement(managementProfile)}
    {switchAccount}
    connectSession={connectApplication}
    changed={async () => { await requestRefresh('full'); await refreshIntegrations(); }}
    close={() => (managementProfile = null)}
  />{/if}
{#if checkProfileTarget}<ConnectionCheckModal
    profile={checkProfileTarget}
    activate={switchAccount}
    changed={() => requestRefresh('dashboard')}
    close={() => (checkProfileTarget = null)}
  />{/if}
{#if authorizationRequest}<AuthorizationModal
    request={authorizationRequest}
    snapshot={integrations}
    prepared={id => void applicationPrepared(id)}
    closed={() => {
      authorizationRequest = null;
      void refreshAuthorizations();
    }}
  />{/if}
{#if deletionRequest && !authorizationRequest}<DeletionModal
    request={deletionRequest}
    closed={() => {
      deletionRequest = null;
      void requestRefresh('full');
      void refreshAuthorizations();
    }}
  />{/if}
