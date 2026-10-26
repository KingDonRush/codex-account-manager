<script lang="ts">
  import type { MailProvider } from '../lib/types';
  import Icon from './Icon.svelte';

  export let name: string;
  export let providerId: string;
  export let mailUrl: string;
  export let providers: MailProvider[];
  export let updateName: (value: string) => void;
  export let updateProviderId: (value: string) => void;
  export let updateMailUrl: (value: string) => void;
  export let submit: () => void;
  export let close: () => void;
</script>

<div class="modal-backdrop" role="presentation" onclick={(event) => event.target === event.currentTarget && close()}>
  <div class="modal compact-modal" role="dialog" aria-modal="true" aria-labelledby="create-title">
    <div class="modal-title-row">
      <span class="modal-icon"><Icon name="plus" size={22} /></span>
      <div>
        <h2 id="create-title">Adicionar conta</h2>
        <p class="subtitle">Comece pelo nome. O restante pode ser definido depois.</p>
      </div>
    </div>
    <div class="form">
      <label>
        Nome do perfil
        <input
          value={name}
          oninput={(event) => updateName(event.currentTarget.value)}
          maxlength="80"
        />
      </label>

      <details class="form-disclosure" open={Boolean(providerId || mailUrl)}>
        <summary><Icon name="mail" size={17} />Configurar e-mail agora <Icon name="chevron-down" size={15} /></summary>
        <div class="disclosure-fields">
          <p>Opcional. Depois do login, o provedor também pode ser reconhecido pelo endereço.</p>
          <label>
            Provedor de e-mail
            <select value={providerId} onchange={(event) => updateProviderId(event.currentTarget.value)}>
              <option value="">Detectar depois</option>
              {#each providers as provider (provider.id)}
                <option value={provider.id}>{provider.displayName}</option>
              {/each}
            </select>
          </label>
          <label>
            URL personalizada
            <input
              value={mailUrl}
              oninput={(event) => updateMailUrl(event.currentTarget.value)}
              placeholder="https://webmail.exemplo.com"
            />
          </label>
        </div>
      </details>

      <div class="row modal-actions">
        <button class="button primary" onclick={submit}><Icon name="plus" size={17} />Criar perfil</button>
        <button class="button" onclick={close}>Cancelar</button>
      </div>
    </div>
  </div>
</div>
