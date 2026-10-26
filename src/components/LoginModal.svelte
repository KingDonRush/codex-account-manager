<script lang="ts">
  import { onMount } from 'svelte';
  import type {
    LoginOperation,
    ManagedProfile,
    OperationKind,
    OperationState,
    ReplacementPreview,
  } from '../lib/types';
  import Icon from './Icon.svelte';
  import { dialogFocus } from '../lib/dialog-focus';
  import ProviderPicker from './ProviderPicker.svelte';
  import OperationRecovery from './OperationRecovery.svelte';
  import type { OperationError, RecoveryAction } from '../lib/operation-recovery';
  import type { MailProvider } from '../lib/types';
  export let providers: MailProvider[] = [];
  export let useNow: () => void;
  export let savePreference: (enabled: boolean, providerId: string | null) => void;
  export let applicationSession = false;
  export let launchError: OperationError | null = null;
  export let preparingSwitch = false;
  export let recoveringDesktop = false;
  export let recover: (action: RecoveryAction) => void;
  let openEmail = false;
  let providerId: string | null = null;
  $: if (profile) {
    openEmail = profile.openMailWithLogin ?? false;
    providerId = profile.mailProviderId;
  }
  import { isOperationActive, operationLabel, OPERATION_STEPS, stepStatus } from '../lib/view';

  export let profile: ManagedProfile;
  export let reconnect: boolean;
  export let intent: OperationKind;
  export let operation: LoginOperation | null;
  export let cancelling: boolean;
  export let cancel: () => void;
  export let close: () => void;
  export let preview: ReplacementPreview | null = null;
  export let confirmReplacement: () => void;

  let now = Date.now();

  onMount(() => {
    const timer = window.setInterval(() => (now = Date.now()), 1000);
    return () => window.clearInterval(timer);
  });

  $: elapsed = operation ? elapsedLabel(operation, now) : '';
  $: switchSubtitle = preparingSwitch
    ? 'A conta ativa permanece intacta durante a preparação.'
    : launchError || (operation && !isOperationActive(operation.state))
      ? 'A operação terminou. Consulte o resultado e as ações disponíveis abaixo.'
      : 'A troca continua em segundo plano e esta janela mostra a etapa real.';
  $: elapsedSeconds = operation ? operationElapsedSeconds(operation, now) : 0;
  $: launchIsSlow =
    operation && intent !== 'switch'
      ? ['preparing', 'browser_launching'].includes(operation.state) && elapsedSeconds >= 10
      : false;
  $: displaySteps =
    intent === 'switch'
      ? [
          { state: 'preparing' as const, label: 'Preparando troca' },
          { state: 'verifying' as const, label: 'Validando e ativando conta' },
          { state: 'persisting' as const, label: 'Atualizando histórico e quota' },
        ]
      : OPERATION_STEPS;

  function operationElapsedSeconds(current: LoginOperation, currentTime: number): number {
    const startedAt = new Date(current.startedAt).getTime();
    const finishedAt = current.finishedAt ? new Date(current.finishedAt).getTime() : currentTime;
    return Math.max(0, Math.floor((finishedAt - startedAt) / 1000));
  }

  function elapsedLabel(current: LoginOperation, currentTime: number): string {
    const seconds = operationElapsedSeconds(current, currentTime);
    const minutes = Math.floor(seconds / 60);
    return `${minutes.toString().padStart(2, '0')}:${(seconds % 60).toString().padStart(2, '0')}`;
  }

  function operationGuidance(state: OperationState): string {
    if (intent === 'switch') {
      const switchMessages: Partial<Record<OperationState, string>> = {
        idle: 'A troca ainda não começou.',
        preparing: 'Preparando o estado local sem bloquear a interface.',
        verifying: 'Validando a credencial e reiniciando o Codex.',
        persisting: 'Concluindo a gravação e verificando a abertura do Desktop.',
        completed:
          operation?.errorCode === 'desktop_reopen_pending'
            ? 'A sessão foi ativada. Tente abrir o Codex abaixo, sem repetir a troca.'
            : 'Consulte o resultado acima para saber se o Codex foi reaberto.',
        cancelled: 'A conta ativa não foi alterada.',
        failed: 'A troca não foi concluída. O resultado abaixo informa a falha e a ação segura.',
      };
      return switchMessages[state] ?? 'A troca não pôde ser concluída.';
    }
    const messages: Record<OperationState, string> = {
      idle: 'A operação ainda não começou.',
      preparing: 'Preparando os arquivos isolados desta conta.',
      browser_launching: 'O Chromium deve aparecer em poucos segundos.',
      awaiting_login: 'Conclua o login na janela do Chromium.',
      awaiting_confirmation:
        'A nova conta foi validada; a associação anterior ainda não foi alterada.',
      verifying: 'Conferindo se a conta autenticada responde corretamente.',
      persisting: 'Protegendo a credencial no armazenamento do sistema.',
      completed:
        operation?.errorCode === 'browser_cleanup_pending'
          ? 'A conexão foi salva. Conclua o fechamento abaixo, sem refazer o login.'
          : 'A conta está pronta para uso.',
      cancelled: 'Nenhuma credencial parcial foi salva.',
      failed: 'Abra os detalhes abaixo para identificar a etapa.',
      timed_out: 'A etapa demorou mais do que o permitido.',
    };
    return messages[state];
  }
</script>

<div class="modal-backdrop" role="presentation">
  <div
    class="modal"
    tabindex="-1"
    use:dialogFocus={{
      escape: () => {
        if (preparingSwitch || launchError || (operation && !isOperationActive(operation.state)))
          close();
      },
    }}
    role="dialog"
    aria-modal="true"
    aria-labelledby="login-title"
  >
    <div class="modal-title-row">
      <span class="modal-icon"
        ><Icon
          name={intent === 'switch' ? 'log-in' : reconnect ? 'key-refresh' : 'link'}
          size={22}
        /></span
      >
      <div>
        <h2 id="login-title">
          {intent === 'replace'
            ? 'Substituir conta de'
            : intent === 'switch'
              ? 'Usar'
              : reconnect
                ? 'Reautenticar'
                : 'Conectar'}
          {profile.name}
        </h2>
        <p class="subtitle">
          {intent === 'switch'
            ? switchSubtitle
            : applicationSession
              ? 'Conexão própria para aplicativos, sem trocar a sessão do Desktop.'
              : 'Conclua o login no navegador. O e-mail só abre se você habilitar essa preferência.'}
        </p>
        {#if intent === 'replace'}<p class="form-hint">
            Entre na outra conta. Você verá a nova identidade antes de confirmar.
          </p>{:else if intent !== 'switch' && profile.email}<p class="form-hint">
            Entre no navegador como {profile.email}.
          </p>{/if}
      </div>
    </div>

    {#if operation}
      <div class="operation">
        <div
          class={`operation-hero ${operation.state}`}
          class:pending={operation.state === 'completed' && !!operation.errorCode}
        >
          <span class:active={isOperationActive(operation.state)} class="operation-state-icon">
            {#if isOperationActive(operation.state)}<span class="activity-ring" aria-hidden="true"
              ></span>
            {:else if operation.state === 'completed' && !operation.errorCode}<Icon
                name="circle-check"
                size={23}
              />
            {:else if operation.state === 'cancelled'}<Icon name="circle-x" size={23} />
            {:else}<Icon name="triangle-alert" size={23} />{/if}
          </span>
          <div>
            <strong>{operation.step || operationLabel(operation.state)}</strong>
            <span>{operationGuidance(operation.state)}</span>
          </div>
          <time datetime={`PT${elapsedSeconds}S`}>{elapsed}</time>
        </div>

        {#if launchIsSlow}
          <div class="operation-slow" role="status">
            <Icon name="info" size={17} />
            <span
              >O navegador deveria ter aberto. Cancele e tente novamente; não é necessário fechar o
              aplicativo.</span
            >
          </div>
        {/if}
        {#if intent !== 'switch' && operation.state === 'completed'}
          <label class="email-preference"
            ><input
              type="checkbox"
              bind:checked={openEmail}
              onchange={(event) => savePreference(event.currentTarget.checked, providerId)}
            />Abrir e-mail junto numa próxima vez?</label
          >
          {#if openEmail}<ProviderPicker
              {providers}
              selectedId={providerId}
              select={(id) => {
                providerId = id;
                savePreference(true, id);
              }}
            />{/if}
        {/if}
        {#if operation.errorMessage && operation.errorMessage !== operation.step}<div
            class="notice error"
          >
            {operation.errorMessage}
          </div>{/if}
        {#if operation.errorCode && !isOperationActive(operation.state)}<OperationRecovery
            code={operation.errorCode}
            canReplace={!!profile.accountId && !applicationSession}
            {recover}
            busy={recoveringDesktop}
          />{/if}
        {#if operation.state === 'awaiting_confirmation' && preview}<div
            class="replacement-preview"
          >
            <p>
              <span>Conta anterior</span><strong
                >{preview.previousEmail ?? 'Identidade anterior'}</strong
              >
            </p>
            <Icon name="arrow-right" size={17} />
            <p>
              <span>Nova conta validada</span><strong
                >{preview.nextEmail ?? 'Identidade validada'}{preview.nextPlan
                  ? ` · ${preview.nextPlan}`
                  : ''}</strong
              >
            </p>
            <button class="button primary" disabled={cancelling} onclick={confirmReplacement}
              >Confirmar substituição</button
            >
          </div>{/if}

        <details class="operation-details">
          <summary
            ><Icon name="activity" size={16} />Ver etapas da conexão <Icon
              name="chevron-down"
              size={14}
            /></summary
          >
          <div class="steps">
            {#each displaySteps as item (item.state)}
              {@const status = stepStatus(operation.state, item.state)}
              <div class:current={status === 'current'} class:done={status === 'done'} class="step">
                <span class="step-dot"></span>{item.label}
              </div>
            {/each}
          </div>
        </details>

        <div class="row modal-actions">
          {#if operation.cancelable}
            <button class="button danger" disabled={cancelling} onclick={cancel}>
              <Icon name={cancelling ? 'hourglass' : 'circle-x'} size={17} />
              {cancelling ? 'Cancelando…' : 'Cancelar operação'}
            </button>
          {:else if ['failed', 'timed_out', 'cancelled'].includes(operation.state)}
            {#if !operation.errorCode}<OperationRecovery code={null} {recover} />{/if}
            <button class="button" onclick={close}>Fechar</button>
          {:else if !isOperationActive(operation.state)}
            {#if operation.state === 'completed' && intent !== 'switch' && !applicationSession}<button
                class="button primary"
                onclick={useNow}>Usar esta conta agora</button
              >{/if}
            <button class="button" onclick={close}>Fechar</button>
          {:else}
            <span class="subtitle">Finalizando de forma segura…</span>
          {/if}
        </div>
      </div>
    {:else if launchError}
      <div class="notice error" role="alert">{launchError.message}</div>
      <p class="form-hint" role="status">
        A operação não chegou a começar. Nenhuma conta foi trocada.
      </p>
      <OperationRecovery
        code={launchError.code}
        canReplace={!!profile.accountId && !applicationSession}
        {recover}
        busy={recoveringDesktop}
      />
      <div class="row modal-actions"><button class="button" onclick={close}>Fechar</button></div>
    {:else}
      <div class="operation-hero preparing" role="status" aria-live="polite">
        <span class="operation-state-icon active"
          ><span class="activity-ring" aria-hidden="true"></span></span
        >
        <div>
          <strong
            >{preparingSwitch
              ? 'Conferindo o uso desta conta'
              : intent === 'switch'
                ? 'Iniciando a troca'
                : 'Iniciando a conexão'}</strong
          >
          <span
            >{intent === 'switch'
              ? 'A conta ativa permanece intacta durante a preparação.'
              : 'O navegador será aberto automaticamente.'}</span
          >
        </div>
      </div>
      {#if preparingSwitch}<div class="row modal-actions">
          <button class="button" onclick={close}>Cancelar preparação</button>
        </div>{/if}
    {/if}
  </div>
</div>

<style>
  .email-preference {
    display: flex;
    align-items: center;
    gap: 10px;
    margin: 16px 0;
  }
  .email-preference input {
    width: auto;
  }
</style>
