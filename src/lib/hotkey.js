/**
 * Map a DOM KeyboardEvent key value to Tauri global-shortcut format.
 *
 * @param {string} key
 * @returns {string | null}
 */
export function tauriKeyName(key) {
  if (key === " ") return "Space";
  if (["Meta", "Control", "Alt", "Shift"].includes(key)) return null;

  const functionKey = /^F(\d{1,2})$/i.exec(key.trim());
  if (functionKey) {
    const number = Number.parseInt(functionKey[1], 10);
    return number >= 1 && number <= 24 ? `F${number}` : null;
  }

  if (/^[a-zA-Z0-9]$/.test(key)) return key.toUpperCase();
  if (key.startsWith("Arrow")) return key;

  /** @type {Record<string, string>} */
  const mapped = {
    Tab: "Tab",
    Enter: "Enter",
    Backspace: "Backspace",
    Delete: "Delete",
    Escape: "Escape",
    Home: "Home",
    End: "End",
    PageUp: "PageUp",
    PageDown: "PageDown",
    Insert: "Insert",
  };
  return mapped[key] ?? null;
}

/**
 * Return whether a canonical key may be registered without modifiers.
 * Sagascript has a native macOS path for F13-F24 and the Windows backend
 * supports the same range; the locked Linux backend has no mapping beyond F12.
 *
 * @param {string} key
 * @param {string} platform
 * @returns {boolean}
 */
export function canUseBareHotkey(key, platform) {
  const match = /^F(\d{1,2})$/i.exec(key.trim());
  if (!match) return false;

  const number = Number.parseInt(match[1], 10);
  const maximum = platform === "macos" || platform === "windows" ? 24 : 12;
  return number >= 13 && number <= maximum;
}

/**
 * @param {string} platform
 * @returns {string | null}
 */
export function supportedBareFunctionKeyRange(platform) {
  if (platform === "macos") return "F13–F24";
  if (platform === "windows") return "F13–F24";
  return null;
}
