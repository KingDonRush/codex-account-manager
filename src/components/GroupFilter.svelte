<script lang="ts">
  import type { AccountGroup } from '../lib/integrations';
  import Icon from './Icon.svelte';
  export let groups: AccountGroup[];
  export let value = '';
  export let label: string;
  export let manage: () => void;
</script>

<div class="group-filter">
  <label
    ><Icon name="accounts" size={16} /><select aria-label={label} bind:value>
      <option value="">Todos os grupos</option>
      {#each groups.filter((group) => !group.archived) as group (group.id)}
        <option value={group.id}>{group.name}</option>
      {/each}
    </select></label
  >
  <button
    class="button small"
    onclick={manage}
    aria-label="Gerenciar grupos"
    title="Criar e editar grupos de contas"
    ><Icon name="settings" size={16} /><span>Grupos</span></button
  >
</div>

<style>
  .group-filter,
  label {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  label {
    color: var(--muted);
    margin: 0;
  }
  select {
    width: 170px;
    max-width: 100%;
    min-height: 35px;
    border: 1px solid var(--line);
    border-radius: 7px;
    background: var(--surface);
    color: var(--ink);
    padding: 6px 8px;
    font: inherit;
  }
  .button {
    white-space: nowrap;
  }
  @media (max-width: 740px) {
    select {
      width: 145px;
    }
  }
</style>
