<script lang="ts">
  import type { Workspace } from "../state.svelte";
  import { t } from "../i18n/index.svelte";
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

{#if workspace.status.kind === "problem" && workspace.status.message !== workspace.loadProblem}
  <Banner kind="problem"><p>{workspace.status.message}</p></Banner>
{/if}
