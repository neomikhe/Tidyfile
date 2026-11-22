<script lang="ts">
  import { untrack } from "svelte";
  import { fromBytes, toBytes, type SizeUnit } from "../present";
  import { t } from "../i18n/index.svelte";

  let { bytes, label, onchange }: { bytes: number; label: string; onchange: (bytes: number) => void } = $props();

  const units: SizeUnit[] = ["KB", "MB", "GB"];
  // WHY: the unit is the user's choice; deriving it from the bytes snapped MB back to KB while the amount was still 0
  const start = untrack(() => fromBytes(bytes));
  let amount = $state(String(start.amount));
  let unit = $state<SizeUnit>(start.unit);

  function emit(): void {
    const value = Number(amount);
    onchange(toBytes(Number.isFinite(value) && value > 0 ? value : 0, unit));
  }
</script>

<input
  type="number"
  min="0"
  step="any"
  class="amount"
  aria-label={label}
  value={amount}
  oninput={(event) => {
    amount = event.currentTarget.value;
    emit();
  }}
/>
<select
  aria-label={t("editor.sizeUnit")}
  value={unit}
  onchange={(event) => {
    unit = event.currentTarget.value as SizeUnit;
    emit();
  }}
>
  {#each units as choice (choice)}
    <option value={choice}>{choice}</option>
  {/each}
</select>

<style>
  .amount {
    width: 80px;
  }
</style>
