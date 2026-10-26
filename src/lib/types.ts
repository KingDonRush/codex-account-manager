export type ProfileStatus = 'active' | 'archived' | 'deleted';
export type CredentialState = 'not_connected' | 'ready' | 'missing' | 'unavailable' | 'quarantined';
export type AccountHealth = 'connected' | 'attention' | 'archived';
export type AccountFilter = AccountHealth;
export type RotationStrategy = 'before_reset' | 'largest_balance';
export interface ManagerSettings {
  rotationStrategy: RotationStrategy;
}
export interface ProfileOptions {
  rotationEnabled: boolean;
  preferred: boolean;
}

export type View = 'dashboard' | 'applications' | 'providers' | 'diagnostics';

export type OperationState =
  | 'idle'
  | 'preparing'
  | 'browser_launching'
  | 'awaiting_login'
  | 'awaiting_confirmation'
  | 'verifying'
  | 'persisting'
  | 'completed'
  | 'cancelled'
  | 'failed'
  | 'timed_out';
export type OperationKind = 'login' | 'reconnect' | 'switch' | 'replace';
export interface ReplacementPreview {
  previousEmail: string | null;
  nextEmail: string | null;
  nextPlan: string | null;
}
export interface ConnectionCheck {
  confirmed: boolean;
  canRenew: boolean;
  title: string;
  guidance: string;
  errorCode: string | null;
  checkedAt: string;
}

export interface ManagedProfile {
  id: string;
  name: string;
  accountId: string | null;
  email: string | null;
  mailProviderId: string | null;
  mailUrl: string | null;
  openMailWithLogin?: boolean;
  status: ProfileStatus;
  lastUsedAt: string | null;
  lastActivatedAt: string | null;
  lastDisconnectedAt: string | null;
  lastBrowserOpenedAt: string | null;
  lastBrowserAuthenticatedAt: string | null;
  lastError: string | null;
  planType: string | null;
  rateLimits: RateLimits | null;
  usageUpdatedAt: string | null;
  isActiveAccount: boolean;
  needsActivation: boolean;
  credentialState: CredentialState;
  rotationEnabled: boolean;
  preferred: boolean;
  createdAt: string;
  updatedAt: string;
}

export interface Recommendation {
  label: string;
  effectiveRemaining: number;
  projectedReset: boolean;
  reason: string;
  nextResetAt: string | null;
}

export interface QuotaPresentation {
  remaining: number | null;
  projected: boolean;
  windowDurationMins: number | null;
  resetsAt: number | null;
}

export interface AccountCard {
  profile: ManagedProfile;
  health: AccountHealth;
  attentionReason: string | null;
  recommendation: Recommendation | null;
  effectiveRemaining: number | null;
  projectedReset: boolean;
  primaryQuota: QuotaPresentation | null;
  secondaryQuota: QuotaPresentation | null;
  nextResetAt: string | null;
  availableAt: string | null;
}

export interface DashboardCounts {
  connected: number;
  attention: number;
  archived: number;
}

export interface DashboardSnapshot {
  revision?: number;
  accounts: AccountCard[];
  counts: DashboardCounts;
  generatedAt: string;
}

export interface UsageWindow {
  usedPercent: number | null;
  windowDurationMins: number | null;
  resetsAt: number | null;
}

export interface RateLimits {
  buckets?: Record<string, RateLimitBucket>;
  credits?: CreditBalance | null;
  primary: UsageWindow | null;
  secondary: UsageWindow | null;
  rateLimitReachedType: string | null;
  resetCredits: ResetCredits | null;
}

export interface CreditBalance {
  hasCredits: boolean | null;
  unlimited: boolean | null;
  balance: string | null;
}
export interface RateLimitBucket {
  limitId: string | null;
  limitName: string | null;
  planType: string | null;
  primary: UsageWindow | null;
  secondary: UsageWindow | null;
  credits: CreditBalance | null;
  rateLimitReachedType: string | null;
}

export interface ResetCredits {
  availableCount: number | null;
}

export interface MailProvider {
  id: string;
  displayName: string;
  domains: string[];
  defaultWebmailUrl: string;
  aliases: string[];
  isCustom: boolean;
}

export interface SaveMailProviderInput {
  id: string | null;
  displayName: string;
  domains: string[];
  defaultWebmailUrl: string;
  aliases: string[];
}

export interface LoginOperation {
  id: string;
  profileId: string;
  kind: OperationKind;
  reconnect: boolean;
  state: OperationState;
  step: string;
  startedAt: string;
  stageStartedAt: string;
  lastStageDurationMs: number | null;
  finishedAt: string | null;
  errorCode: string | null;
  errorMessage: string | null;
  browserPid: number | null;
  runtimeKind: string | null;
  durationMs: number | null;
  cancelable: boolean;
}

export interface RuntimeStatus {
  runtimeKind: 'managed' | 'system';
  version: string | null;
  installed: boolean;
  repairRequired: boolean;
  downloadSizeBytes: number | null;
}

export interface OperationEvent {
  operationId: string;
  profileId: string;
  kind: OperationKind;
  state: OperationState;
  step: string;
  message: string;
  errorCode?: string;
  stageDurationMs: number | null;
  cancelable: boolean;
}

export interface OperationStage {
  name: string;
  durationMs: number;
}

export interface UpdateInfo {
  currentVersion: string;
  version: string;
  body: string | null;
  publishedAt: string | null;
}

export interface UpdateProgress {
  downloadedBytes: number;
  totalBytes: number | null;
}

export interface RuntimeThread {
  id: string;
  name: string | null;
  state: string;
  activeGoal: boolean;
  backgroundTerminals: number;
}
export interface SharedRuntimeSnapshot {
  state: 'absent' | 'idle' | 'busy' | 'unavailable';
  pid: number | null;
  threads: RuntimeThread[];
  externalCalls: number[];
  externalInspectionSupported: boolean;
  explanation: string;
}
export interface RuntimeRecoveryReport {
  recovered: boolean;
  needsConfirmation: boolean;
  explanation: string;
  affectedThreads: number;
  confirmation: RuntimeConsent | null;
}
export interface RuntimeConsent {
  pid: number;
  startedTicks: number;
  threadIds: string[];
}
export interface ShutdownState {
  state: 'waiting' | 'blocked';
  message: string;
}
