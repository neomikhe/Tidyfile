<script lang="ts">
  import type { Snippet } from "svelte";

  let { kind, children }: { kind: "info" | "problem"; children: Snippet } = $props();
</script>

<div class="banner {kind}" role={kind === "problem" ? "alert" : "status"}>
  {@render children()}
</div>

<style>
  .banner {
    margin: 12px 0 0;
    padding: 9px 12px;
    border-radius: var(--radius-group);
    border: 1px solid var(--group-border);
    background: var(--accent-soft);
    color: var(--text);
  }

  .banner.problem {
    background: var(--danger-soft);
    border-color: var(--danger);
  }

  .banner :global(p) {
    margin: 0 0 6px;
  }

  .banner :global(p:last-child) {
    margin-bottom: 0;
  }
</style>
