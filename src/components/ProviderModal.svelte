<script lang="ts">
  import type { MailProvider } from '../lib/types';
  import Icon from './Icon.svelte';

  export let provider: MailProvider | null;
  export let displayName: string;
  export let domains: string;
  export let url: string;
  export let aliases: string;
  export let updateDisplayName: (value: string) => void;
  export let updateDomains: (value: string) => void;
  export let updateUrl: (value: string) => void;
  export let updateAliases: (value: string) => void;
  export let submit: () => void;
  export let close: () => void;
</script>

<div class="modal-backdrop" role="presentation">
  <div class="modal" role="dialog" aria-modal="true" aria-labelledby="provider-title">
    <h2 id="provider-title">{provider ? 'Editar provedor' : 'Novo provedor personalizado'}</h2>
    <div class="form">
      <label>
        Nome
        <input
          value={displayName}
          oninput={(event) => updateDisplayName(event.currentTarget.value)}
          maxlength="80"
        />
      </label>
      <label>
        Domínios, separados por vírgula
        <input
          value={domains}
          oninput={(event) => updateDomains(event.currentTarget.value)}
          placeholder="empresa.com, empresa.com.br"
        />
      </label>
      <label>
        URL do webmail
        <input
          value={url}
          oninput={(event) => updateUrl(event.currentTarget.value)}
          placeholder="https://webmail.empresa.com"
        />
      </label>
      <details class="form-disclosure" open={Boolean(aliases)}>
        <summary><Icon name="settings" size={17} />Opções avançadas <Icon name="chevron-down" size={15} /></summary>
        <div class="disclosure-fields">
          <label>
            Aliases, separados por vírgula
            <input value={aliases} oninput={(event) => updateAliases(event.currentTarget.value)} />
          </label>
        </div>
      </details>
      <div class="row">
        <button class="button primary" onclick={submit}><Icon name="circle-check" size={17} />Salvar</button>
        <button class="button" onclick={close}>Cancelar</button>
      </div>
    </div>
  </div>
</div>
