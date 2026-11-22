<script lang="ts">
  let {
    checked,
    label,
    disabled = false,
    onchange,
  }: { checked: boolean; label: string; disabled?: boolean; onchange: (checked: boolean) => void } = $props();
</script>

<input
  type="checkbox"
  role="switch"
  class="switch"
  aria-label={label}
  title={label}
  {checked}
  {disabled}
  onchange={(event) => onchange(event.currentTarget.checked)}
/>

<style>
  .switch {
    appearance: none;
    position: relative;
    flex-shrink: 0;
    width: 32px;
    height: 19px;
    margin: 0;
    border-radius: 10px;
    background: var(--switch-off);
    cursor: pointer;
    transition: background 0.15s;
  }

  .switch::after {
    content: "";
    position: absolute;
    top: 2px;
    left: 2px;
    width: 15px;
    height: 15px;
    border-radius: 50%;
    background: #ffffff;
    box-shadow: 0 1px 2px rgba(0, 0, 0, 0.3);
    transition: transform 0.15s;
  }

  .switch:checked {
    background: var(--accent);
  }

  .switch:checked::after {
    transform: translateX(13px);
  }

  .switch:disabled {
    opacity: 0.45;
    cursor: not-allowed;
  }

  @media (prefers-reduced-motion: reduce) {
    .switch,
    .switch::after {
      transition: none;
    }
  }
</style>
