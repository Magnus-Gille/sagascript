import type { Language } from "../api";

/** The name of a dictation language as the Settings window shows it. */
export function languageLabel(lang: Language): string {
  switch (lang) {
    case "sv":
      return "Swedish";
    case "no":
      return "Norwegian";
    case "fi":
      return "Finnish";
    case "da":
      return "Danish";
    case "en":
      return "English";
    default:
      return "Auto-detect";
  }
}
