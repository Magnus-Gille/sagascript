// Vitest setup for every test file (vitest.config.ts, #361).
import "@testing-library/jest-dom/vitest";
import { act, cleanup } from "@testing-library/svelte";
import { afterEach } from "vitest";
import { resetIpcMock } from "./ipc";

// Unmount rendered components first: their teardown unlistens through the
// IPC mock, which must still be installed. Then remove the mock; that fails
// the test if it invoked a command without a handler, or with arguments its
// Rust signature does not read, even when the code under test caught the error.
afterEach(async () => {
  await act();
  cleanup();
  resetIpcMock();
});
