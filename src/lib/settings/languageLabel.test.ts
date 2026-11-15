import { expect, test } from "vitest";
import type { Language } from "../api";
import { languageLabel } from "./languageLabel";

test("every dictation language has its English name, and Auto is Auto-detect", () => {
  const names: Record<Language, string> = {
    en: "English",
    sv: "Swedish",
    no: "Norwegian",
    fi: "Finnish",
    da: "Danish",
    auto: "Auto-detect",
  };
  for (const [language, name] of Object.entries(names)) expect(languageLabel(language as Language)).toBe(name);
});
