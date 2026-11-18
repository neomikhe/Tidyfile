import js from "@eslint/js";
import { defineConfig } from "eslint/config";
import ts from "typescript-eslint";
import svelte from "eslint-plugin-svelte";
import globals from "globals";

export default defineConfig(
  js.configs.recommended,
  ...ts.configs.recommended,
  ...svelte.configs.recommended,
  {
    languageOptions: { globals: { ...globals.browser, ...globals.node } },
  },
  {
    files: ["**/*.svelte", "**/*.svelte.ts"],
    languageOptions: { parserOptions: { parser: ts.parser } },
  },
  {
    rules: {
      "@typescript-eslint/no-explicit-any": "error",
      "no-console": "error",
    },
  },
  {
    ignores: [
      "build/",
      ".svelte-kit/",
      "vitest.config.ts",
      "src-tauri/",
      "node_modules/",
      "static/",
      ".claude/",
      ".github/",
    ],
  },
);
