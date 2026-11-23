<script lang="ts">
  import type { Workspace } from "../state.svelte";
  import { t, tn } from "../i18n/index.svelte";
  import { showable } from "../present";
  import Banner from "../ui/Banner.svelte";

  let { workspace }: { workspace: Workspace } = $props();
</script>

{#if workspace.loadProblem !== null}
  <Banner kind="problem">
    <p>
      {workspace.loadProblem}
      {#if workspace.savesBlocked}
        {t("load.savesBlocked")}
      {/if}
    </p>
  </Banner>
{/if}

{#if workspace.problem !== null && workspace.problem !== workspace.loadProblem}
  <Banner kind="problem"><p>{workspace.problem}</p></Banner>
{/if}

{#if workspace.bouncing.length > 0}
  <Banner kind="problem">
    <p>{tn("loop.stopped", workspace.bouncing.length, { files: workspace.bouncing.map(showable).join(", ") })}</p>
    <p><button onclick={() => workspace.dismissBouncing()}>{t("notice.dismiss")}</button></p>
  </Banner>
{/if}
