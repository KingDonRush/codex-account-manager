<script lang="ts">
  import type { AccountGroup, IntegrationSnapshot } from '../lib/integrations';
  import { integrationApi } from '../lib/integrations';
  import { buildSearchIndex, searchIndex } from '../lib/search';
  import Icon from './Icon.svelte';
  import { dialogFocus } from '../lib/dialog-focus';
  export let snapshot: IntegrationSnapshot;
  export let changed: () => Promise<void>;
  export let close: () => void;
  let query = '';
  let memberQuery = '';
  let archived = false;
  let draft: AccountGroup | null = null;
  let error = '';
  let busy = false;
  $: index = buildSearchIndex(snapshot.groups, (group) => [group.name]);
  $: groups = searchIndex(index, query).filter((group) => group.archived === archived);
  $: memberIndex = buildSearchIndex(snapshot.accounts, (account) => [
    account.name,
    account.email ?? '',
  ]);
  $: candidates = searchIndex(memberIndex, memberQuery);
  function create(): void {
    draft = {
      id: crypto.randomUUID(),
      name: '',
      accountIds: [],
      archived: false,
      revision: 0,
      exclusiveApplicationId: null,
    };
  }
  function toggle(id: string, selected: boolean): void {
    if (!draft) return;
    draft = {
      ...draft,
      accountIds: selected
        ? [...new Set([...draft.accountIds, id])]
        : draft.accountIds.filter((member) => member !== id),
    };
  }
  async function save(group: AccountGroup): Promise<void> {
    busy = true;
    error = '';
    try {
      await integrationApi.saveGroup(group);
      draft = null;
      await changed();
    } catch (cause) {
      error = cause instanceof Error ? cause.message : String(cause);
    } finally {
      busy = false;
    }
  }
  async function remove(group: AccountGroup): Promise<void> {
    if (
      !window.confirm(`Remover o grupo “${group.name}”? Suas contas e credenciais serão mantidas.`)
    )
      return;
    busy = true;
    try {
      await integrationApi.deleteGroup(group.id, group.revision);
      await changed();
    } catch (cause) {
      error = cause instanceof Error ? cause.message : String(cause);
    } finally {
      busy = false;
    }
  }
</script>

{#if !draft}<div class="modal-backdrop" role="presentation">
    <div
      class="modal groups-modal"
      tabindex="-1"
      use:dialogFocus={{
        escape: () => {
          if (!busy) close();
        },
      }}
      role="dialog"
      aria-modal="true"
      aria-labelledby="groups-title"
    >
      <div class="modal-title-row">
        <span class="modal-icon"><Icon name="accounts" size={22} /></span>
        <div>
          <h2 id="groups-title">Grupos de contas</h2>
          <p class="subtitle">Organize os filtros de Contas e Aplicações.</p>
        </div>
      </div>
      <section aria-label="Grupos de contas">
        <div class="accounts-toolbar">
          <label class="search-field"
            ><Icon name="search" size={17} /><input
              data-area-search
              aria-label="Buscar grupos"
              placeholder="Buscar grupos"
              bind:value={query}
            /></label
          >
          <button class="button" onclick={() => (archived = !archived)}
            >{archived ? 'Ativos' : 'Arquivados'}</button
          >
          <button class="button primary" onclick={create}
            ><Icon name="plus" size={17} />Criar grupo</button
          >
        </div>
        {#if error}<p class="notice error" role="alert">{error}</p>{/if}
        <p class="section-hint">
          Uma conta pode participar de vários grupos. Remover um grupo não exclui suas contas.
        </p>
        {#if !groups.length}<div class="empty-state compact-empty">
            <Icon name="accounts" size={28} />
            <h2>Nenhum grupo nesta lista</h2>
          </div>{/if}
        <div class="provider-list">
          {#each groups as group (group.id)}
            <details class="provider-item">
              <summary
                ><Icon name="accounts" size={19} /><strong>{group.name}</strong><span
                  >{group.accountIds.length} contas</span
                ></summary
              >
              <div class="provider-details">
                <p>
                  {snapshot.accounts
                    .filter((account) => group.accountIds.includes(account.id))
                    .map((account) => account.name)
                    .join(' · ') || 'Grupo vazio'}
                </p>
                <div class="row">
                  <button class="button" onclick={() => (draft = structuredClone(group))}
                    >Editar contas e nome</button
                  >
                  <button
                    class="button"
                    disabled={busy}
                    onclick={() => {
                      if (
                        group.paused ||
                        window.confirm(
                          `Pausar “${group.name}” e cancelar os trabalhos deste grupo? Outros grupos e o Desktop continuam.`,
                        )
                      )
                        void save({ ...group, paused: !group.paused });
                    }}>{group.paused ? 'Retomar' : 'Pausar e cancelar trabalhos'}</button
                  >
                  <button
                    class="button"
                    disabled={busy}
                    onclick={() => save({ ...group, archived: !group.archived })}
                    >{group.archived ? 'Restaurar' : 'Arquivar'}</button
                  >
                  <button class="button danger" disabled={busy} onclick={() => remove(group)}
                    >Remover grupo</button
                  >
                </div>
              </div>
            </details>
          {/each}
        </div>
      </section>
      <div class="row modal-actions"><button class="button" onclick={close}>Fechar</button></div>
    </div>
  </div>{/if}
{#if draft}
  <div class="modal-backdrop" role="presentation">
    <div
      class="modal"
      tabindex="-1"
      use:dialogFocus={{
        escape: () => {
          if (!busy) draft = null;
        },
      }}
      role="dialog"
      aria-modal="true"
      aria-labelledby="group-title"
    >
      <h2 id="group-title">{draft.revision ? 'Editar grupo' : 'Criar grupo'}</h2>
      <label>Nome<input bind:value={draft.name} maxlength={80} /></label>
      <p class="form-hint">
        O nome e os IDs dos integrantes fazem parte do catálogo público local para integrações.
      </p>
      <label>Buscar contas<input aria-label="Buscar integrantes" bind:value={memberQuery} /></label>
      <div class="row">
        <button
          class="button small"
          onclick={() => candidates.forEach((account) => toggle(account.id, true))}
          >Selecionar resultados</button
        ><button
          class="button small"
          onclick={() => candidates.forEach((account) => toggle(account.id, false))}
          >Desselecionar resultados</button
        >
      </div>
      <div class="member-list">
        {#each candidates as account (account.id)}<label class="member-option"
            ><input
              type="checkbox"
              checked={draft.accountIds.includes(account.id)}
              onchange={(event) => toggle(account.id, event.currentTarget.checked)}
            /><span>{account.name}<small>{account.email ?? ''}</small></span></label
          >{/each}
      </div>
      <div class="row modal-actions">
        <button
          class="button primary"
          disabled={busy || !draft.name.trim()}
          onclick={() => draft && save(draft)}>Salvar</button
        ><button class="button" disabled={busy} onclick={() => (draft = null)}>Cancelar</button>
      </div>
      {#if error}<p class="notice error" role="alert">{error}</p>{/if}
    </div>
  </div>
{/if}

<style>
  .groups-modal {
    width: min(700px, calc(100vw - 32px));
  }
  .member-list {
    max-height: 280px;
    overflow: auto;
  }
  .member-option {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 8px;
  }
  .member-option input {
    width: auto;
  }
  .member-option small {
    display: block;
    opacity: 0.65;
  }
  summary {
    gap: 12px;
  }
</style>
