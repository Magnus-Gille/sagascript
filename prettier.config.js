// Prettier (#361). Scope and exclusions: .prettierignore. Prettier's
// defaults already match the existing style (double quotes, semicolons,
// trailing commas, parenthesized arrow parameters); the print width is the
// one with the least churn without allowing much longer lines (changed
// lines when introduced: 80 → 18 906, 100 → 10 709, 120 → 7 146,
// 140 → 6 041, 160 → 5 861).
/** @type {import("prettier").Config} */
export default {
  printWidth: 140,
  plugins: ["prettier-plugin-svelte"],
  overrides: [{ files: "*.svelte", options: { parser: "svelte" } }],
};
