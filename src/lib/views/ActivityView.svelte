<script lang="ts">
  import type { ActivityEntry, PlannedChange } from "../ipc";
  import type { Workspace } from "../state.svelte";
  import { batchCounts, groupByDay, knownFolderLabel } from "../present";
  import { formatTime, t, tn, type MessageKey } from "../i18n/index.svelte";
  import Toolbar from "../ui/Toolbar.svelte";
  import Group from "../ui/Group.svelte";
  import Banner from "../ui/Banner.svelte";
  import Problems from "./Problems.svelte";

  let { workspace }: { workspace: Workspace } = $props();

  const groups = $derived(groupByDay(workspace.history, new Date()));
  const busy = $derived(workspace.status.kind === "working");
  const states: Record<string, MessageKey> = {
    done: "state.done",
    undone: "state.undone",
    failed: "state.failed",
    skipped: "state.skipped",
    planned: "state.planned",
  };
  const kinds: Record<string, MessageKey> = { move: "kind.move", copy: "kind.copy", trash: "kind.trash" };

  function origin(entry: ActivityEntry): string {
    if (entry.origin === "manual") return t("origin.manual");
    if (entry.origin !== "watch") return "";
    return entry.folder === null ? t("origin.watchUnknown") : t("origin.watch", { folder: knownFolderLabel(entry.folder) });
  }

  function fileName(path: string): string {
    const parts = path.split(/[\\/]/).filter((part) => part.length > 0);
    return parts[parts.length - 1] ?? path;
  }

  function kindLabel(change: PlannedChange): string {
    return t(kinds[change.kind] ?? "kind.move");
  }
</script>

<Toolbar title={t("activity.title")} subtitle={t("activity.subtitle")} />

<div class="content">
  <Problems {workspace} />

  {#if workspace.manualRestore > 0}
    <Banner kind="info"><p>{tn("notice.manualRestore", workspace.manualRestore)}</p></Banner>
  {/if}
  {#if workspace.notInTrash > 0}
    <Banner kind="info"><p>{tn("notice.notInTrash", workspace.notInTrash)}</p></Banner>
  {/if}
  {#if workspace.alreadyGone > 0}
    <Banner kind="info"><p>{tn("notice.alreadyGone", workspace.alreadyGone)}</p></Banner>
  {/if}
  {#if workspace.couldNotPutBack > 0}
    <Banner kind="info"><p>{tn("notice.couldNotPutBack", workspace.couldNotPutBack)}</p></Banner>
  {/if}

  {#if workspace.unfinished.length > 0}
    <Banner kind="problem">
      <p><strong>{tn("notice.unfinished", workspace.unfinished.length)}</strong> {t("notice.unfinishedBody")}</p>
      {#each workspace.unfinished as change, index (index)}
        <p class="unfinished-row">
          <span class="tag {change.kind}">{kindLabel(change)}</span>
          <bdi>{fileName(change.source)}</bdi>
          {#if change.destination !== null}<span class="muted">→ {change.destination}</span>{/if}
        </p>
      {/each}
      <p><button onclick={() => workspace.acknowledgeUnfinished()}>{t("notice.dismiss")}</button></p>
    </Banner>
  {/if}

  {#if workspace.history.length === 0}
    <p class="muted empty">{t("activity.empty")}</p>
  {/if}

  {#each groups as group (group.label)}
    <Group title={group.label}>
      {#each group.entries as entry (entry.batch)}
        {@const expanded = workspace.expanded === entry.batch}
        <div class="row">
          <span class="muted time">{formatTime(entry.recordedAt)}</span>
          <div class="grow">
            <div class="counts">{batchCounts(entry)}</div>
            {#if origin(entry) !== ""}
              <bdi class="muted">{origin(entry)}</bdi>
            {/if}
          </div>
          <button class="link" aria-expanded={expanded} onclick={() => workspace.expand(entry.batch)}>
            {t(expanded ? "activity.hide" : "activity.show")}
          </button>
          {#if entry.skipped > 0}
            <button disabled={busy} onclick={() => workspace.decideConflicts(entry.batch, true)}>{t("activity.keepBoth")}</button>
            <button disabled={busy} onclick={() => workspace.decideConflicts(entry.batch, false)}>{t("activity.leave")}</button>
          {/if}
          <button disabled={entry.done === 0 || busy} onclick={() => workspace.revert(entry.batch)}>
            {t("activity.undoAll")}
          </button>
        </div>
        {#if expanded}
          {#each workspace.details as change (change.id)}
            <div class="row sub">
              <bdi class="ellipsis file" title={change.source}>{fileName(change.source)}</bdi>
              <span class="muted" aria-hidden="true">→</span>
              <span class="grow muted ellipsis tail" title={change.destination ?? ""}>
                <bdi>{change.destination ?? t("kind.trash")}</bdi>
              </span>
              <span class="tag {change.state}">{t(states[change.state] ?? "state.planned")}</span>
              <button class="link" disabled={!change.undoable || busy} onclick={() => workspace.revertOne(change.id)}>
                {t("activity.undo")}
              </button>
            </div>
          {/each}
        {/if}
      {/each}
    </Group>
  {/each}
</div>

<style>
  .content {
    flex: 1;
    min-height: 0;
    overflow: auto;
    padding: 4px 22px 22px;
  }

  .time {
    width: 48px;
    flex-shrink: 0;
  }

  .counts {
    font-weight: 600;
  }

  .sub {
    padding-left: 34px;
    background: var(--content-bg);
  }

  .file {
    max-width: 40%;
  }

  .tail {
    direction: rtl;
    text-align: left;
  }

  .empty {
    margin: 18px 4px;
  }

  .unfinished-row {
    display: flex;
    gap: 8px;
    align-items: center;
  }
</style>
