<script lang="ts">
  import { untrack } from "svelte";
  import { extensionsToText, textToExtensions } from "../factories";
  import { t } from "../i18n/index.svelte";

  let { anyOf, onchange }: { anyOf: string[]; onchange: (anyOf: string[]) => void } = $props();

  // WHY: re-rendering from the parsed list would swallow the comma the user is typing
  let text = $state(untrack(() => extensionsToText(anyOf)));
</script>

<input
  class="grow"
  placeholder={t("editor.placeholderExtension")}
  aria-label={t("condition.extension")}
  value={text}
  oninput={(event) => {
    text = event.currentTarget.value;
    onchange(textToExtensions(text));
  }}
/>
