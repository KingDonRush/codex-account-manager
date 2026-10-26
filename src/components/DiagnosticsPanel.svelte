<script lang="ts">
  import type {
    LoginOperation,
    ManagedProfile,
    OperationStage,
    UpdateInfo,
    UpdateProgress,
    SharedRuntimeSnapshot,
    RuntimeRecoveryReport,
    RuntimeConsent,
  } from '../lib/types';
  import { isOperationActive, operationLabel } from '../lib/view';
  import Icon from './Icon.svelte';
  import OperationTimeline from './OperationTimeline.svelte';
  import RuntimePanel from './RuntimePanel.svelte';

  export let operations: LoginOperation[];
  export let profiles: ManagedProfile[];
  export let exportDiagnostics: () => void;
  export let availableUpdate: UpdateInfo | null;
  export let updateProgress: UpdateProgress | null;
  export let checkingUpdate: boolean;
  export let installingUpdate: boolean;
  export let recoveringAccounts: boolean;
  export let recoverSavedAccounts: () => void;
  export let checkForUpdate: () => void;
  export let installUpdate: () => void;
  export let loadOperationStages: (operationId: string) => Promise<OperationStage[]>;
  export let inspectRuntime: () => Promise<SharedRuntimeSnapshot>;
  export let recoverChat: (
    thread: string,
    consent: RuntimeConsent | null,
  ) => Promise<RuntimeRecoveryReport>;
  export let openChat: (thread: string) => Promise<void>;

  let showAllOperations = false;
  let expandedOperations = new Set<string>();

  $: failures = operations.filter((operation) =>
    ['failed', 'timed_out'].includes(operation.state),
  ).length;
  $: activeOperations = operations.filter((operation) => isOperationActive(operation.state)).length;
  $: visibleOperations = showAllOperations ? operations : operations.slice(0, 5);

  function profileName(profileId: string): string {
    return profiles.find((profile) => profile.id === profileId)?.name ?? 'Perfil removido';
  }

  function durationLabel(durationMs: number | null): string {
    if (durationMs === null) return 'em andamento';
    const seconds = Math.floor(durationMs / 1000);
    return seconds < 60 ? `${seconds}s` : `${Math.floor(seconds / 60)}m ${seconds % 60}s`;
  }

  function operationTone(operation: LoginOperation): 'success' | 'warning' | 'neutral' | 'active' {
    if (operation.state === 'completed') return operation.errorCode ? 'warning' : 'success';
    if (['failed', 'timed_out'].includes(operation.state)) return 'warning';
    if (isOperationActive(operation.state)) return 'active';
    return 'neutral';
  }

  function revealTimeline(event: Event, operationId: string): void {
    const details = event.currentTarget as HTMLDetailsElement;
    if (!details.open || expandedOperations.has(operationId)) return;
    expandedOperations = new Set([...expandedOperations, operationId]);
  }
</script>

<div class="diagnostics-layout">
  <section class="health-overview" aria-label="Resumo do diagnóstico">
    <div class="health-card">
      <span class="health-icon good"><Icon name="shield-check" size={20} /></span>
      <div>
        <strong
          >{profiles.filter((profile) => !profile.lastError && profile.accountId).length}</strong
        ><span>contas prontas</span>
      </div>
    </div>
    <div class="health-card">
      <span class="health-icon"><Icon name="activity" size={20} /></span>
      <div><strong>{activeOperations}</strong><span>em andamento</span></div>
    </div>
    <div class="health-card">
      <span class:warning={failures > 0} class="health-icon"
        ><Icon name="triangle-alert" size={20} /></span
      >
      <div><strong>{failures}</strong><span>falhas registradas</span></div>
    </div>
  </section>

  <section class="operations-panel">
    <div class="section-heading">
      <div>
        <h2>Atividade recente</h2>
        <p>Abra uma operação somente quando precisar dos detalhes.</p>
      </div>
      <span>{operations.length} no histórico</span>
    </div>

    {#if operations.length === 0}
      <div class="empty-state small-empty">
        <span class="empty-icon"><Icon name="activity" size={25} /></span>
        <h2>Nenhuma operação ainda</h2>
        <p>Conexões e trocas aparecerão aqui.</p>
      </div>
    {:else}
      <div class="operation-list">
        {#each visibleOperations as operation (operation.id)}
          {@const tone = operationTone(operation)}
          <details class="operation-item" ontoggle={(event) => revealTimeline(event, operation.id)}>
            <summary>
              <span class={`operation-result ${tone}`}>
                {#if tone === 'success'}<Icon name="circle-check" size={18} />
                {:else if tone === 'warning'}<Icon name="triangle-alert" size={18} />
                {:else if tone === 'active'}<Icon name="hourglass" size={18} />
                {:else}<Icon name="circle-x" size={18} />{/if}
              </span>
              <span class="operation-main">
                <strong>{profileName(operation.profileId)}</strong>
                <span
                  >{operation.state === 'completed' && operation.errorCode ? 'Concluída com pendência' : operationLabel(operation.state)} · {new Date(
                    operation.startedAt,
                  ).toLocaleString()}</span
                >
              </span>
              <span class={`operation-badge ${tone}`}>{operation.state === 'completed' && operation.errorCode ? 'Com pendência' : operationLabel(operation.state)}</span>
              <Icon name="chevron-down" size={16} />
            </summary>
            <div class="operation-metadata">
              <dl>
                <div>
                  <dt>Etapa</dt>
                  <dd>{operation.step}</dd>
                </div>
                <div>
                  <dt>Última etapa</dt>
                  <dd>{durationLabel(operation.lastStageDurationMs)}</dd>
                </div>
                <div>
                  <dt>Duração</dt>
                  <dd>{durationLabel(operation.durationMs)}</dd>
                </div>
                <div>
                  <dt>Navegador</dt>
                  <dd>
                    {operation.browserPid
                      ? `${operation.runtimeKind ?? 'browser'} · PID ${operation.browserPid}`
                      : 'Não iniciado'}
                  </dd>
                </div>
                {#if operation.errorCode}<div>
                    <dt>Código do erro</dt>
                    <dd>{operation.errorCode}</dd>
                  </div>{/if}
              </dl>
              {#if expandedOperations.has(operation.id)}
                <OperationTimeline operationId={operation.id} load={loadOperationStages} />
              {/if}
            </div>
          </details>
        {/each}
      </div>
      {#if operations.length > 5}
        <button
          class="button reveal-button"
          onclick={() => (showAllOperations = !showAllOperations)}
        >
          <Icon name={showAllOperations ? 'chevron-down' : 'plus'} size={16} />
          {showAllOperations ? 'Mostrar menos' : `Mostrar mais ${operations.length - 5}`}
        </button>
      {/if}
    {/if}
  </section>

  <details class="maintenance-panel">
    <summary>
      <span class="health-icon"><Icon name="settings" size={20} /></span>
      <span class="maintenance-copy"
        ><strong>Manutenção e exportação</strong><span
          >Contas salvas, atualizações e diagnóstico sanitizado</span
        ></span
      >
      <Icon name="chevron-down" size={16} />
    </summary>
    <div class="maintenance-content">
      <div class="maintenance-row">
        <div>
          <strong>Contas de instalações anteriores</strong>
          <p>
            Procura sessões salvas pelo gerenciador desde a 0.3. Não altera o Codex em uso, perfis
            arquivados ou exclusões definitivas.
          </p>
        </div>
        <button class="button" disabled={recoveringAccounts} onclick={recoverSavedAccounts}>
          <Icon name="refresh" size={17} />{recoveringAccounts
            ? 'Procurando…'
            : 'Recuperar contas salvas'}
        </button>
      </div>
      <div class="maintenance-row">
        <div>
          <strong>Atualizações assinadas</strong>
          <p>
            {availableUpdate
              ? `Versão ${availableUpdate.version} disponível. ${availableUpdate.body ?? ''}`
              : 'Verifique se há uma nova versão validada pela chave incorporada.'}
          </p>
          {#if updateProgress}
            <p>
              Baixados {Math.round(updateProgress.downloadedBytes / 1024 / 1024)} MB
              {updateProgress.totalBytes
                ? ` de ${Math.round(updateProgress.totalBytes / 1024 / 1024)} MB`
                : ''}.
            </p>
          {/if}
        </div>
        {#if availableUpdate}
          <button class="button primary" disabled={installingUpdate} onclick={installUpdate}>
            <Icon name="download" size={17} />{installingUpdate
              ? 'Instalando…'
              : `Instalar ${availableUpdate.version}`}
          </button>
        {:else}
          <button class="button" disabled={checkingUpdate} onclick={checkForUpdate}>
            <Icon name="refresh" size={17} />{checkingUpdate
              ? 'Verificando…'
              : 'Verificar atualizações'}
          </button>
        {/if}
      </div>
      <div class="maintenance-row">
        <div>
          <strong>Pacote de diagnóstico</strong>
          <p>Exporta eventos sanitizados, sem tokens, cookies ou URLs OAuth.</p>
        </div>
        <button class="button" onclick={exportDiagnostics}
          ><Icon name="download" size={17} />Exportar</button
        >
      </div>
    </div>
  </details>
  <RuntimePanel inspect={inspectRuntime} recover={recoverChat} open={openChat} />
</div>
