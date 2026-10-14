// Vitest setup for every test file (vitest.config.ts, #361).
import { afterEach } from "vitest";
import { resetIpcMock } from "./ipc";

// Tests run in plain Node (no DOM environment yet). @tauri-apps/api keeps its
// IPC hooks on `window` (`window.__TAURI_INTERNALS__`) and the mock draws
// callback ids from `window.crypto`, so `window` is the Node global here.
if (typeof window === "undefined") {
  Object.defineProperty(globalThis, "window", { value: globalThis, configurable: true, writable: true });
}

// Remove the IPC mock after each test; fails the test if it invoked a
// command without a handler, even when the code under test caught the error.
afterEach(() => {
  resetIpcMock();
});
