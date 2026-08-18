<script lang="ts">
  import { onMount } from "svelte";
  import { open } from "@tauri-apps/plugin-dialog";
  import { Workspace } from "$lib/state.svelte";
  import RuleEditor from "$lib/RuleEditor.svelte";
  import { blankRule, isIncomplete } from "$lib/factories";

  type View = "rules" | "activity" | "settings";

  const views: { id: View; label: string }[] = [
    { id: "rules", label: "Rules" },
    { id: "activity", label: "Activity" },
    { id: "settings", label: "Settings" },
  ];

  const workspace = new Workspace();
  let activeView = $state<View>("rules");
  let editingId = $state<string | null>(null);

  onMount(() => {
    void workspace.initialise();
    return () => workspace.dispose();
  });

  async function createRule(): Promise<void> {
    const rule = blankRule();
    editingId = rule.id;
    await workspace.add(rule);
  }

  async function closeEditor(): Promise<void> {
    editingId = null;
    await workspace.commit();
  }

  async function pickFolder(): Promise<void> {
    const picked = await open({ directory: true, multiple: false });
    if (typeof picked === "string") {
      await workspace.addFolder(picked);
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
            <li class="rule">
              <div class="summary-row">
                <label>
                  <input
                    type="checkbox"
                    checked={rule.enabled}
                    disabled={isIncomplete(rule)}
                    onchange={() => workspace.toggle(rule.id)}
                  />
                  <span class="name">{rule.name}</span>
                </label>
                <span class="detail">
                  {#if isIncomplete(rule)}
                    Needs a value before it can run
                  {:else}
                    {rule.conditions.length} condition{rule.conditions.length === 1 ? "" : "s"},
                    {rule.actions.length} action{rule.actions.length === 1 ? "" : "s"}
                  {/if}
                </span>
                <button
                  onclick={() => {
                    if (editingId === rule.id) {
                      void closeEditor();
                    } else {
                      editingId = rule.id;
                    }
                  }}
                >
                  {editingId === rule.id ? "Close" : "Edit"}
                </button>
              </div>

              {#if editingId === rule.id}
                <RuleEditor
                  {rule}
                  onchange={(edited) => workspace.edit(edited)}
                  onremove={() => {
                    editingId = null;
                    void workspace.remove(rule.id);
                  }}
                  ondone={closeEditor}
                />
              {/if}
            </li>
          {/each}
        </ul>
      {/if}

      <button onclick={createRule}>New rule</button>

      <h2>Preview</h2>
      {#if workspace.folders.length === 0}
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

      {#if workspace.unfinished.length > 0}
        <div class="unfinished" role="status">
          <p>
            <strong
              >{workspace.unfinished.length} operation{workspace.unfinished.length === 1
                ? ""
                : "s"} never finished.</strong
            >
            A previous run was interrupted before these were confirmed, so they may or may not have
            been applied. Check the files below before dismissing.
          </p>
          <ul>
            {#each workspace.unfinished as change, index (index)}
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
          <button onclick={() => workspace.acknowledgeUnfinished()}>Dismiss</button>
        </div>
      {/if}

      {#if workspace.history.length === 0}
        <p class="empty">Nothing has run yet.</p>
      {:else}
        <ul class="history">
          {#each workspace.history as entry (entry.batch)}
            <li class="batch">
              <div class="summary-row">
                <span class="when">{moment(entry.recordedAt)}</span>
                <span class="counts">
                  {entry.done} applied
                  {#if entry.skipped > 0}, {entry.skipped} left alone{/if}
                  {#if entry.failed > 0}, {entry.failed} failed{/if}
                  {#if entry.undone > 0}, {entry.undone} undone{/if}
                </span>
                <button
                  aria-expanded={workspace.expanded === entry.batch}
                  onclick={() => workspace.expand(entry.batch)}
                >
                  {workspace.expanded === entry.batch ? "Hide files" : "Show files"}
                </button>
                <button
                  disabled={entry.done === 0 || workspace.status.kind === "working"}
                  onclick={() => workspace.revert(entry.batch)}
                >
                  Undo all
                </button>
              </div>

              {#if workspace.expanded === entry.batch}
                <ul class="details">
                  {#each workspace.details as change (change.id)}
                    <li>
                      <span class="kind {change.kind}">{change.kind}</span>
                      <span class="from">{fileName(change.source)}</span>
                      {#if change.destination !== null}
                        <span class="arrow" aria-hidden="true">-&gt;</span>
                        <span class="to">{change.destination}</span>
                      {/if}
                      <span class="detail">{change.state}</span>
                      <button
                        disabled={!change.undoable || workspace.status.kind === "working"}
                        onclick={() => workspace.revertOne(change.id)}
                      >
                        Undo
                      </button>
                    </li>
                  {/each}
                </ul>
              {/if}
            </li>
          {/each}
        </ul>
      {/if}
    </section>
  {:else}
    <section aria-labelledby="settings-heading">
      <h2 id="settings-heading">Settings</h2>
      <h3>Watched folders</h3>
      {#if workspace.folders.length === 0}
        <p class="empty">None chosen yet.</p>
      {:else}
        <ul class="folders">
          {#each workspace.folders as watched (watched)}
            <li>
              <span class="folder">{watched}</span>
              <button onclick={() => workspace.removeFolder(watched)}>Remove</button>
            </li>
          {/each}
        </ul>
      {/if}
      <button onclick={pickFolder}>Add folder</button>
      <p class="note">
        System folders, drive roots and your whole home folder cannot be watched.
      </p>

      <h3>When a file with that name already exists</h3>
      <label class="field">
        <select
          value={workspace.onCollision}
          onchange={(event) =>
            workspace.setCollision(event.currentTarget.value as "suffix" | "skip")}
        >
          <option value="suffix">Keep both, adding a number</option>
          <option value="skip">Leave the file where it is</option>
        </select>
      </label>
      <p class="note">Either way, the file already at the destination is never replaced.</p>

      <h3>Watch continuously</h3>
      <label class="switch">
        <input
          type="checkbox"
          checked={workspace.watching}
          disabled={workspace.folders.length === 0 || workspace.status.kind === "working"}
          onchange={(event) => workspace.setWatching(event.currentTarget.checked)}
        />
        <span>Tidy new files as they arrive</span>
      </label>
      <p class="note">
        While this is on, files that appear in the folder are tidied by your enabled rules without
        asking. Everything still goes through the history, so any batch can be undone.
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

  li.rule,
  li.batch {
    flex-direction: column;
    align-items: stretch;
  }

  .folders li {
    justify-content: space-between;
  }

  .details {
    margin-top: 0.5rem;
    padding-left: 0.75rem;
    border-left: 2px solid;
  }

  .summary-row {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 0.6rem;
    justify-content: space-between;
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

  .unfinished {
    border: 1px solid;
    border-radius: 0.375rem;
    padding: 0.8rem;
    margin-block: 0.75rem 1.25rem;
    display: flex;
    flex-direction: column;
    gap: 0.6rem;
  }

  .unfinished > p {
    margin: 0;
    font-size: 0.9rem;
  }

  .unfinished button {
    align-self: flex-start;
  }

  .field select {
    padding: 0.35rem 0.5rem;
    border: 1px solid;
    border-radius: 0.25rem;
    background: none;
    color: inherit;
    font: inherit;
    font-size: 0.9rem;
  }

  .switch {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    cursor: pointer;
  }

  .summary {
    font-size: 0.9rem;
    margin-block: 0 0.75rem;
  }
</style>
