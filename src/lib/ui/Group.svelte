<script lang="ts">
  import type { Snippet } from "svelte";

  let { title, note, children }: { title?: string; note?: string; children: Snippet } = $props();
</script>

<section class="block">
  {#if title}
    <h2>{title}</h2>
  {/if}
  <div class="group">
    {@render children()}
  </div>
  {#if note}
    <p class="note">{note}</p>
  {/if}
</section>

<style>
  .block {
    margin-bottom: 6px;
  }

  h2 {
    margin: 14px 4px 6px;
    font-size: 13px;
    font-weight: 600;
  }

  .group {
    background: var(--group-bg);
    border: 1px solid var(--group-border);
    border-radius: var(--radius-group);
    overflow: hidden;
  }

  .note {
    margin: 6px 4px 0;
    font-size: 12px;
    color: var(--text-secondary);
  }
</style>
