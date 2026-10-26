import { invoke } from './api';
import { createMockIntegrationApi } from './mock-integrations';
export type AccountPolicy = 'desktop_only' | 'applications_only' | 'shared';
export interface IntegrationAccount {
  accountCredentialState?: 'ready' | 'missing' | 'unavailable' | 'quarantined' | 'not_connected';
  applicationSessionRecoveryPending?: boolean;
  usage?: { observedAt: string | null; rateLimits: import('./types').RateLimits | null };
  id: string;
  profileId: string;
  name: string;
  email: string | null;
  plan: string | null;
  policy: AccountPolicy;
  capacity: number;
  /** Historical wire name: true means a session ID is registered, not verified remote access. */
  applicationSessionConnected: boolean;
}
export interface ExecutionStatus {
  id: string;
  applicationId: string;
  bindingId: string;
  accountId: string | null;
  state: string;
  startedAt: string;
  finishedAt: string | null;
  errorCode: string | null;
  background: boolean;
}
export interface ApplicationServerStatus {
  id: string;
  applicationId: string;
  bindingId: string;
  bindingRevision: number;
  accountId: string;
  state: string;
  persistHistory: boolean;
  background: boolean;
  errorCode: string | null;
}
export interface AccountGroup {
  id: string;
  name: string;
  accountIds: string[];
  archived: boolean;
  paused?: boolean;
  revision: number;
  exclusiveApplicationId: string | null;
}
export interface Application {
  id: string;
  name: string;
  slug: string;
  paused: boolean;
  revision: number;
}
export interface BindingTarget {
  kind: 'account' | 'group';
  id: string;
}
export interface ExecutionBinding {
  id: string;
  applicationId: string;
  name: string;
  target: BindingTarget;
  automatic: boolean;
  allowProjectedQuota: boolean;
  allowedModels: string[];
  defaultModel: string | null;
  paused: boolean;
  revision: number;
}
export type Permission =
  | 'read_metadata'
  | 'execute'
  | 'manage_groups'
  | 'manage_accounts'
  | 'activate_desktop'
  | 'manage_bindings'
  | 'open_browser'
  | 'request_deletion';
export interface GrantScope {
  accountIds: string[];
  directAccountIds?: string[] | null;
  groupIds: string[];
  permissions: Permission[];
  globalIncludingFuture: boolean;
  backgroundGroupIds: string[];
  expiresAt: string | null;
  permanent: boolean;
}
export interface AuthorizationSetup {
  sharedAccountIds: string[];
}
export interface AuthorizationGrant {
  id: string;
  applicationId: string;
  scope: GrantScope;
  revoked: boolean;
  createdAt: string;
}
export interface IntegrationSnapshot {
  accounts: IntegrationAccount[];
  groups: AccountGroup[];
  applications: Application[];
  bindings: ExecutionBinding[];
  grants: AuthorizationGrant[];
}
export interface AuthorizationRequest {
  id: string;
  request: { applicationId: string | null; name: string; slug: string; requestedScope: GrantScope };
  peerPid: number;
  executable: string;
  confirmationCode: string;
}
export interface DeletionItem {
  accountId: string;
  profileId: string;
  name: string;
  email: string | null;
  revision: string;
}
export interface DeletionRequest {
  id: string;
  requester: string;
  grantId: string | null;
  items: DeletionItem[];
  state: string;
  deletedIds: string[];
  errorCode: string | null;
}
export const nativeIntegrationApi = {
  clearHistory: (id: string) => invoke<void>('clear_application_history', { id }),
  exportHistory: (id: string) => invoke<string>('export_application_history', { id }),
  recoverCredential: (profileId: string) =>
    invoke<void>('recover_application_credential', { profileId }),
  deletionRequests: () => invoke<DeletionRequest[]>('list_deletion_requests'),
  requestDeletion: (accountIds: string[]) =>
    invoke<DeletionRequest>('request_account_deletion', { accountIds }),
  confirmDeletion: (id: string, selected: string[], removeBrowser: boolean) =>
    invoke<DeletionRequest>('confirm_deletion', { id, selected, removeBrowser }),
  denyDeletion: (id: string) => invoke<void>('deny_deletion', { id }),
  applicationServers: () => invoke<ApplicationServerStatus[]>('list_application_servers'),
  closeApplicationServer: (id: string) => invoke<void>('close_application_server', { id }),
  executions: () => invoke<ExecutionStatus[]>('list_executions'),
  cancelExecution: (id: string) => invoke<void>('cancel_execution', { id }),
  applicationLogin: (profileId: string) =>
    invoke<import('./types').LoginOperation>('start_application_login', { profileId }),
  requests: () => invoke<AuthorizationRequest[]>('list_authorization_requests'),
  approve: (id: string, scope: GrantScope, setup?: AuthorizationSetup) =>
    invoke<string>('approve_authorization', { id, scope, setup: setup ?? null }),
  deny: (id: string) => invoke<void>('deny_authorization', { id }),
  revoke: (id: string) => invoke<void>('revoke_authorization', { id }),
  snapshot: () => invoke<IntegrationSnapshot>('get_integrations'),
  saveGroup: (group: AccountGroup) => invoke<AccountGroup>('save_account_group', { group }),
  deleteGroup: (id: string, revision: number) =>
    invoke<void>('delete_account_group', { id, revision }),
  saveApplication: (application: Application) =>
    invoke<Application>('save_application', { application }),
  saveBinding: (binding: ExecutionBinding) =>
    invoke<ExecutionBinding>('save_execution_binding', { binding }),
  accountPolicy: (id: string, policy: AccountPolicy, capacity: number) =>
    invoke<void>('set_account_policy', { id, policy, capacity }),
  mailPreference: (profileId: string, enabled: boolean, providerId: string | null) =>
    invoke<void>('save_mail_preference', { profileId, enabled, providerId }),
};
export const integrationApi =
  import.meta.env.VITE_USE_MOCK_API === 'true' ? createMockIntegrationApi() : nativeIntegrationApi;
