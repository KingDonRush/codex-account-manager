<script lang="ts">
  import { recoveryChoices, type RecoveryAction } from '../lib/operation-recovery';
  import Icon from './Icon.svelte';
  export let code: string | null;
  export let canReplace = false;
  export let recover: (action: RecoveryAction) => void;
  export let busy = false;
  $: choices = recoveryChoices(code, canReplace, typeof navigator !== 'undefined' && /Windows/.test(navigator.userAgent));
</script>

<div class="row recovery-actions" aria-label="Corrigir operação">
  {#each choices as choice (choice.action)}
    <button class="button" disabled={busy} class:danger={choice.action === 'confirm_close'} onclick={() => recover(choice.action)}
      ><Icon name="arrow-right" size={16} />{choice.label}</button
    >
  {/each}
</div>
