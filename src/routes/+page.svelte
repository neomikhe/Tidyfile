<script lang="ts">
  import { onMount } from "svelte";
  import { open } from "@tauri-apps/plugin-dialog";
  import { Workspace } from "$lib/state.svelte";

  type View = "rules" | "activity" | "settings";

  const views: { id: View; label: string }[] = [
    { id: "rules", label: "Rules" },
    { id: "activity", label: "Activity" },
    { id: "settings", label: "Settings" },
  ];

  const workspace = new Workspace();
  let activeView = $state<View>("rules");

  onMount(() => {
    void workspace.initialise();
  });

  async function pickFolder(): Promise<void> {
    const picked = await open({ directory: true, multiple: false });
    if (typeof picked === "string") {
      await workspace.chooseFolder(picked);
    }
  }

  function fileName(path: string): string {
    const parts = path.split(/[\\/]/);
    return parts[parts.length - 1] ?? path;
  }

  function moment(seconds: number): string {
    return new Date(seconds * 1000).toLocaleString();
  }
</script>

<main>
  <header>
    <h1>Tidyfile</h1>
    <nav aria-label="Views">
      {#each views as view (view.id)}
        <button
          class:active={activeView === view.id}
          aria-current={activeView === view.id ? "page" : undefined}
          onclick={() => (activeView = view.id)}
        >
          {view.label}
        </button>
      {/each}
    </nav>
  </header>

  {#if workspace.status.kind === "problem"}
    <p class="problem" role="alert">{workspace.status.message}</p>
  {/if}

  {#if activeView === "rules"}
    <section aria-labelledby="rules-heading">
      <h2 id="rules-heading">Rules</h2>

      {#if workspace.rules.length === 0}
        <p class="empty">No rules yet. Rules you create are stored on this computer only.</p>
      {:else}
        <ul class="rules">
          {#each workspace.rules as rule (rule.id)}
            <li>
              <label>
                <input
                  type="checkbox"
                  checked={rule.enabled}
                  onchange={() => workspace.toggle(rule.id)}
                />
                <span class="name">{rule.name}</span>
              </label>
              <span class="detail">
                {rule.conditions.length} condition{rule.conditions.length === 1 ? "" : "s"},
                {rule.actions.length} action{rule.actions.length === 1 ? "" : "s"}
              </span>
            </li>
          {/each}
        </ul>
      {/if}

      <h2>Preview</h2>
      {#if workspace.folder === null}
        <p class="empty">Choose a folder in Settings to see what would change.</p>
      {:else if workspace.preview === null}
        <p class="empty">Nothing simulated yet.</p>
      {:else if workspace.preview.length === 0}
        <p class="empty">No file in this folder matches an enabled rule.</p>
      {:else}
        <p class="summary">
          {workspace.preview.length} file{workspace.preview.length === 1 ? "" : "s"} would change.
          Nothing has been touched yet.
        </p>
        <ul class="preview">
          {#each workspace.preview as change (change.source)}
            <li>
              <span class="kind {change.kind}">{change.kind}</span>
              <span class="from">{fileName(change.source)}</span>
              {#if change.destination !== null}
                <span class="arrow" aria-hidden="true">-&gt;</span>
                <span class="to">{change.destination}</span>
              {/if}
            </li>
          {/each}
        </ul>
      {/if}

      <button class="primary" disabled={!workspace.canRun} onclick={() => workspace.run()}>
        Tidy now
      </button>
    </section>
  {:else if activeView === "activity"}
    <section aria-labelledby="activity-heading">
      <h2 id="activity-heading">Activity</h2>
      {#if workspace.history.length === 0}
        <p class="empty">Nothing has run yet.</p>
      {:else}
        <ul class="history">
          {#each workspace.history as entry (entry.batch)}
            <li>
              <span class="when">{moment(entry.recordedAt)}</span>
              <span class="counts">
                {entry.done} applied
                {#if entry.failed > 0}, {entry.failed} failed{/if}
                {#if entry.undone > 0}, {entry.undone} undone{/if}
              </span>
              <button
                disabled={entry.done === 0 || workspace.status.kind === "working"}
                onclick={() => workspace.revert(entry.batch)}
              >
                Undo
              </button>
            </li>
          {/each}
        </ul>
      {/if}
    </section>
  {:else}
    <section aria-labelledby="settings-heading">
      <h2 id="settings-heading">Settings</h2>
      <h3>Watched folder</h3>
      <p class="folder">{workspace.folder ?? "None chosen"}</p>
      <button onclick={pickFolder}>Choose folder</button>
      <p class="note">
        System folders, drive roots and your whole home folder cannot be watched.
      </p>
    </section>
  {/if}
</main>

<style>
  main {
    font-family: system-ui, sans-serif;
    max-width: 52rem;
    margin: 0 auto;
    padding: 1.5rem 1rem 3rem;
    line-height: 1.5;
  }

  header {
    display: flex;
    flex-wrap: wrap;
    align-items: baseline;
    justify-content: space-between;
    gap: 1rem;
  }

  h1 {
    font-size: 1.5rem;
    margin: 0;
  }

  h2 {
    font-size: 1.05rem;
    margin-block: 2rem 0.75rem;
  }

  h3 {
    font-size: 0.95rem;
    margin-block: 1.25rem 0.5rem;
  }

  nav {
    display: flex;
    gap: 0.5rem;
  }

  button {
    padding: 0.4rem 0.9rem;
    border: 1px solid currentColor;
    border-radius: 0.375rem;
    background: none;
    color: inherit;
    font: inherit;
    cursor: pointer;
  }

  button:disabled {
    opacity: 0.45;
    cursor: not-allowed;
  }

  button.active,
  button.primary {
    background: CanvasText;
    color: Canvas;
  }

  button.primary {
    margin-top: 1.5rem;
  }

  ul {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 0.4rem;
  }

  li {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 0.6rem;
    padding: 0.5rem 0.7rem;
    border: 1px solid;
    border-radius: 0.375rem;
  }

  label {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    cursor: pointer;
  }

  .name {
    font-weight: 600;
  }

  .detail,
  .note,
  .empty,
  .when {
    opacity: 0.7;
    font-size: 0.9rem;
  }

  .empty,
  .note {
    margin-block: 0.5rem;
  }

  .kind {
    text-transform: uppercase;
    font-size: 0.7rem;
    letter-spacing: 0.04em;
    padding: 0.1rem 0.4rem;
    border: 1px solid;
    border-radius: 0.25rem;
  }

  .to,
  .folder {
    font-family: ui-monospace, monospace;
    font-size: 0.85rem;
    overflow-wrap: anywhere;
  }

  .history li,
  .preview li {
    justify-content: space-between;
  }

  .problem {
    border: 1px solid;
    border-radius: 0.375rem;
    padding: 0.6rem 0.8rem;
    margin-block: 1rem 0;
  }

  .summary {
    font-size: 0.9rem;
    margin-block: 0 0.75rem;
  }
</style>
