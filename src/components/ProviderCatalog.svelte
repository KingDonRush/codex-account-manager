<script lang="ts">
  import type { MailProvider } from '../lib/types';
  import Icon from './Icon.svelte';
  import { buildSearchIndex, searchIndex } from '../lib/search';

  export let providers: MailProvider[];
  export let edit: (provider: MailProvider | null) => void;

  let query = '';
  let showAll = false;
  const commonProviderOrder = new Map([
    ['gmail', 0],
    ['yahoo', 1],
    ['outlook', 2],
    ['hotmail', 3],
    ['proton', 4],
    ['icloud', 5],
  ]);

  $: normalizedQuery = query.trim().toLocaleLowerCase();
  $: orderedProviders = [...providers].sort((first, second) =>
    Number(second.isCustom) - Number(first.isCustom)
    || (commonProviderOrder.get(first.id) ?? 100) - (commonProviderOrder.get(second.id) ?? 100)
    || first.displayName.localeCompare(second.displayName),
  );
  $: index = buildSearchIndex(orderedProviders, (provider) => [provider.displayName, ...provider.domains, ...provider.aliases]);
  $: matches = searchIndex(index, query);
  $: visibleProviders = showAll || normalizedQuery ? matches : matches.slice(0, 6);

</script>

<section class="provider-panel">
  <div class="provider-toolbar">
    <label class="search-field">
      <Icon name="search" size={18} />
      <span class="sr-only">Buscar provedor</span>
      <input bind:value={query} placeholder="Buscar por nome ou domínio" />
    </label>
    <button class="button" onclick={() => edit(null)}><Icon name="plus" size={17} />Adicionar provedor</button>
  </div>

  <p class="section-hint">Mostrando {visibleProviders.length} de {matches.length} provedores. Abra um item para ver URL e aliases.</p>

  {#if visibleProviders.length === 0}
    <div class="empty-state small-empty">
      <span class="empty-icon"><Icon name="search" size={25} /></span>
      <h2>Nenhum provedor encontrado</h2>
      <p>Tente outro nome ou domínio.</p>
    </div>
  {:else}
    <div class="provider-list">
      {#each visibleProviders as provider (provider.id)}
        <details class="provider-item">
          <summary>
            <span class="provider-icon"><Icon name="mail" size={18} /></span>
            <span class="provider-main">
              <strong>{provider.displayName}</strong>
              <span>{provider.domains.slice(0, 2).join(' · ') || 'Sem domínio cadastrado'}</span>
            </span>
            {#if provider.isCustom}<span class="custom-pill">Personalizado</span>{/if}
            <Icon name="chevron-down" size={16} />
          </summary>
          <div class="provider-details">
            <dl>
              <div><dt>URL padrão</dt><dd>{provider.defaultWebmailUrl}</dd></div>
              <div><dt>Domínios</dt><dd>{provider.domains.join(', ') || '—'}</dd></div>
              {#if provider.aliases.length}<div><dt>Aliases</dt><dd>{provider.aliases.join(', ')}</dd></div>{/if}
            </dl>
            <button class="button small" onclick={() => edit(provider)}><Icon name="pencil" size={15} />Editar</button>
          </div>
        </details>
      {/each}
    </div>
  {/if}

  {#if !normalizedQuery && matches.length > 6}
    <button class="button reveal-button" onclick={() => (showAll = !showAll)}>
      <Icon name={showAll ? 'chevron-down' : 'plus'} size={16} />
      {showAll ? 'Mostrar menos' : `Mostrar todos os ${matches.length} provedores`}
    </button>
  {/if}
</section>
