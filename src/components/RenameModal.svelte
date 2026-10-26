<script lang="ts">
  import Icon from './Icon.svelte';
  import { dialogFocus } from '../lib/dialog-focus';

  export let name: string;
  export let updateName: (value: string) => void;
  export let submit: () => void;
  export let close: () => void;
</script>

<div class="modal-backdrop" role="presentation">
  <div
    class="modal"
    tabindex="-1"
    use:dialogFocus={{ escape: close }}
    role="dialog"
    aria-modal="true"
    aria-labelledby="rename-title"
  >
    <h2 id="rename-title">Renomear perfil</h2>
    <div class="form">
      <label>
        Novo nome
        <input
          value={name}
          oninput={(event) => updateName(event.currentTarget.value)}
          maxlength="80"
        />
      </label>
      <div class="row">
        <button class="button primary" onclick={submit}
          ><Icon name="circle-check" size={17} />Salvar</button
        >
        <button class="button" onclick={close}>Cancelar</button>
      </div>
    </div>
  </div>
</div>
