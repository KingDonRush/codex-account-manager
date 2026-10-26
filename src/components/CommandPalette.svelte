<script lang="ts">
  import { onMount } from 'svelte';
  import { buildSearchIndex, searchIndex } from '../lib/search';
  import type { PaletteCommand } from '../lib/command-palette';
  import Icon from './Icon.svelte';
  export let commands: PaletteCommand[];
  export let close: () => void;
  let field: HTMLInputElement;
  let query = '';
  let selected = 0;
  $: index = buildSearchIndex(commands, (command) => [
    command.label,
    command.detail,
    ...command.keywords,
  ]);
  $: matches = searchIndex(index, query).slice(0, 12);
  $: if (query !== undefined) selected = 0;
  onMount(() => {
    field.focus();
  });
  function perform(command: PaletteCommand): void {
    close();
    command.perform();
  }
  function keys(event: KeyboardEvent): void {
    if (event.key === 'Escape') {
      event.preventDefault();
      close();
    }
    if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
      event.preventDefault();
      selected = Math.max(
        0,
        Math.min(matches.length - 1, selected + (event.key === 'ArrowDown' ? 1 : -1)),
      );
    }
    const command = matches[selected];
    if (event.key === 'Enter' && command) {
      event.preventDefault();
      perform(command);
    }
  }
</script>

<div class="modal-backdrop" role="presentation">
  <div
    class="modal command-palette"
    role="dialog"
    aria-modal="true"
    aria-labelledby="palette-title"
  >
    <div class="modal-title-row">
      <Icon name="search" size={23} />
      <h2 id="palette-title">O que você quer fazer?</h2>
      <button class="menu-trigger" aria-label="Fechar comandos" onclick={close}
        ><Icon name="circle-x" size={18} /></button
      >
    </div>
    <input
      bind:this={field}
      aria-label="Buscar comandos e contas"
      bind:value={query}
      onkeydown={keys}
      placeholder="Conta, grupo, diagnóstico…"
    />
    <div class="palette-results" aria-label="Comandos encontrados">
      {#each matches as command, position (command.id)}<button
          class:highlighted={position === selected}
          onclick={() => perform(command)}
          ><Icon name="arrow-right" size={17} /><span
            >{command.label}<small>{command.detail}</small></span
          ></button
        >{/each}
      {#if !matches.length}<p class="form-hint">Nenhum comando corresponde à busca.</p>{/if}
    </div>
    <p class="form-hint">
      ↑ ↓ selecionar · Enter abrir · Esc fechar. Ações sensíveis mantêm suas confirmações.
    </p>
  </div>
</div>

<style>
  .command-palette {
    max-width: 620px;
  }
  .palette-results {
    margin-top: 14px;
    max-height: 360px;
    overflow: auto;
  }
  .palette-results button {
    display: flex;
    align-items: center;
    gap: 12px;
    width: 100%;
    padding: 12px;
    text-align: left;
    color: var(--ink);
    background: transparent;
    border: 0;
    border-radius: 10px;
  }
  .palette-results button.highlighted {
    background: var(--accent-soft);
  }
  .palette-results small {
    display: block;
    color: var(--muted);
    margin-top: 3px;
  }
</style>
