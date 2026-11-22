<script lang="ts">
  import type { Snippet } from "svelte";

  let { title, subtitle, children }: { title: string; subtitle?: string; children?: Snippet } = $props();
</script>

<header class="toolbar">
  <h1>{title}</h1>
  {#if subtitle}
    <span class="muted subtitle">{subtitle}</span>
  {/if}
  <div class="actions">
    {@render children?.()}
  </div>
</header>

<style>
  .toolbar {
    height: 52px;
    flex-shrink: 0;
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 0 18px;
    background: var(--toolbar-bg);
    border-bottom: 1px solid var(--toolbar-border);
  }

  h1 {
    margin: 0;
    font-size: 15px;
    font-weight: 600;
  }

  .subtitle {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .actions {
    margin-left: auto;
    display: flex;
    gap: 8px;
  }
</style>
