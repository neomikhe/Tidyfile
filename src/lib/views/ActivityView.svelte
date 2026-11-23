<script lang="ts">
  import type { ActivityEntry, FailureReason, PlannedChange, RecordedChange } from "../ipc";
  import type { Workspace } from "../state.svelte";
  import { batchCounts, fileName, groupByDay, knownFolderLabel, showable } from "../present";
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

  function kindLabel(change: PlannedChange): string {
    return t(kinds[change.kind] ?? "kind.move");
  }

  const reasons: Record<FailureReason, { label: MessageKey; hint: MessageKey }> = {
    noRecycleBin: { label: "reason.noRecycleBin", hint: "reason.noRecycleBinHint" },
    tooBigForRecycleBin: { label: "reason.tooBigForRecycleBin", hint: "reason.tooBigForRecycleBinHint" },
  };

  function stateLabel(change: RecordedChange): string {
    return t(states[change.state] ?? "state.planned");
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
  {#if workspace.notRecycled > 0}
    <Banner kind="info"><p>{tn("notice.notRecycled", workspace.notRecycled)}</p></Banner>
  {/if}

  {#if workspace.unfinished.length > 0}
    <Banner kind="problem">
      <p><strong>{tn("notice.unfinished", workspace.unfinished.length)}</strong> {t("notice.unfinishedBody")}</p>
      {#each workspace.unfinished as change, index (index)}
        <p class="unfinished-row">
          <span class="tag {change.kind}">{kindLabel(change)}</span>
          <bdi>{fileName(change.source)}</bdi>
          {#if change.destination !== null}<span class="muted">→ <bdi>{showable(change.destination)}</bdi></span>{/if}
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
              <bdi class="ellipsis file" title={showable(change.source)}>{fileName(change.source)}</bdi>
              <span class="muted" aria-hidden="true">→</span>
              <span class="grow muted ellipsis tail" title={showable(change.destination ?? "")}>
                <bdi>{change.destination === null ? t("kind.trash") : showable(change.destination)}</bdi>
              </span>
              <span class="tag {change.state}">{stateLabel(change)}</span>
              {#if change.reason !== null}
                {@const reason = reasons[change.reason]}
                <span class="tag failed" title={t(reason.hint)}>{t(reason.label)}</span>
                <span class="visually-hidden">{t(reason.hint)}</span>
              {/if}
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
    min-width: 64px;
    flex-shrink: 0;
    white-space: nowrap;
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
