<script lang="ts">
  import type { PlannedChange, Rule } from "../ipc";
  import type { Workspace } from "../state.svelte";
  import { isIncomplete } from "../factories";
  import { describeRule } from "../present";
  import { t, tn, type MessageKey } from "../i18n/index.svelte";
  import Toolbar from "../ui/Toolbar.svelte";
  import Group from "../ui/Group.svelte";
  import Switch from "../ui/Switch.svelte";
  import Banner from "../ui/Banner.svelte";
  import Problems from "./Problems.svelte";

  let {
    workspace,
    onedit,
    onsettings,
  }: { workspace: Workspace; onedit: (rule: Rule | null) => void; onsettings: () => void } = $props();

  const blocker = $derived(tidyBlocker());
  const kinds: Record<string, MessageKey> = { move: "kind.move", copy: "kind.copy", trash: "kind.trash" };

  function tidyBlocker(): MessageKey | null {
    if (workspace.folders.length === 0) return "rules.tidyNoFolders";
    if (workspace.enabledRules.length === 0) return "rules.tidyNoRules";
    if (workspace.status.kind === "working") return "rules.tidyWorking";
    return workspace.preview === null ? "rules.tidyNoPreview" : null;
  }

  function fileName(path: string): string {
    const parts = path.split(/[\\/]/).filter((part) => part.length > 0);
    return parts[parts.length - 1] ?? path;
  }

  function kindLabel(change: PlannedChange): string {
    return t(kinds[change.kind] ?? "kind.move");
  }
</script>

<Toolbar title={t("rules.title")}>
  <button onclick={() => onedit(null)}>{t("rules.new")}</button>
  <button
    class="primary"
    disabled={!workspace.canRun}
    title={blocker === null ? undefined : t(blocker)}
    onclick={() => workspace.run()}
  >
    {t("rules.tidy")}
  </button>
</Toolbar>

<div class="content">
  <Problems {workspace} />

  {#if workspace.deactivatedRule !== null}
    <Banner kind="info"><p>{t("rules.deactivated", { name: workspace.deactivatedRule })}</p></Banner>
  {/if}
  {#if workspace.previewChanged}
    <Banner kind="info"><p>{t("run.changed")}</p></Banner>
  {/if}
  {#if workspace.lastRun !== null}
    {@const run = workspace.lastRun}
    <Banner kind="info">
      <p>
        {#if run.applied === 0 && run.skipped === 0 && run.failed === 0}
          {t("run.nothing")}
        {:else}
          {tn("run.tidied", run.applied)}
          {#if run.skipped > 0}{tn("run.skipped", run.skipped)}{/if}
          {#if run.failed > 0}{tn("run.failed", run.failed)}{/if}
        {/if}
      </p>
    </Banner>
  {/if}

  {#if workspace.rules.length === 0}
    <div class="empty">
      <p class="muted">{t("rules.empty")}</p>
      <button class="primary" onclick={() => onedit(null)}>{t("rules.createFirst")}</button>
    </div>
  {:else}
    <Group title={t("rules.group")}>
      {#each workspace.rules as rule (rule.id)}
        {@const summary = describeRule(rule)}
        <div class="row">
          <div class="grow">
            <div class="name-line">
              <bdi class="name ellipsis">{rule.name}</bdi>
              {#if isIncomplete(rule)}
                <span class="tag skipped">{t("rules.incomplete")}</span>
              {/if}
            </div>
            <div class="muted ellipsis" title={summary}>{summary}</div>
          </div>
          <button class="link" aria-label={t("rules.editLabel", { name: rule.name })} onclick={() => onedit(rule)}>
            {t("rules.edit")}
          </button>
          <Switch
            checked={rule.enabled}
            label={t("rules.enableLabel", { name: rule.name })}
            disabled={isIncomplete(rule) || workspace.savesBlocked}
            onchange={() => workspace.toggle(rule.id)}
          />
        </div>
      {/each}
    </Group>
  {/if}

  {#if workspace.folders.length === 0}
    <Group title={t("preview.titleEmpty")}>
      <div class="row">
        <span class="grow muted">{t("preview.noFolders")}</span>
        <button onclick={onsettings}>{t("preview.goSettings")}</button>
      </div>
    </Group>
  {:else if workspace.preview === null}
    <Group title={t("preview.titleEmpty")}>
      <div class="row">
        <span class="grow muted">{t("preview.notYet")}</span>
        <button disabled={workspace.status.kind === "working"} onclick={() => workspace.refreshPreview()}>
          {t("preview.checkAgain")}
        </button>
      </div>
    </Group>
  {:else if workspace.preview.length === 0}
    <Group title={t("preview.titleEmpty")}>
      <div class="row"><span class="muted">{t("preview.noMatches")}</span></div>
    </Group>
  {:else}
    <Group title={tn("preview.title", workspace.preview.length)} note={t("preview.untouched")}>
      {#each workspace.preview as change, index (index)}
        <div class="row">
          <span class="tag {change.kind}">{kindLabel(change)}</span>
          <bdi class="file ellipsis" title={change.source}>{fileName(change.source)}</bdi>
          {#if change.destination !== null}
            <span class="muted" aria-hidden="true">→</span>
            <span class="muted ellipsis tail" title={change.destination}><bdi>{change.destination}</bdi></span>
          {/if}
        </div>
      {/each}
    </Group>
  {/if}
</div>

<style>
  .content {
    flex: 1;
    min-height: 0;
    overflow: auto;
    padding: 4px 22px 22px;
  }

  .name-line {
    display: flex;
    align-items: center;
    gap: 8px;
    min-width: 0;
  }

  .name {
    font-weight: 600;
  }

  .file {
    flex: 0 1 auto;
    max-width: 40%;
  }

  .tail {
    flex: 1;
    direction: rtl;
    text-align: right;
  }

  .empty {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 8px;
    margin: 18px 4px;
  }
</style>
