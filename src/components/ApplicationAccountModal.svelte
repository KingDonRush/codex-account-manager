<script lang="ts">
  import { integrationApi, type IntegrationAccount, type AccountPolicy } from '../lib/integrations';
  import Icon from './Icon.svelte';
  import { dialogFocus } from '../lib/dialog-focus';
  import AccountUsageSettings from './AccountUsageSettings.svelte';
  import PlanBadge from './PlanBadge.svelte';
  import { applicationSessionPresentation } from '../lib/application-access';
  export let account: IntegrationAccount;
  export let changed: () => Promise<void>;
  export let connectSession: (profileId: string) => void;
  export let close: () => void;
  let policy: AccountPolicy = account.policy;
  let capacity = account.capacity;
  let busy = false;
  let error = '';
  let feedback = '';
  $: dirty = policy !== account.policy || capacity !== account.capacity;
  $: session = applicationSessionPresentation(account);
  async function commit(action: () => Promise<void>, message: string): Promise<void> {
    busy = true;
    error = '';
    feedback = '';
    try {
      await action();
      await changed();
      feedback = message;
    } catch (cause) {
      error = cause instanceof Error ? cause.message : String(cause);
    } finally {
      busy = false;
    }
  }
  function save(): void {
    if (!Number.isInteger(capacity) || capacity < 1 || capacity > 32) {
      error = 'Capacidade deve ser um número inteiro de 1 a 32.';
      return;
    }
    void commit(
      () => integrationApi.accountPolicy(account.id, policy, capacity),
      'Permissão e capacidade salvas.',
    );
  }
  async function connect(): Promise<void> {
    if (policy !== 'shared' && policy !== 'applications_only') {
      error = 'Escolha Codex e aplicações ou Só aplicações antes de conectar.';
      return;
    }
    if (dirty) {
      await commit(
        () => integrationApi.accountPolicy(account.id, policy, capacity),
        'Uso da conta salvo.',
      );
      if (error) return;
    }
    const id = account.profileId;
    close();
    connectSession(id);
  }
</script>

<div class="modal-backdrop" role="presentation">
  <div
    class="modal compact-modal"
    tabindex="-1"
    use:dialogFocus={{
      escape: () => {
        if (!busy) close();
      },
    }}
    role="dialog"
    aria-modal="true"
    aria-labelledby="resource-title"
  >
    <div class="modal-title-row">
      <span class="modal-icon"><Icon name="accounts" size={22} /></span>
      <div class="identity">
        <h2 id="resource-title">{account.name}</h2>
        <p class="subtitle">{account.email ?? 'Conta para aplicações'}</p>
        <PlanBadge plan={account.plan} />
      </div>
    </div>
    <div class="account-settings-content">
      <section class="usage-section">
        <AccountUsageSettings bind:policy bind:capacity disabled={busy} />
        <button class="button primary" disabled={busy || !dirty} onclick={save}
          >Salvar uso da conta</button
        >
      </section>
      <section class="session-section" aria-label="Sessão para aplicações">
        <h3>Sessão para aplicações</h3>
        <p class="subtitle">{session.label}</p>
        <p class="form-hint">{session.guidance}</p>
        {#if session.kind === 'recovery'}<button
            class="button primary"
            disabled={busy}
            onclick={() =>
              commit(
                () => integrationApi.recoverCredential(account.profileId),
                'Gravação no cofre concluída.',
              )}><Icon name="shield-check" size={16} />Concluir gravação no cofre</button
          >
        {:else if session.actionLabel}<button
            class="button"
            disabled={busy || (policy !== 'shared' && policy !== 'applications_only')}
            onclick={connect}><Icon name="link" size={16} />{session.actionLabel}</button
          >{/if}
      </section>
      {#if busy}<p role="status" class="form-hint">Salvando com segurança…</p>{/if}{#if feedback}<p
          role="status"
          class="notice"
        >
          {feedback}
        </p>{/if}{#if error}<p role="alert" class="notice error">{error}</p>{/if}
    </div>
    <div class="row modal-actions">
      <button class="button" disabled={busy} onclick={close}>Fechar</button>
    </div>
  </div>
</div>

<style>
  .modal {
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }
  .modal-title-row,
  .modal-actions {
    flex: none;
  }
  .account-settings-content {
    min-height: 0;
    overflow-y: auto;
  }
  .modal-actions {
    margin-top: 14px;
  }
  .identity {
    min-width: 0;
    overflow-wrap: anywhere;
  }
  .usage-section {
    margin-top: 20px;
  }
  .usage-section > .button {
    margin-top: 14px;
  }
  .session-section {
    padding: 16px;
    background: var(--soft);
    border-radius: 10px;
    margin: 16px 0;
  }
  h3 {
    font-size: 14px;
    margin: 0 0 8px;
  }
  .session-section .button {
    margin-top: 10px;
  }
</style>
