<script lang="ts">
  import { onDestroy, onMount, tick } from 'svelte';
  import type { ManagedProfile } from '../lib/types';
  import { accountMenuItems, type AccountMenuAction } from '../lib/account-menu';
  import { menuAssets } from '../lib/menu-assets';
  import Icon from './Icon.svelte';
  export let profile: ManagedProfile;
  export let point: { x: number; y: number };
  export let trigger: HTMLElement | null;
  export let close: () => void;
  export let select: (action: AccountMenuAction) => void;
  export let extended = false;
  let element: HTMLDivElement;
  let x = point.x;
  let y = point.y;
  $: items = accountMenuItems(profile, extended);

  function dismiss(restoreFocus = true): void {
    const target = trigger;
    close();
    if (restoreFocus)
      void tick().then(() => {
        if (target?.isConnected && !document.querySelector('[aria-modal="true"]'))
          target.focus({ preventScroll: true });
      });
  }
  function keys(event: KeyboardEvent): void {
    if (['PageUp', 'PageDown'].includes(event.key)) {
      dismiss(false);
      return;
    }
    if ((event.ctrlKey || event.metaKey) && ['f', 'k'].includes(event.key.toLowerCase())) {
      dismiss(false);
      return;
    }
    if (event.key === 'F2') {
      event.preventDefault();
      event.stopPropagation();
      activate('rename', false);
      return;
    }
    if (event.key === 'Escape') {
      event.preventDefault();
      event.stopPropagation();
      dismiss();
      return;
    }
    const buttons = Array.from(element.querySelectorAll<HTMLButtonElement>('[role="menuitem"]'));
    if (!['ArrowDown', 'ArrowUp', 'Home', 'End', 'Tab'].includes(event.key)) return;
    event.preventDefault();
    event.stopPropagation();
    if (event.key === 'Tab') {
      dismiss();
      return;
    }
    const current = buttons.findIndex((button) => button === document.activeElement);
    const next =
      event.key === 'Home'
        ? 0
        : event.key === 'End'
          ? buttons.length - 1
          : (current + (event.key === 'ArrowDown' ? 1 : -1) + buttons.length) % buttons.length;
    buttons[next]?.focus({ preventScroll: true });
  }
  function activate(action: AccountMenuAction, disabled: boolean | undefined): void {
    if (disabled) return;
    // Resolve the selected profile before close invalidates the parent's menu state.
    select(action);
    dismiss();
  }
  function scrollGesture(event: Event): void {
    if (event.target instanceof Node && element.contains(event.target)) return;
    dismiss(false);
  }
  onMount(() => {
    const bounds = element.getBoundingClientRect();
    x = Math.max(8, Math.min(point.x, window.innerWidth - bounds.width - 8));
    y = Math.max(8, Math.min(point.y, window.innerHeight - bounds.height - 8));
    // A fixed menu must not scroll the underlying account into a different position.
    element.querySelector<HTMLButtonElement>('[role="menuitem"]')?.focus({ preventScroll: true });
  });
  onDestroy(() => close());
</script>

<svelte:window
  onblur={() => dismiss(false)}
  onresize={() => dismiss()}
  onwheel={scrollGesture}
  ontouchmove={scrollGesture}
/>
<button
  class="menu-dismiss-layer"
  aria-label="Fechar menu de conta"
  tabindex="-1"
  onclick={() => dismiss()}
  oncontextmenu={(event) => {
    event.preventDefault();
    dismiss();
  }}
></button>
<div
  bind:this={element}
  class="account-context"
  role="menu"
  aria-label={`Ações de ${profile.name}`}
  tabindex="-1"
  style={`left:${x}px;top:${y}px`}
  onkeydown={keys}
>
  {#each items as item, index (item.action)}
    {#if index > 0 && items[index - 1]?.group !== item.group}<div
        class="menu-separator"
        role="separator"
      ></div>{/if}
    <button
      role="menuitem"
      tabindex="-1"
      aria-disabled={item.disabled ?? false}
      class:menu-danger={item.group === 'danger'}
      onclick={() => activate(item.action, item.disabled)}
    >
      {#if item.glyph === 'folder-open'}<Icon name="folder-open" size={18} />{:else}<img
          class="menu-glyph"
          src={menuAssets[item.glyph]}
          alt=""
          aria-hidden="true"
        />{/if}
      <span>{item.label}</span>
      {#if item.shortcut}<kbd>{item.shortcut}</kbd>{/if}
    </button>
  {/each}
</div>

<style>
  .menu-dismiss-layer {
    position: fixed;
    inset: 0;
    z-index: 79;
    border: 0;
    padding: 0;
    margin: 0;
    background: transparent;
    cursor: default;
  }
  .account-context {
    position: fixed;
    z-index: 80;
    min-width: 238px;
    max-width: calc(100vw - 16px);
    max-height: calc(100dvh - 16px);
    overflow-y: auto;
    display: flex;
    flex-direction: column;
    padding: 5px;
    background: var(--surface);
    color: var(--ink);
    border: 1px solid var(--line);
    border-radius: 9px;
    box-shadow: 0 12px 32px #0003;
  }
  .account-context button {
    display: grid;
    grid-template-columns: 18px minmax(0, 1fr) auto;
    align-items: center;
    gap: 10px;
    min-height: 34px;
    padding: 7px 9px;
    border: 0;
    border-radius: 5px;
    background: transparent;
    color: inherit;
    text-align: left;
    font-size: 12px;
  }
  .account-context button:hover,
  .account-context button:focus-visible {
    outline: 0;
    background: var(--soft);
  }
  .account-context button[aria-disabled='true'] {
    color: var(--muted);
  }
  .account-context .menu-danger {
    color: var(--danger);
  }
  .account-context .menu-danger:hover,
  .account-context .menu-danger:focus-visible {
    background: var(--danger-soft);
  }
  .menu-glyph {
    width: 18px;
    height: 18px;
    object-fit: contain;
  }
  :global(:root[data-theme='dark']) .menu-glyph {
    filter: brightness(1.65) saturate(0.7);
  }
  .menu-separator {
    height: 1px;
    background: var(--line);
    margin: 4px 6px;
  }
  kbd {
    color: var(--muted);
    font: inherit;
    font-size: 10px;
  }
</style>
