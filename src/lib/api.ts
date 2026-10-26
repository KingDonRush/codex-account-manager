import { invoke as nativeInvoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { createMockApi } from './mock-api';
import type {
  LoginOperation,
  DashboardSnapshot,
  MailProvider,
  ManagedProfile,
  ManagerSettings,
  ProfileOptions,
  ReplacementPreview,
  ConnectionCheck,
  OperationEvent,
  OperationStage,
  RuntimeStatus,
  SaveMailProviderInput,
  UpdateInfo,
  UpdateProgress,
  SharedRuntimeSnapshot,
  RuntimeRecoveryReport,
  RuntimeConsent,
  ShutdownState,
} from './types';

export class NativeCommandError extends Error {
  constructor(
    message: string,
    readonly code: string,
    readonly rpcCode: number | null,
  ) {
    super(message);
  }
}

export async function invoke<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  try {
    return await nativeInvoke<T>(command, args);
  } catch (cause) {
    if (
      typeof cause === 'object' &&
      cause !== null &&
      'message' in cause &&
      typeof cause.message === 'string'
    ) {
      const code = 'code' in cause && typeof cause.code === 'string' ? cause.code : 'unknown';
      const rpcCode =
        'rpcCode' in cause && typeof cause.rpcCode === 'number' ? cause.rpcCode : null;
      throw new NativeCommandError(cause.message, code, rpcCode);
    }
    throw cause instanceof Error ? cause : new Error(String(cause));
  }
}

export interface AccountManagerApi {
  retryBrowserCleanup(operationId: string): Promise<void>;
  recoverSavedAccounts(): Promise<number>;
  focusCodex(operationId?: string): Promise<void>;
  selectCodexDesktop(): Promise<boolean>;
  confirmCodexClose(operationId: string): Promise<boolean>;
  inspectSharedRuntime(): Promise<SharedRuntimeSnapshot>;
  recoverOriginalChat(
    thread: string,
    consent: RuntimeConsent | null,
  ): Promise<RuntimeRecoveryReport>;
  openOriginalChat(thread: string): Promise<void>;
  subscribeToShutdown(handler: (event: ShutdownState) => void): Promise<() => void>;
  unlinkProfile(profileId: string): Promise<void>;
  logoutProfile(profileId: string): Promise<void>;
  startReplacement(profileId: string): Promise<LoginOperation>;
  replacementPreview(operationId: string): Promise<ReplacementPreview | null>;
  confirmReplacement(operationId: string): Promise<void>;
  settings(): Promise<ManagerSettings>;
  saveSettings(settings: ManagerSettings): Promise<void>;
  saveProfileOptions(profileId: string, options: ProfileOptions): Promise<void>;
  openProfileBrowser(profileId: string): Promise<void>;
  checkProfile(profileId: string): Promise<ConnectionCheck>;
  listProfiles(): Promise<ManagedProfile[]>;
  dashboardSnapshot(): Promise<DashboardSnapshot>;
  listProviders(): Promise<MailProvider[]>;
  saveProvider(input: SaveMailProviderInput): Promise<MailProvider>;
  listOperations(): Promise<LoginOperation[]>;
  operationStages(operationId: string): Promise<OperationStage[]>;
  runtimeStatus(): Promise<RuntimeStatus>;
  createProfile(
    name: string,
    providerId: string | null,
    mailUrl: string | null,
  ): Promise<ManagedProfile>;
  renameProfile(profileId: string, name: string): Promise<ManagedProfile>;
  updateProfileMailUrl(profileId: string, mailUrl: string | null): Promise<ManagedProfile>;
  archiveProfile(profileId: string): Promise<ManagedProfile>;
  restoreProfile(profileId: string): Promise<ManagedProfile>;
  deleteProfile(profileId: string, permanently: boolean): Promise<void>;
  openMail(profileId: string): Promise<void>;
  openProfileFolder(profileId: string): Promise<void>;
  startLogin(
    profileId: string,
    reconnect: boolean,
    mailUrl: string | null,
  ): Promise<LoginOperation>;
  startSwitchAccount(profileId: string): Promise<LoginOperation>;
  cancelOperation(operationId: string): Promise<void>;
  exportDiagnostics(): Promise<string>;
  checkForUpdate(): Promise<UpdateInfo | null>;
  installUpdate(expectedVersion: string): Promise<void>;
  subscribeToOperations(handler: (event: OperationEvent) => void): Promise<() => void>;
  subscribeToDashboardChanges(handler: () => void): Promise<() => void>;
  subscribeToUpdateProgress(handler: (event: UpdateProgress) => void): Promise<() => void>;
}

const tauriApi: AccountManagerApi = {
  retryBrowserCleanup: (operationId) => invoke<void>('retry_browser_cleanup', { operationId }),
  recoverSavedAccounts: () => invoke<number>('recover_saved_accounts'),
  focusCodex: (operationId) => invoke<void>('focus_codex', { operationId: operationId ?? null }),
  selectCodexDesktop: () => invoke<boolean>('select_codex_desktop'),
  confirmCodexClose: (operationId) => invoke<boolean>('confirm_codex_close', { operationId }),
  inspectSharedRuntime: () => invoke<SharedRuntimeSnapshot>('inspect_shared_runtime'),
  recoverOriginalChat: (thread, consent) =>
    invoke<RuntimeRecoveryReport>('recover_original_chat', { thread, consent }),
  openOriginalChat: (thread) => invoke<void>('open_original_chat', { thread }),
  subscribeToShutdown: async (handler) =>
    listen<ShutdownState>('shutdown-state', ({ payload }) => handler(payload)),
  unlinkProfile: (profileId) => invoke<void>('unlink_profile', { profileId }),
  logoutProfile: (profileId) => invoke<void>('logout_profile', { profileId }),
  startReplacement: (profileId) => invoke<LoginOperation>('start_replacement', { profileId }),
  replacementPreview: (operationId) =>
    invoke<ReplacementPreview | null>('get_replacement_preview', { operationId }),
  confirmReplacement: (operationId) => invoke<void>('confirm_replacement', { operationId }),
  settings: () => invoke<ManagerSettings>('get_manager_settings'),
  saveSettings: (settings) => invoke<void>('save_manager_settings', { settings }),
  saveProfileOptions: (profileId, options) =>
    invoke<void>('save_profile_options', { profileId, options }),
  openProfileBrowser: (profileId) => invoke<void>('open_profile_browser', { profileId }),
  checkProfile: (profileId) => invoke<ConnectionCheck>('check_profile', { profileId }),
  listProfiles: () => invoke<ManagedProfile[]>('list_profiles'),
  dashboardSnapshot: () => invoke<DashboardSnapshot>('get_dashboard_snapshot'),
  listProviders: () => invoke<MailProvider[]>('list_mail_providers'),
  saveProvider: (input) => invoke<MailProvider>('save_mail_provider', { input }),
  listOperations: () => invoke<LoginOperation[]>('list_operations'),
  operationStages: (operationId) =>
    invoke<OperationStage[]>('get_operation_stages', { operationId }),
  runtimeStatus: () => invoke<RuntimeStatus>('get_runtime_status'),
  createProfile: (name, providerId, mailUrl) =>
    invoke<ManagedProfile>('create_profile', { name, providerId, mailUrl }),
  renameProfile: (profileId, name) => invoke<ManagedProfile>('rename_profile', { profileId, name }),
  updateProfileMailUrl: (profileId, mailUrl) =>
    invoke<ManagedProfile>('update_profile_mail_url', { profileId, mailUrl }),
  archiveProfile: (profileId) => invoke<ManagedProfile>('archive_profile', { profileId }),
  restoreProfile: (profileId) => invoke<ManagedProfile>('restore_profile', { profileId }),
  deleteProfile: (profileId, permanently) =>
    invoke<void>('delete_profile', { profileId, permanently }),
  openMail: (profileId) => invoke<void>('open_mail', { profileId }),
  openProfileFolder: (profileId) => invoke<void>('open_profile_folder', { profileId }),
  startLogin: (profileId, reconnect, mailUrl) =>
    invoke<LoginOperation>('start_login', { profileId, reconnect, mailUrl }),
  startSwitchAccount: (profileId) => invoke<LoginOperation>('start_switch_account', { profileId }),
  cancelOperation: (operationId) => invoke<void>('cancel_operation', { operationId }),
  exportDiagnostics: () => invoke<string>('export_diagnostics'),
  checkForUpdate: () => invoke<UpdateInfo | null>('check_for_update'),
  installUpdate: (expectedVersion) => invoke<void>('install_update', { expectedVersion }),
  subscribeToOperations: async (handler) =>
    listen<OperationEvent>('operation-progress', ({ payload }) => handler(payload)),
  subscribeToDashboardChanges: async (handler) => listen('dashboard-changed', () => handler()),
  subscribeToUpdateProgress: async (handler) =>
    listen<UpdateProgress>('update-progress', ({ payload }) => handler(payload)),
};

export const api: AccountManagerApi =
  import.meta.env.VITE_USE_MOCK_API === 'true' ? createMockApi() : tauriApi;
