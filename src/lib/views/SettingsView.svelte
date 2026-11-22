<script lang="ts">
  import { open } from "@tauri-apps/plugin-dialog";
  import type { Collision, Language } from "../ipc";
  import type { Workspace } from "../state.svelte";
  import { knownFolderLabel } from "../present";
  import { t, type MessageKey } from "../i18n/index.svelte";
  import Toolbar from "../ui/Toolbar.svelte";
  import Group from "../ui/Group.svelte";
  import Switch from "../ui/Switch.svelte";
  import Icon from "../ui/Icon.svelte";
  import Banner from "../ui/Banner.svelte";
  import Problems from "./Problems.svelte";

  let { workspace }: { workspace: Workspace } = $props();

  const collisions: { value: Collision; label: MessageKey }[] = [
    { value: "suffix", label: "collision.suffix" },
    { value: "skip", label: "collision.skip" },
    { value: "ask", label: "collision.ask" },
  ];
  const languages: { value: Language; label: MessageKey }[] = [
    { value: "auto", label: "language.auto" },
    { value: "es", label: "language.es" },
    { value: "en", label: "language.en" },
  ];

  async function pickFolder(): Promise<void> {
    const picked = await open({ directory: true, multiple: false });
    if (typeof picked === "string") {
      await workspace.addFolder(picked);
    }
  }
</script>

<Toolbar title={t("settings.title")} />

<div class="content">
  <Problems {workspace} />

  <Group title={t("settings.folders")} note={t("settings.forbiddenNote")}>
    {#each workspace.folders as folder (folder)}
      {@const name = knownFolderLabel(folder)}
      {@const state = workspace.stateOf(folder)}
      <div class="row">
        <Icon name="folder" />
        <div class="grow">
          <bdi class="name">{name}</bdi>
          {#if state === "unavailable"}
            <span class="tag skipped">{t("settings.unavailable")}</span>
          {:else if state === "forbidden"}
            <span class="tag failed">{t("settings.forbidden")}</span>
          {/if}
          <div class="muted ellipsis" title={folder}><bdi>{folder}</bdi></div>
        </div>
        <span class="muted">{t("settings.watch")}</span>
        <Switch
          checked={workspace.isWatched(folder)}
          label={t("settings.watchLabel", { folder: name })}
          disabled={workspace.status.kind === "working" || state === "forbidden"}
          onchange={(checked) => workspace.setWatched(folder, checked)}
        />
        <button class="link" aria-label={t("settings.removeLabel", { folder: name })} onclick={() => workspace.removeFolder(folder)}>
          {t("settings.remove")}
        </button>
      </div>
    {/each}
    {#if workspace.folders.length === 0}
      <div class="row"><span class="muted">{t("settings.noFolders")}</span></div>
    {/if}
    <div class="row"><button onclick={pickFolder}>{t("settings.add")}</button></div>
  </Group>

  {#if workspace.refusedFolder !== null}
    <Banner kind="problem">
      <p><bdi class="name">{workspace.refusedFolder}</bdi> {t("settings.refusedRest")}</p>
    </Banner>
  {/if}
  <p class="note">{t("settings.watchNote")}</p>

  <Group title={t("settings.collision")} note={t("settings.collisionNote")}>
    <label class="row">
      <span class="grow">{t("settings.collisionLabel")}</span>
      <select value={workspace.onCollision} onchange={(event) => workspace.setCollision(event.currentTarget.value as Collision)}>
        {#each collisions as choice (choice.value)}
          <option value={choice.value}>{t(choice.label)}</option>
        {/each}
      </select>
    </label>
  </Group>

  <Group title={t("settings.language")}>
    <label class="row">
      <span class="grow">{t("settings.languageLabel")}</span>
      <select value={workspace.language} onchange={(event) => workspace.setLanguage(event.currentTarget.value as Language)}>
        {#each languages as choice (choice.value)}
          <option value={choice.value}>{t(choice.label)}</option>
        {/each}
      </select>
    </label>
  </Group>
</div>

<style>
  .content {
    flex: 1;
    min-height: 0;
    overflow: auto;
    padding: 4px 22px 22px;
  }

  .name {
    font-weight: 600;
  }

  .note {
    margin: 6px 4px 0;
    font-size: 12px;
    color: var(--text-secondary);
  }

  .row :global(svg) {
    color: var(--accent-text);
  }
</style>
