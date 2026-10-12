/**
 * Helpers for the two independent dictation bindings on a hotkey profile.
 * Keep the legacy `shortcut` field in the payload: older settings use it
 * when neither explicit binding is present, and the backend keeps it as the
 * compatibility/default value.
 */
import type { HotkeyMode, HotkeyProfile } from "./api";

/** The two explicit bindings a profile can carry. */
export type ProfileShortcutSlot = "push_to_talk_shortcut" | "toggle_shortcut";

/** The shortcut fields of a profile; suggestions only read these. */
export type ProfileShortcutFields = Pick<HotkeyProfile, "shortcut" | "push_to_talk_shortcut" | "toggle_shortcut">;

const MODIFIER_ALIASES: Record<string, string> = {
  control: "control",
  ctrl: "control",
  alt: "alt",
  option: "alt",
  super: "super",
  command: "super",
  cmd: "super",
  meta: "super",
  shift: "shift",
};

const KEY_ALIASES: Record<string, string> = {
  up: "arrowup",
  down: "arrowdown",
  left: "arrowleft",
  right: "arrowright",
  esc: "escape",
};

export function canonicalShortcut(shortcut: string | null | undefined, platform = "macos"): string {
  if (typeof shortcut !== "string") return "";
  const tokens = shortcut.split("+").map((token) => token.trim().toLowerCase()).filter(Boolean);
  if (tokens.length === 0) return "";
  const keyToken = tokens[tokens.length - 1];
  const modifiers = [...new Set(tokens.slice(0, -1).map((token) => {
    if (["commandorcontrol", "commandorctrl", "cmdorctrl", "cmdorcontrol"].includes(token)) {
      return platform === "macos" ? "super" : "control";
    }
    return MODIFIER_ALIASES[token] ?? token;
  }))].sort();
  const key = KEY_ALIASES[keyToken]
    ?? (keyToken.length === 1 && /^[a-z]$/.test(keyToken) ? `key${keyToken}` : null)
    ?? (keyToken.length === 1 && /^[0-9]$/.test(keyToken) ? `digit${keyToken}` : keyToken);
  return [...modifiers, key].join("+");
}

function nonEmpty(value: unknown): string {
  return typeof value === "string" && value.trim() ? value.trim() : "";
}

export function displayProfileShortcut(
  profile: HotkeyProfile,
  slot: ProfileShortcutSlot,
  hotkeyMode: HotkeyMode,
): string {
  const explicit = nonEmpty(profile[slot]);
  if (explicit) return explicit;
  const hasAnyExplicitBinding = nonEmpty(profile.push_to_talk_shortcut) || nonEmpty(profile.toggle_shortcut);
  if (hasAnyExplicitBinding) return "";
  // Legacy settings are shown in the slot selected by the old global mode,
  // but this read-only migration view must not manufacture explicit fields.
  if (hotkeyMode === slotPrefixToMode(slot)) {
    return nonEmpty(profile.shortcut);
  }
  return "";
}

function slotPrefixToMode(slot: ProfileShortcutSlot): HotkeyMode {
  return slot === "push_to_talk_shortcut" ? "push" : "toggle";
}

/**
 * Return the bindings that should be considered when checking registration
 * health. Explicit bindings replace legacy shortcut routing for that profile;
 * a profile with neither remains legacy-compatible.
 */
export function profileShortcutValues(profile: HotkeyProfile, hotkeyMode: HotkeyMode = "push"): string[] {
  const push = nonEmpty(profile.push_to_talk_shortcut);
  const toggle = nonEmpty(profile.toggle_shortcut);
  const legacy = nonEmpty(profile.shortcut);
  if (push || toggle) return [push, toggle].filter(Boolean);
  return legacy ? [legacy] : [];
}

export function allProfileShortcutValues(profiles: HotkeyProfile[], hotkeyMode: HotkeyMode = "push"): string[] {
  return profiles.flatMap((profile) => profileShortcutValues(profile, hotkeyMode));
}

export function profileWithShortcut(
  profile: HotkeyProfile,
  slot: ProfileShortcutSlot,
  value: string | null | undefined,
  hotkeyMode: HotkeyMode = "push",
): HotkeyProfile {
  const next: HotkeyProfile = {
    ...profile,
    push_to_talk_shortcut: nonEmpty(profile.push_to_talk_shortcut) || null,
    toggle_shortcut: nonEmpty(profile.toggle_shortcut) || null,
    [slot]: nonEmpty(value) || null,
  };
  const hasAnyExplicitBinding = nonEmpty(profile.push_to_talk_shortcut) || nonEmpty(profile.toggle_shortcut);
  const legacySlot: ProfileShortcutSlot = hotkeyMode === "toggle" ? "toggle_shortcut" : "push_to_talk_shortcut";
  if (!hasAnyExplicitBinding && nonEmpty(profile.shortcut) && legacySlot !== slot) {
    next[legacySlot] = profile.shortcut;
  }
  const push = nonEmpty(next.push_to_talk_shortcut);
  const toggle = nonEmpty(next.toggle_shortcut);
  // The backend retains this compatibility field and synchronizes it to the
  // first explicit binding. An empty value is intentionally left for backend
  // validation when the user clears both fields.
  next.shortcut = push || toggle || "";
  return next;
}

/**
 * Pick the first unused pair of friendly defaults. Existing customized
 * profile shortcuts, including legacy values, are treated as occupied.
 */
export function suggestedProfileShortcuts(
  profiles: readonly ProfileShortcutFields[],
  legacyShortcut: string,
  platform: string,
): { push_to_talk_shortcut: string | null; toggle_shortcut: string | null; shortcut: string } {
  const occupied = new Set([
    ...profiles.flatMap((profile) => [profile.shortcut, profile.push_to_talk_shortcut, profile.toggle_shortcut]),
    legacyShortcut,
  ].map((shortcut) => canonicalShortcut(shortcut, platform)).filter(Boolean));
  const pttCandidates = platform === "macos"
    ? ["Super+S", "Super+E"]
    : ["Control+S", "Control+E"];
  const toggleCandidates = platform === "macos"
    ? ["Shift+Super+S", "Shift+Super+E"]
    : ["Control+Shift+S", "Control+Shift+E"];
  const pick = (candidates: string[], reserved = new Set<string>()) => candidates.find((candidate) => {
    const canonical = canonicalShortcut(candidate, platform);
    return !occupied.has(canonical) && !reserved.has(canonical);
  }) ?? "";
  const ptt = pick(pttCandidates);
  const reserved = new Set(ptt ? [canonicalShortcut(ptt, platform)] : []);
  const toggle = pick(toggleCandidates, reserved);
  return {
    push_to_talk_shortcut: ptt || null,
    toggle_shortcut: toggle || null,
    shortcut: ptt || toggle,
  };
}
