<script lang="ts">
  import type { RuntimeStatus, View } from '../lib/types';
  import Icon from './Icon.svelte';
  import ThemeControl from './ThemeControl.svelte';

  export let view: View;
  export let runtime: RuntimeStatus | null;
  export let selectView: (view: View) => void;

  const links = [
    { id: 'dashboard', label: 'Contas', icon: 'accounts' },
    { id: 'applications', label: 'Aplicações', icon: 'grid' },
    { id: 'providers', label: 'E-mail', icon: 'mail' },
    { id: 'diagnostics', label: 'Diagnóstico', icon: 'activity' },
  ] as const;
</script>

<aside class="sidebar">
  <div class="brand">
    <span class="brand-mark"><Icon name="sparkles" size={20} /></span>
    <span class="brand-copy"><strong>Codex</strong><small>Contas</small></span>
  </div>
  <nav class="nav" aria-label="Navegação principal">
    {#each links as link (link.id)}
      <button
        aria-label={link.label}
        title={link.label}
        class:active={view === link.id}
        onclick={() => selectView(link.id)}
      >
        <Icon name={link.icon} size={19} />
        <span class="nav-label">{link.label}</span>
      </button>
    {/each}
  </nav>
  <div class="sidebar-bottom"><ThemeControl /></div>
  {#if runtime}
    <div class="runtime-card">
      <span
        class:attention={runtime.runtimeKind !== 'managed' || runtime.repairRequired}
        class="runtime-dot"
      ></span>
      <div>
        <strong
          >{runtime.runtimeKind === 'managed'
            ? 'Chromium gerenciado'
            : 'Navegador do sistema'}</strong
        >
        <span>{runtime.repairRequired ? 'Reparo necessário' : 'Pronto para abrir perfis'}</span>
      </div>
    </div>
  {/if}
</aside>
