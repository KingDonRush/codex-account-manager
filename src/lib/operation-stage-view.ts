const STAGE_LABELS: Readonly<Record<string, string>> = {
  switch_lock_wait: 'Fila de troca',
  shared_runtime_preflight: 'Coordenação da sessão com outras integrações',
  read_previous_auth: 'Leitura da sessão anterior',
  credential_generation_refs: 'Consulta ao cofre de sessões',
  verify_existing_session: 'Validação da sessão já ativa',
  preserve_previous_session: 'Proteção da sessão anterior',
  desktop_process_discovery: 'Localização do Codex',
  desktop_installation_discovery: 'Descoberta da instalação do Codex',
  desktop_discovered_store: 'Instalação da Microsoft Store identificada',
  desktop_discovered_configured: 'Executável selecionado identificado',
  desktop_discovered_system: 'Instalação tradicional identificada',
  desktop_graceful_signal: 'Pedido para fechar o Codex',
  desktop_graceful_wait: 'Espera pelo fechamento do Codex',
  desktop_force_signal: 'Encerramento forçado do Codex',
  desktop_force_wait: 'Confirmação do encerramento',
  desktop_stop: 'Fechamento total do Codex',
  credential_generation_load: 'Leitura da credencial',
  materialize_target_auth: 'Ativação local da credencial',
  app_server_spawn_initialize: 'Inicialização do App Server',
  app_server_account_read: 'Validação da identidade',
  app_server_rate_limits_read: 'Atualização da quota',
  app_server_rate_limits_unavailable: 'Tentativa de atualizar quota',
  app_server_snapshot_parse: 'Leitura da resposta da conta',
  read_verified_auth: 'Leitura da sessão renovada',
  validate_verified_auth: 'Conferência da conta ativada',
  verify_target_session: 'Validação completa da sessão',
  activate_credential_generation: 'Confirmação da credencial',
  persist_target_session: 'Atualização do cofre de sessões',
  rollback_auth: 'Restauração da sessão anterior',
  desktop_restart_after_rollback: 'Reabertura após restauração',
  desktop_restart: 'Reabertura do Codex',
  desktop_restart_failed: 'Abertura não confirmada; sessão preservada',
  desktop_restart_not_required: 'Reabertura não executada: não necessária',
  persist_switch_metadata: 'Atualização do histórico',
  close_previous_browser: 'Fechamento do navegador anterior',
  browser_runtime_ready: 'Verificação do Chromium',
  create_isolated_codex_home: 'Preparação da autenticação isolada',
  app_server_start_login: 'Criação do link de autenticação',
  browser_process_launch: 'Abertura do Chromium',
  validate_authenticated_session: 'Validação completa do login',
};

export function operationStageLabel(name: string): string {
  return STAGE_LABELS[name] ?? name.replaceAll('_', ' ');
}

export function stageDurationLabel(durationMs: number): string {
  if (durationMs < 1_000) return `${durationMs} ms`;
  if (durationMs < 60_000) return `${(durationMs / 1_000).toFixed(2)} s`;
  const minutes = Math.floor(durationMs / 60_000);
  const seconds = Math.round((durationMs % 60_000) / 1_000);
  return `${minutes} min ${seconds} s`;
}
