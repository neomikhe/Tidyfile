<script lang="ts">
  import type { Snippet } from "svelte";
  import "../lib/ui/theme.css";

  let { children }: { children: Snippet } = $props();
</script>

{@render children()}
