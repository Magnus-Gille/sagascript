import { expect, test } from "vitest";
import { forgetPlatform, loadPlatform } from "./platform";
import { deferred } from "../test/deferred";
import { mockIpc } from "../test/ipc";

test("the platform is asked for once, however many callers and however late", async () => {
  const answer = deferred<string>();
  const ipc = mockIpc({ get_platform: () => answer.promise });

  const callers = [loadPlatform(), loadPlatform()];
  answer.resolve("macos");

  await expect(Promise.all(callers)).resolves.toEqual(["macos", "macos"]);
  await expect(loadPlatform()).resolves.toBe("macos");
  expect(ipc.callsTo("get_platform")).toHaveLength(1);
});

test("a failed lookup rejects every caller that shared it and is asked again next time", async () => {
  let failing = true;
  const ipc = mockIpc({
    get_platform: () => {
      if (failing) throw "no window";
      return "windows";
    },
  });

  const callers = [loadPlatform(), loadPlatform()];
  await expect(callers[0]).rejects.toBe("no window");
  await expect(callers[1]).rejects.toBe("no window");

  failing = false;
  await expect(loadPlatform()).resolves.toBe("windows");
  expect(ipc.callsTo("get_platform")).toHaveLength(2);
});

test("forgetting the answer makes the next call ask again", async () => {
  let platform = "macos";
  const ipc = mockIpc({ get_platform: () => platform });
  await loadPlatform();

  platform = "windows";
  forgetPlatform();

  await expect(loadPlatform()).resolves.toBe("windows");
  expect(ipc.callsTo("get_platform")).toHaveLength(2);
});
