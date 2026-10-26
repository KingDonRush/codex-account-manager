<script lang="ts">
  import catalog from '../lib/icon-catalog.json';

  type IconName = keyof typeof catalog;

  export let name: IconName;
  export let size = 20;
  export let label: string | null = null;
  export let strokeWidth = 1.8;

  $: icon = catalog[name];
</script>

<span
  class="ui-icon"
  style:width={`${size}px`}
  style:height={`${size}px`}
  role={label ? 'img' : undefined}
  aria-label={label ?? undefined}
  aria-hidden={label ? undefined : 'true'}
>
  <svg
    viewBox="0 0 24 24"
    width={size}
    height={size}
    fill="none"
    stroke="currentColor"
    stroke-width={strokeWidth}
    stroke-linecap="round"
    stroke-linejoin="round"
  >
    {@html icon.markup}
  </svg>
</span>
