import type {
  AccountGroup,
  AuthorizationGrant,
  GrantScope,
  IntegrationAccount,
  IntegrationSnapshot,
  ExecutionStatus,
} from './integrations';

const executionLabels: Readonly<Record<string, string>> = {
  queued: 'Aguardando conta livre',
  preparing: 'Preparando',
  running: 'Em execução',
  cancelling: 'Cancelando',
  completed: 'Concluído',
  failed: 'Falhou',
  cancelled: 'Cancelado',
  interrupted: 'Interrompido',
};

/** Execution history and current preservation status have different sources. */
export function executionOutcomeLabel(
  job: Pick<ExecutionStatus, 'state' | 'errorCode'>,
  account: IntegrationAccount | undefined,
): string {
  const label = executionLabels[job.state] ?? 'Estado não reconhecido';
  if (job.errorCode !== 'credential_recovery_pending') return label;
  return `${label} · ${
    account?.applicationSessionRecoveryPending === true
      ? 'gravação no cofre pendente'
      : 'houve pendência de sessão'
  }`;
}

export function effectiveScopeAccounts(scope: GrantScope, groups: AccountGroup[]): Set<string> {
  const ids = new Set(scope.directAccountIds ?? scope.accountIds);
  for (const group of groups) {
    if (scope.groupIds.includes(group.id)) group.accountIds.forEach((id) => ids.add(id));
  }
  return ids;
}

export function activeApplicationGrants(
  snapshot: IntegrationSnapshot,
  applicationId: string,
  now = Date.now(),
): AuthorizationGrant[] {
  return snapshot.grants.filter(
    (grant) =>
      grant.applicationId === applicationId &&
      !grant.revoked &&
      (!grant.scope.expiresAt || Date.parse(grant.scope.expiresAt) > now),
  );
}

export function applicationAccounts(
  snapshot: IntegrationSnapshot,
  applicationId: string,
): IntegrationAccount[] {
  const grants = activeApplicationGrants(snapshot, applicationId);
  return snapshot.accounts.filter((account) =>
    grants.some((grant) => includesAccount(snapshot, grant, account.id)),
  );
}

export function executableApplicationAccounts(
  snapshot: IntegrationSnapshot,
  applicationId: string,
): string[] {
  const grants = activeApplicationGrants(snapshot, applicationId).filter((grant) =>
    grant.scope.permissions.includes('execute'),
  );
  return snapshot.accounts
    .filter((account) => grants.some((grant) => includesAccount(snapshot, grant, account.id)))
    .map((account) => account.id);
}

function includesAccount(
  snapshot: IntegrationSnapshot,
  grant: AuthorizationGrant,
  id: string,
): boolean {
  if (grant.scope.globalIncludingFuture) return true;
  if ((grant.scope.directAccountIds ?? grant.scope.accountIds).includes(id)) return true;
  return (
    grant.scope.accountIds.includes(id) &&
    snapshot.groups.some(
      (group) =>
        grant.scope.groupIds.includes(group.id) &&
        !group.archived &&
        !group.paused &&
        (!group.exclusiveApplicationId || group.exclusiveApplicationId === grant.applicationId) &&
        group.accountIds.includes(id),
    )
  );
}

type SessionKind = 'saved' | 'login' | 'recovery' | 'unverified';
export interface ApplicationSessionPresentation {
  kind: SessionKind;
  label: string;
  guidance: string;
  actionLabel: string | null;
}

const sessionPresentations: Readonly<
  Record<SessionKind, Readonly<ApplicationSessionPresentation>>
> = {
  saved: {
    kind: 'saved',
    label: 'Sessão registrada',
    actionLabel: 'Reconectar sessão de app',
    guidance:
      'Há um cadastro de login para aplicações. A autenticação será validada ao executar; seu Codex não é alterado.',
  },
  login: {
    kind: 'login',
    label: 'Login necessário',
    actionLabel: 'Reconectar conta',
    guidance:
      'A sessão salva desta conta não está disponível. Reconecte a conta; não é necessário um login separado por aplicação.',
  },
  recovery: {
    kind: 'recovery',
    label: 'Gravação pendente',
    actionLabel: 'Concluir gravação no cofre',
    guidance:
      'Gravação protegida pendente no cofre. Conclua a gravação antes de outro login ou trabalho.',
  },
  unverified: {
    kind: 'unverified',
    label: 'Sessão não verificada',
    actionLabel: null,
    guidance:
      'O estado da sessão não foi informado. O cadastro existente não foi descartado nem considerado inválido.',
  },
};

/** Registration is local metadata, not proof of a readable credential or live authentication. */
export function applicationSessionPresentation(
  account: IntegrationAccount,
): Readonly<ApplicationSessionPresentation> {
  if (account.applicationSessionRecoveryPending === true) return sessionPresentations.recovery;
  if (account.applicationSessionConnected === true) return sessionPresentations.saved;
  if (account.accountCredentialState === 'ready')
    return {
      kind: 'saved',
      label: 'Usa a conta conectada',
      actionLabel: null,
      guidance:
        'Usa a sessão já salva. A autenticação é validada na execução, sem trocar seu Codex nem exigir outro login.',
    };
  if (
    account.accountCredentialState === 'unavailable' ||
    account.accountCredentialState === 'quarantined'
  )
    return {
      ...sessionPresentations.unverified,
      label: 'Sessão requer verificação',
      guidance:
        'A sessão foi preservada, mas o acesso ao cofre ou à geração precisa ser verificado. Isso não confirma necessidade de novo login.',
    };
  if (account.applicationSessionConnected === false) return sessionPresentations.login;
  return sessionPresentations.unverified;
}

export function applicationReadiness(
  account: IntegrationAccount,
): Readonly<
  | ApplicationSessionPresentation
  | { label: string; kind: 'policy'; guidance: string; actionLabel: string }
> {
  if (account.policy === 'desktop_only')
    return {
      label: 'Uso em aplicações desativado',
      kind: 'policy',
      guidance: 'Habilite o uso compartilhado no contexto da conta, sem trocar seu Codex.',
      actionLabel: 'Habilitar uso',
    };
  if (account.policy !== 'shared' && account.policy !== 'applications_only')
    return { ...sessionPresentations.unverified, label: 'Uso não verificado' };
  return applicationSessionPresentation(account);
}
