<script lang="ts">
  import Icon, { type IconName } from "./Icon.svelte";
  import { t, type MessageKey } from "../i18n/index.svelte";
  import { watchingLabel } from "../present";
  import type { View } from "./view";

  let {
    active,
    watched,
    onselect,
  }: { active: View; watched: string[]; onselect: (view: View) => void } = $props();

  const items: { view: View; icon: IconName; label: MessageKey }[] = [
    { view: "rules", icon: "rules", label: "nav.rules" },
    { view: "activity", icon: "activity", label: "nav.activity" },
    { view: "settings", icon: "settings", label: "nav.settings" },
  ];

  const watching = $derived(watchingLabel(watched));
</script>

<nav class="sidebar" aria-label={t("nav.label")}>
  {#each items as item (item.view)}
    <button
      class="item"
      class:on={active === item.view}
      aria-current={active === item.view ? "page" : undefined}
      title={t(item.label)}
      onclick={() => onselect(item.view)}
    >
      <Icon name={item.icon} />
      <span class="label">{t(item.label)}</span>
    </button>
  {/each}
  <button class="watching" title={watching} onclick={() => onselect("settings")}>
    <span class="dot" class:off={watched.length === 0} aria-hidden="true"></span>
    <span class="label">{watching}</span>
  </button>
</nav>

<style>
  .sidebar {
    width: 200px;
    flex-shrink: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding: 12px 10px;
    background: var(--sidebar-bg);
    border-right: 1px solid var(--sidebar-border);
  }

  .item,
  .watching {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    padding: 6px 8px;
    border: 0;
    border-radius: var(--radius-control);
    background: none;
    text-align: left;
  }

  .item :global(svg) {
    color: var(--accent-text);
  }

  .item:hover:not(.on) {
    background: var(--neutral-soft);
  }

  .item.on {
    background: var(--accent);
    color: var(--on-accent);
  }

  .item.on :global(svg) {
    color: var(--on-accent);
  }

  .watching {
    margin-top: auto;
    font-size: 12px;
    color: var(--sidebar-text-secondary);
  }

  .label {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .dot {
    width: 8px;
    height: 8px;
    flex-shrink: 0;
    border-radius: 50%;
    background: #28c840;
    box-shadow: 0 0 0 3px rgba(40, 200, 64, 0.18);
  }

  .dot.off {
    background: var(--text-secondary);
    box-shadow: none;
  }

  @media (max-width: 899px) {
    .sidebar {
      width: 56px;
      padding: 12px 8px;
    }

    .item,
    .watching {
      justify-content: center;
      padding: 8px 0;
    }

    .label {
      position: absolute;
      width: 1px;
      height: 1px;
      overflow: hidden;
      clip-path: inset(50%);
      white-space: nowrap;
    }
  }
</style>
