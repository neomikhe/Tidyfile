<script lang="ts" module>
  export type IconName = "rules" | "activity" | "settings" | "folder" | "file" | "trash" | "plus" | "minus";
</script>

<script lang="ts">
  let { name }: { name: IconName } = $props();
</script>

<svg viewBox="0 0 16 16" aria-hidden="true" focusable="false">
  {#if name === "rules"}
    <path d="M3 4h10M3 8h10M3 12h6" />
  {:else if name === "activity"}
    <circle cx="8" cy="8" r="5.8" />
    <path d="M8 4.8V8l2.2 1.6" />
  {:else if name === "settings"}
    <circle cx="8" cy="8" r="2.2" />
    <path d="M8 1.8v2M8 12.2v2M1.8 8h2M12.2 8h2M3.6 3.6 5 5M11 11l1.4 1.4M3.6 12.4 5 11M11 5l1.4-1.4" />
  {:else if name === "folder"}
    <path d="M2 4.5h4l1.5 1.5H14v6.5H2z" />
  {:else if name === "file"}
    <path d="M4 2h5l3 3v9H4z" />
    <path d="M9 2v3h3" />
  {:else if name === "trash"}
    <path d="M3 4.5h10M6 4.5V3h4v1.5M4.5 4.5l.7 9h5.6l.7-9" />
  {:else if name === "plus"}
    <path d="M8 3.5v9M3.5 8h9" />
  {:else}
    <path d="M3.5 8h9" />
  {/if}
</svg>

<style>
  svg {
    width: 16px;
    height: 16px;
    flex-shrink: 0;
    fill: none;
    stroke: currentColor;
    stroke-width: 1.3;
    stroke-linecap: round;
    stroke-linejoin: round;
  }
</style>
