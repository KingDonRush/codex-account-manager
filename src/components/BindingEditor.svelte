<script lang="ts">
  import type { ExecutionBinding, IntegrationSnapshot } from '../lib/integrations';
  import Icon from './Icon.svelte';
  import { dialogFocus } from '../lib/dialog-focus';
  export let binding: ExecutionBinding;
  export let snapshot: IntegrationSnapshot;
  export let save: (binding: ExecutionBinding) => Promise<void>;
  export let close: () => void;
  let draft = structuredClone(binding);
  let busy = false;
  let error = '';
  async function submit(): Promise<void> {
    busy = true;
    error = '';
    try {
      await save(draft);
      close();
    } catch (cause) {
      error = cause instanceof Error ? cause.message : String(cause);
    } finally {
      busy = false;
    }
  }
  function setKind(kind: 'account' | 'group'): void {
    draft = { ...draft, target: { kind, id: '' }, automatic: false };
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
    aria-labelledby="binding-title"
  >
    <h2 id="binding-title">Vínculo de execução</h2>
    <p class="subtitle">
      Escolha quais contas esta rotina pode usar. O vínculo não concede acesso por si só.
    </p>
    <label
      >Nome<input
        bind:value={draft.name}
        maxlength="80"
        placeholder="Pesquisa"
        disabled={busy}
      /></label
    >
    <label
      >Usar<select
        value={draft.target.kind}
        disabled={busy}
        onchange={(event) => setKind(event.currentTarget.value as 'account' | 'group')}
        ><option value="account">Conta específica</option><option value="group"
          >Grupo de contas</option
        ></select
      ></label
    >
    <label
      >{draft.target.kind === 'group' ? 'Grupo' : 'Conta'}<select
        aria-label={draft.target.kind === 'group' ? 'Grupo' : 'Conta'}
        bind:value={draft.target.id}
        disabled={busy}
      >
        <option value="">Selecionar…</option
        >{#if draft.target.kind === 'group'}{#each snapshot.groups.filter((group) => !group.archived) as group}<option
              value={group.id}>{group.name}</option
            >{/each}{:else}{#each snapshot.accounts as account}<option value={account.id}
              >{account.name}</option
            >{/each}{/if}
      </select></label
    >
    {#if draft.target.kind === 'group'}<label class="checkbox-row"
        ><input type="checkbox" bind:checked={draft.automatic} disabled={busy} />Escolher a melhor
        conta disponível antes de cada trabalho</label
      >{/if}
    <details class="form-disclosure">
      <summary><Icon name="settings" size={16} />Preferências avançadas</summary>
      <label class="checkbox-row"
        ><input type="checkbox" bind:checked={draft.allowProjectedQuota} disabled={busy} />Permitir
        saldo projetado quando ainda não confirmado</label
      >
      <label
        >Modelos permitidos<input
          value={draft.allowedModels.join(', ')}
          disabled={busy}
          oninput={(event) =>
            (draft = {
              ...draft,
              allowedModels: event.currentTarget.value
                .split(',')
                .map((value) => value.trim())
                .filter(Boolean),
            })}
          placeholder="Sem restrição se vazio"
        /></label
      >
      <label
        >Modelo padrão<input
          value={draft.defaultModel ?? ''}
          disabled={busy}
          oninput={(event) =>
            (draft = { ...draft, defaultModel: event.currentTarget.value.trim() || null })}
        /></label
      >
      <p class="form-hint">
        Sandbox, ferramentas e aprovação de ações são configurados pelo aplicativo consumidor.
      </p>
    </details>
    {#if error}<p class="notice error" role="alert">{error}</p>{/if}{#if busy}<p
        role="status"
        class="form-hint"
      >
        Salvando vínculo…
      </p>{/if}
    <div class="row modal-actions">
      <button
        class="button primary"
        disabled={busy || !draft.name.trim() || !draft.target.id}
        onclick={submit}>Salvar vínculo</button
      ><button class="button" disabled={busy} onclick={close}>Cancelar</button>
    </div>
  </div>
</div>

<style>
  .checkbox-row {
    display: flex;
    gap: 8px;
    align-items: start;
  }
  .checkbox-row input {
    width: 16px;
    min-height: 16px;
    margin-top: 2px;
    flex: none;
  }
</style>
