<script lang="ts">
  import { PLACEHOLDERS } from "../factories";
  import { t } from "../i18n/index.svelte";

  let { enabled, oninsert }: { enabled: boolean; oninsert: (token: string) => void } = $props();
</script>

<div class="row chips">
  <span class="muted">{t("editor.insert")}</span>
  {#each PLACEHOLDERS as placeholder (placeholder.token)}
    <button
      class="chip"
      disabled={!enabled}
      title={placeholder.token}
      onmousedown={(event) => event.preventDefault()}
      onclick={() => oninsert(placeholder.token)}
    >
      {t(placeholder.label)}
    </button>
  {/each}
</div>

<style>
  .chips {
    flex-wrap: wrap;
    gap: 6px;
    padding-left: 22px;
  }

  .chip {
    padding: 1px 9px;
    border: 0;
    border-radius: 9px;
    font-size: 11.5px;
    background: var(--accent-soft);
    color: var(--accent-soft-text);
  }
</style>
