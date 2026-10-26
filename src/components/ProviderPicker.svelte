<script lang="ts">
  import type { MailProvider } from '../lib/types';
  import { buildSearchIndex, searchIndex } from '../lib/search';
  import Icon from './Icon.svelte';
  export let providers: MailProvider[];
  export let selectedId: string | null;
  export let select: (id: string) => void;
  let query = '';
  $: index = buildSearchIndex(providers, (provider) => [
    provider.displayName,
    ...provider.domains,
    ...provider.aliases,
  ]);
  $: matches = searchIndex(index, query);
</script>

<label
  >Buscar provedor<input
    aria-label="Buscar provedor de e-mail"
    placeholder="Gmail, Yahoo, Outlook…"
    bind:value={query}
  /></label
>
<div class="provider-picker" role="group" aria-label="Escolher provedor">
  {#each matches as provider (provider.id)}<button
      class="provider-choice"
      class:selected={selectedId === provider.id}
      aria-pressed={selectedId === provider.id}
      onclick={() => select(provider.id)}
      ><Icon name="mail" size={19} /><span>{provider.displayName}</span
      >{#if selectedId === provider.id}<Icon name="circle-check" size={16} />{/if}</button
    >{/each}
  {#if !matches.length}<p>Nenhum provedor corresponde à busca.</p>{/if}
</div>

<style>
  .provider-picker {
    max-height: 220px;
    overflow: auto;
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 6px;
  }
  .provider-choice {
    display: flex;
    align-items: center;
    gap: 9px;
    padding: 10px;
    border: 1px solid var(--line);
    background: var(--surface);
    color: inherit;
    border-radius: 9px;
    text-align: left;
  }
  .provider-choice.selected {
    border-color: var(--accent);
  }
  .provider-choice:hover,
  .provider-choice:focus-visible {
    background: var(--soft);
  }
  .provider-choice span {
    flex: 1;
  }
</style>
