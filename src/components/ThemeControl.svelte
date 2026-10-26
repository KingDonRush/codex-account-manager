<script lang="ts">
  import { onMount } from 'svelte';
  import { applyTheme, readTheme, saveTheme, type ThemePreference } from '../lib/theme';
  import Icon from './Icon.svelte';
  let preference = readTheme();
  const themes = [
    { value: 'light', label: 'Claro', icon: 'sun' },
    { value: 'dark', label: 'Escuro', icon: 'moon' },
    { value: 'system', label: 'Sistema', icon: 'monitor' },
  ] as const;

  function choose(value: ThemePreference): void { preference = value; saveTheme(value); }
  onMount(() => {
    const media = matchMedia('(prefers-color-scheme: dark)');
    const update = () => applyTheme(preference);
    media.addEventListener('change', update);
    return () => media.removeEventListener('change', update);
  });
</script>

<div class="theme-control" role="group" aria-label="Aparência">
  {#each themes as theme (theme.value)}
    <button title={theme.label} aria-label={`Tema ${theme.label.toLowerCase()}`} aria-pressed={preference === theme.value} onclick={() => choose(theme.value)}>
      <Icon name={theme.icon} size={17} />
    </button>
  {/each}
</div>
