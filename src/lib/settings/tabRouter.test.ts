// The tab router on its own (SAGA-379): which tab shows, when a request is
// refused and how a panel's guard defers it.
import { describe, expect, test, vi } from "vitest";
import { TabRouter, type LeaveGuard } from "./tabRouter.svelte";

const IDS = ["one", "two", "three"];

function guardThatHolds(): { guard: LeaveGuard; asked: Array<() => void>; setAsking(value: boolean): void } {
  const asked: Array<() => void> = [];
  let asking = false;
  return {
    guard: {
      requestLeave(leave) {
        asked.push(leave);
        asking = true;
      },
      isAsking: () => asking,
    },
    asked,
    setAsking: (value) => {
      asking = value;
    },
  };
}

describe("TabRouter", () => {
  test("the first id is active", () => {
    expect(new TabRouter(IDS).active).toBe("one");
  });

  test("a request changes the tab and runs afterNavigate", () => {
    const router = new TabRouter(IDS);
    const after = vi.fn();
    router.request("two", after);
    expect(router.active).toBe("two");
    expect(after).toHaveBeenCalledTimes(1);
  });

  test("a request for the active tab only runs afterNavigate", () => {
    const router = new TabRouter(IDS);
    const after = vi.fn();
    router.request("one", after);
    expect(router.active).toBe("one");
    expect(after).toHaveBeenCalledTimes(1);
  });

  test("while held nothing changes and afterNavigate does not run", () => {
    const router = new TabRouter(IDS);
    router.held = true;
    const after = vi.fn();
    router.request("two", after);
    router.request("one", after);
    expect(router.active).toBe("one");
    expect(after).not.toHaveBeenCalled();
  });

  test("with a guard, the tab changes only when the guard calls leave", () => {
    const router = new TabRouter(IDS);
    const { guard, asked } = guardThatHolds();
    router.guard = guard;
    const after = vi.fn();
    router.request("three", after);
    expect(asked).toHaveLength(1);
    expect(router.active).toBe("one");
    expect(after).not.toHaveBeenCalled();
    asked[0]();
    expect(router.active).toBe("three");
    expect(after).toHaveBeenCalledTimes(1);
  });

  test("asking follows the guard", () => {
    const router = new TabRouter(IDS);
    expect(router.asking).toBe(false);
    const { guard, setAsking } = guardThatHolds();
    router.guard = guard;
    expect(router.asking).toBe(false);
    router.request("two");
    expect(router.asking).toBe(true);
    setAsking(false);
    expect(router.asking).toBe(false);
  });

  test("has accepts exactly the ids", () => {
    const router = new TabRouter(IDS);
    for (const id of IDS) expect(router.has(id)).toBe(true);
    for (const other of [null, undefined, 1, 0, "", "four", "ONE", {}]) expect(router.has(other)).toBe(false);
  });
});
