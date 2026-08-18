<script lang="ts">
  type View = "rules" | "activity" | "settings";

  const views: { id: View; label: string }[] = [
    { id: "rules", label: "Rules" },
    { id: "activity", label: "Activity" },
    { id: "settings", label: "Settings" },
  ];

  let activeView = $state<View>("rules");
</script>

<main>
  <h1>Tidyfile</h1>

  <nav>
    {#each views as view (view.id)}
      <button
        class:active={activeView === view.id}
        onclick={() => (activeView = view.id)}
      >
        {view.label}
      </button>
    {/each}
  </nav>

  <section>
    <p>The {activeView} view is not implemented yet.</p>
  </section>
</main>

<style>
  main {
    font-family: system-ui, sans-serif;
    max-width: 48rem;
    margin: 0 auto;
    padding: 2rem 1rem;
  }

  nav {
    display: flex;
    gap: 0.5rem;
    margin-block: 1.5rem;
  }

  button {
    padding: 0.5rem 1rem;
    border: 1px solid currentColor;
    border-radius: 0.375rem;
    background: none;
    color: inherit;
    cursor: pointer;
  }

  button.active {
    background: CanvasText;
    color: Canvas;
  }

</style>
