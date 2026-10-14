import { describe, expect, test, vi } from "vitest";
import { requestAccessibility, waitForAccessibility } from "./permissions";
import { mockIpc } from "../test/ipc";

const QUICK = { intervalMs: 1 };

/** check_accessibility_permission answering `answers` in order, then false. */
function checks(answers: boolean[]) {
  const remaining = [...answers];
  return mockIpc({
    check_accessibility_permission: () => remaining.shift() ?? false,
    request_accessibility_permission: () => undefined,
  });
}

describe("waitForAccessibility", () => {
  test("checks until macOS reports the grant, and no more after it", async () => {
    const ipc = checks([false, false, true, true]);

    await expect(waitForAccessibility(QUICK)).resolves.toBe(true);

    expect(ipc.callsTo("check_accessibility_permission")).toHaveLength(3);
  });

  test("waits one interval before the first check", async () => {
    const ipc = checks([true]);

    const wait = waitForAccessibility({ intervalMs: 30 });
    expect(ipc.callsTo("check_accessibility_permission")).toHaveLength(0);

    await expect(wait).resolves.toBe(true);
  });

  test("gives up after its attempts, with false", async () => {
    const ipc = checks([]);

    await expect(waitForAccessibility({ ...QUICK, attempts: 4 })).resolves.toBe(false);

    expect(ipc.callsTo("check_accessibility_permission")).toHaveLength(4);
  });

  test("cancelled while waiting, it never checks again", async () => {
    const ipc = checks([]);
    let cancelled = false;

    const wait = waitForAccessibility({ ...QUICK, isCancelled: () => cancelled });
    cancelled = true;

    await expect(wait).resolves.toBe(false);
    expect(ipc.callsTo("check_accessibility_permission")).toHaveLength(0);
  });

  test("an answer that arrives after cancelling is discarded, even a grant", async () => {
    let cancelled = false;
    mockIpc({
      check_accessibility_permission: () => {
        cancelled = true;
        return true;
      },
    });

    await expect(waitForAccessibility({ ...QUICK, isCancelled: () => cancelled })).resolves.toBe(false);
  });

  test("a failing check rejects and stops the polling", async () => {
    const ipc = mockIpc({
      check_accessibility_permission: () => {
        throw "TCC database unavailable";
      },
    });

    await expect(waitForAccessibility(QUICK)).rejects.toBe("TCC database unavailable");
    await new Promise((resolve) => setTimeout(resolve, 20));

    expect(ipc.callsTo("check_accessibility_permission")).toHaveLength(1);
  });
});

describe("requestAccessibility", () => {
  test("shows the prompt once, waits for the grant, then runs onGranted before resolving", async () => {
    const ipc = checks([false, true]);
    const onGranted = vi.fn(async () => {
      expect(ipc.calls.map((call) => call.command)).toEqual([
        "request_accessibility_permission",
        "check_accessibility_permission",
        "check_accessibility_permission",
      ]);
    });

    await expect(requestAccessibility({ ...QUICK, onGranted })).resolves.toBe(true);

    expect(onGranted).toHaveBeenCalledTimes(1);
    expect(ipc.callsTo("request_accessibility_permission")).toHaveLength(1);
  });

  test("a grant that never comes resolves false without onGranted", async () => {
    checks([]);
    const onGranted = vi.fn();

    await expect(requestAccessibility({ ...QUICK, attempts: 3, onGranted })).resolves.toBe(false);

    expect(onGranted).not.toHaveBeenCalled();
  });

  test("a failing prompt rejects before any check", async () => {
    const ipc = mockIpc({
      request_accessibility_permission: () => {
        throw "System Settings did not open";
      },
    });

    await expect(requestAccessibility(QUICK)).rejects.toBe("System Settings did not open");

    expect(ipc.callsTo("check_accessibility_permission")).toHaveLength(0);
  });

  test("cancelled while the prompt is showing, it does not wait", async () => {
    const ipc = checks([true]);
    let cancelled = false;
    ipc.handle("request_accessibility_permission", () => {
      cancelled = true;
    });

    await expect(requestAccessibility({ ...QUICK, isCancelled: () => cancelled })).resolves.toBe(false);

    expect(ipc.callsTo("check_accessibility_permission")).toHaveLength(0);
  });

  test("an onGranted that fails rejects the request", async () => {
    checks([true]);

    await expect(
      requestAccessibility({
        ...QUICK,
        onGranted: async () => {
          throw new Error("hotkey status unavailable");
        },
      }),
    ).rejects.toThrow("hotkey status unavailable");
  });
});
