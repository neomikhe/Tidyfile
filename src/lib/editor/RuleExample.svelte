<script lang="ts">
  import type { RuleExample } from "../ipc";
  import { t } from "../i18n/index.svelte";

  let { example }: { example: RuleExample } = $props();

  function fileName(path: string): string {
    const parts = path.split(/[\\/]/).filter((part) => part.length > 0);
    return parts[parts.length - 1] ?? path;
  }
</script>

<div class="example" role="status">
  {#if example.change !== null}
    <span>{t("example.label")}</span>
    <bdi class="strong">{fileName(example.change.source)}</bdi>
    {#if example.change.kind === "trash"}
      <span>{t("example.trash")}</span>
    {:else}
      <span>{t(example.change.kind === "copy" ? "example.copy" : "example.move")}</span>
      <bdi class="strong path">{example.change.destination}</bdi>
    {/if}
  {:else if example.takenBy !== null}
    <span>{t("example.taken", { name: example.takenBy })}</span>
  {:else}
    <span>{t("example.none")}</span>
  {/if}
</div>

<style>
  .example {
    margin-top: 12px;
    padding: 9px 12px;
    border-radius: var(--radius-group);
    background: var(--accent-soft);
    border: 1px solid var(--group-border);
    overflow-wrap: anywhere;
  }

  .strong {
    font-weight: 600;
    color: var(--accent-soft-text);
  }
</style>
