<script lang="ts">
  import type { AccountPolicy } from '../lib/integrations';
  import Icon from './Icon.svelte';
  export let policy: AccountPolicy;
  export let capacity = 1;
  export let disabled = false;
  const choices = [
    {
      value: 'desktop_only',
      label: 'Só Codex',
      note: 'Aplicações não usam esta conta.',
      icon: 'monitor',
    },
    {
      value: 'shared',
      label: 'Codex e aplicações',
      note: 'Mantém o Codex disponível; a quota é compartilhada.',
      icon: 'accounts',
    },
    {
      value: 'applications_only',
      label: 'Só aplicações',
      note: 'Reserva a conta. Usar no Codex pedirá confirmação.',
      icon: 'grid',
    },
  ] as const;
</script>

<fieldset class="usage-choices" {disabled}>
  <legend>Onde a conta pode ser usada</legend>
  {#each choices as choice}
    <label class:chosen={policy === choice.value}>
      <input type="radio" name="account-policy" value={choice.value} bind:group={policy} />
      <Icon name={choice.icon} size={18} />
      <span><strong>{choice.label}</strong><small>{choice.note}</small></span>
    </label>
  {/each}
</fieldset>
<details class="form-disclosure capacity-settings">
  <summary><Icon name="settings" size={16} />Limites avançados</summary>
  <label
    >Trabalhos simultâneos de aplicações<input
      type="number"
      min="1"
      max="32"
      step="1"
      bind:value={capacity}
      {disabled}
    /></label
  >
  <p class="form-hint">
    Capacidade por conta, compartilhada entre grupos. Não cria quota extra; renovação da mesma
    sessão continua serializada.
  </p>
</details>

<style>
  fieldset {
    border: 0;
    padding: 0;
    margin: 0;
    min-width: 0;
  }
  legend {
    font-size: 13px;
    font-weight: 600;
    padding: 0 0 10px;
  }
  .usage-choices label {
    display: flex;
    align-items: center;
    gap: 11px;
    padding: 12px;
    margin: 7px 0;
    border: 1px solid var(--line);
    border-radius: 9px;
    background: var(--surface);
    cursor: pointer;
  }
  .usage-choices label:hover {
    background: var(--soft);
  }
  .usage-choices label.chosen {
    border-color: var(--accent);
    background: var(--accent-soft);
  }
  input[type='radio'] {
    width: 15px;
    height: 15px;
    min-height: 0;
    margin: 0;
    padding: 0;
    flex: none;
    accent-color: var(--accent);
  }
  span {
    display: grid;
    gap: 4px;
    min-width: 0;
  }
  strong {
    font-size: 12px;
  }
  small {
    font-size: 11px;
    color: var(--muted);
    line-height: 1.45;
  }
  .capacity-settings {
    margin: 16px 0 0;
  }
</style>
