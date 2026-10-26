export type RecoveryAction =
  | 'retry'
  | 'check_connection'
  | 'reconnect'
  | 'replace_account'
  | 'diagnostics'
  | 'open_desktop'
  | 'select_desktop'
  | 'confirm_close'
  | 'close_browser';
export interface RecoveryChoice {
  action: RecoveryAction;
  label: string;
}
export interface OperationError {
  code: string;
  message: string;
}
export function operationError(cause: unknown): OperationError {
  const code =
    typeof cause === 'object' && cause !== null && 'code' in cause && typeof cause.code === 'string'
      ? cause.code
      : 'unknown';
  return { code, message: cause instanceof Error ? cause.message : String(cause) };
}
/** Error codes select UI shortcuts; the backend still decides what is safe. */
export function recoveryChoices(
  code: string | null,
  canReplace: boolean,
  windows = false,
): RecoveryChoice[] {
  if (code === 'browser_cleanup_pending')
    return [{ action: 'close_browser', label: 'Concluir fechamento do navegador' }];
  if (code === 'account_mismatch')
    return [
      { action: 'reconnect', label: 'Entrar na conta correta' },
      ...(canReplace
        ? [{ action: 'replace_account' as const, label: 'Quero substituir a conta…' }]
        : []),
    ];
  if (code === 'authentication_required')
    return [{ action: 'reconnect', label: 'Reconectar no navegador' }];
  if (code === 'session_renewal_required')
    return [
      { action: 'retry', label: 'Tentar recuperação novamente' },
      { action: 'reconnect', label: 'Entrar novamente no navegador' },
    ];
  if (code === 'credential_recovery_pending')
    return [{ action: 'diagnostics', label: 'Acompanhar recuperação da sessão' }];
  if (code === 'credential_unavailable' || code === 'keyring_error')
    return [{ action: 'check_connection', label: 'Verificar acesso ao cofre' }];
  if (code === 'desktop_reopen_pending' || code === 'desktop_launch_failed')
    return [{ action: 'open_desktop', label: 'Abrir Codex novamente' }];
  if (code === 'desktop_discovery_failed')
    return [
      { action: 'open_desktop', label: 'Detectar e abrir Codex' },
      ...(windows ? [{ action: 'select_desktop' as const, label: 'Selecionar executável…' }] : []),
      { action: 'diagnostics', label: 'Ver diagnóstico' },
    ];
  if (code === 'desktop_close_pending')
    return [
      { action: 'retry', label: 'Tentar novamente' },
      ...(windows ? [{ action: 'confirm_close' as const, label: 'Autorizar interrupção…' }] : []),
      { action: 'diagnostics', label: 'Ver o que impediu a troca' },
    ];
  if (
    ['runtime_busy', 'thread_owned_elsewhere', 'resource_busy', 'runtime_error'].includes(
      code ?? '',
    )
  )
    return [{ action: 'diagnostics', label: 'Abrir recuperação no Diagnóstico' }];
  return [{ action: 'retry', label: 'Tentar novamente' }];
}
