<script lang="ts">
  import { onMount } from "svelte";
  import { Workspace } from "#lib/state.svelte.ts";
  import type { Rule } from "#lib/ipc.ts";
  import type { View } from "#lib/ui/view.ts";
  import Sidebar from "#lib/ui/Sidebar.svelte";
  import RulesView from "#lib/views/RulesView.svelte";
  import ActivityView from "#lib/views/ActivityView.svelte";
  import SettingsView from "#lib/views/SettingsView.svelte";
  import RuleSheet from "#lib/editor/RuleSheet.svelte";

  const workspace = new Workspace();
  let activeView = $state<View>("rules");
  let sheet = $state<{ rule: Rule | null } | null>(null);

  onMount(() => {
    void workspace.initialise();
    return () => workspace.dispose();
  });

  function show(view: View): void {
    activeView = view;
    workspace.dismissRefusal();
    if (view === "settings") {
      void workspace.refreshFolderStates();
    }
    if (view === "rules" && workspace.preview === null) {
      void workspace.refreshPreview();
    }
  }

  function openSheet(rule: Rule | null): void {
    sheet = { rule };
  }
</script>

<div class="app">
  <Sidebar active={activeView} watched={workspace.watched} onselect={show} />
  <main class="pane">
    {#if activeView === "rules"}
      <RulesView {workspace} onedit={openSheet} onsettings={() => show("settings")} />
    {:else if activeView === "activity"}
      <ActivityView {workspace} />
    {:else}
      <SettingsView {workspace} />
    {/if}
  </main>
</div>

{#if sheet !== null}
  <RuleSheet {workspace} rule={sheet.rule} onclose={() => (sheet = null)} />
{/if}

<style>
  :global(#svelte-announcer) {
    position: absolute;
    left: 0;
    top: 0;
    clip: rect(0 0 0 0);
    clip-path: inset(50%);
    overflow: hidden;
    white-space: nowrap;
    width: 1px;
    height: 1px;
  }

  .app {
    display: flex;
    height: 100vh;
  }

  .pane {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
  }
</style>
