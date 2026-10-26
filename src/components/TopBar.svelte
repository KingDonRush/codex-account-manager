<script lang="ts">
  import type { View } from '../lib/types';
  import { viewTitle } from '../lib/view';
  import Icon from './Icon.svelte';

  export let view: View;
  export let createProfile: () => void;

  const subtitles: Record<View, string> = {
    dashboard: 'Veja o saldo restante e escolha qual conta o Codex deve usar.',
    applications: 'Contas e acesso para os aplicativos que você autorizar.',
    providers: 'Defina para onde cada endereço de e-mail deve abrir.',
    diagnostics: 'Acompanhe conexões, falhas e atualizações do aplicativo.',
  };
</script>

<header class="topbar">
  <div>
    <span class="page-kicker">Gerenciador de contas</span>
    <h1>{viewTitle(view)}</h1>
    <p class="subtitle">{subtitles[view]}</p>
  </div>
  {#if view === 'dashboard'}
    <button class="button primary add-account" onclick={createProfile}>
      <Icon name="plus" size={18} />Adicionar conta
    </button>
  {/if}
</header>
