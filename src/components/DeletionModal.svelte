<script lang="ts">
  import type { DeletionRequest } from '../lib/integrations';
  import { integrationApi } from '../lib/integrations';
  import { buildSearchIndex, searchIndex } from '../lib/search';
  import Icon from './Icon.svelte';
  export let request: DeletionRequest;
  export let closed: () => void;
  let selected = new Set(request.items.map((item) => item.accountId));
  let query = '';
  let reviewed = false;
  let removeBrowser = false;
  let acknowledged = false;
  let busy = false;
  let error = '';
  $: index = buildSearchIndex(request.items, (item) => [item.name, item.email ?? '']);
  $: matches = searchIndex(index, query);
  function toggle(id: string, enabled: boolean): void {
    const next = new Set(selected);
    if (enabled) next.add(id);
    else next.delete(id);
    selected = next;
    reviewed = false;
    acknowledged = false;
  }
  async function confirm(): Promise<void> {
    busy = true;
    error = '';
    try {
      await integrationApi.confirmDeletion(request.id, [...selected], removeBrowser);
      closed();
    } catch (cause) {
      error = cause instanceof Error ? cause.message : String(cause);
    } finally {
      busy = false;
    }
  }
  async function deny(): Promise<void> {
    busy = true;
    try {
      await integrationApi.denyDeletion(request.id);
      closed();
    } catch (cause) {
      error = cause instanceof Error ? cause.message : String(cause);
      busy = false;
    }
  }
</script>

<div class="modal-backdrop" role="presentation">
  <div class="modal" role="dialog" aria-modal="true" aria-labelledby="deletion-title">
    <div class="modal-title-row">
      <Icon name="trash" size={23} />
      <div>
        <h2 id="deletion-title">Excluir dados locais?</h2>
        <p class="subtitle">
          Solicitado por {request.requester}. A conta remota não será revogada.
        </p>
      </div>
    </div>
    <label
      >Buscar na seleção<input aria-label="Buscar contas na exclusão" bind:value={query} /></label
    >
    <div class="row">
      <button
        class="button small"
        disabled={busy}
        onclick={() => matches.forEach((item) => toggle(item.accountId, true))}
        >Selecionar resultados</button
      ><button
        class="button small"
        disabled={busy}
        onclick={() => matches.forEach((item) => toggle(item.accountId, false))}
        >Desselecionar resultados</button
      >
    </div>
    <div class="deletion-items">
      {#each matches as item (item.accountId)}<label class="checkbox-option"
          ><input
            type="checkbox"
            disabled={busy}
            checked={selected.has(item.accountId)}
            onchange={(event) => toggle(item.accountId, event.currentTarget.checked)}
          /><span>{item.name}<small>{item.email ?? ''}</small></span></label
        >{/each}
    </div>
    <label class="checkbox-option"
      ><input
        type="checkbox"
        disabled={busy}
        bind:checked={removeBrowser}
        onchange={() => {
          reviewed = false;
          acknowledged = false;
        }}
      />Excluir também o perfil local do navegador e seus cookies</label
    >
    <p class="form-hint">
      Sem essa opção, a associação da conta e a sessão local são removidas, mas o perfil do
      navegador é mantido. Históricos e projetos do Codex não são excluídos.
    </p>
    {#if reviewed}<p class="notice error">
        Serão removidos os dados locais de {selected.size} conta(s){removeBrowser
          ? ' e os respectivos perfis de navegador'
          : ''}.
      </p>
      <label class="checkbox-option"
        ><input type="checkbox" disabled={busy} bind:checked={acknowledged} />Revisei as contas e o
        escopo; confirmo a exclusão.</label
      >{/if}
    {#if error}<p class="notice error" role="alert">{error}</p>{/if}
    <div class="row modal-actions">
      <button class="button" disabled={busy} onclick={deny}>Cancelar pedido</button
      >{#if !reviewed}<button
          class="button danger"
          disabled={busy || !selected.size}
          onclick={() => (reviewed = true)}>Revisar {selected.size} conta(s)</button
        >{:else}<button class="button danger" disabled={busy || !acknowledged} onclick={confirm}
          >{busy ? 'Excluindo com segurança…' : 'Excluir definitivamente'}</button
        >{/if}
    </div>
  </div>
</div>

<style>
  .deletion-items {
    max-height: 230px;
    overflow: auto;
  }
  .checkbox-option {
    display: flex;
    align-items: center;
    gap: 12px;
    margin: 10px 0;
  }
  .checkbox-option input {
    width: auto;
  }
  .checkbox-option small {
    display: block;
    opacity: 0.65;
  }
</style>
