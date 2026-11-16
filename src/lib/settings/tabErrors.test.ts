// The error of each tab on its own (SAGA-379): one message per tab, and the
// removal of a message that a later event made untrue.
import { describe, expect, test } from "vitest";
import { TabErrors } from "./tabErrors.svelte";

describe("TabErrors", () => {
  test("every tab starts without an error", () => {
    const errors = new TabErrors(["one", "two"]);

    expect(errors.of("one").message).toBe("");
    expect(errors.of("two").message).toBe("");
  });

  test("a message written for one tab is that tab's only", () => {
    const errors = new TabErrors(["one", "two"]);

    errors.of("one").message = "disk full";

    expect(errors.of("one").message).toBe("disk full");
    expect(errors.of("two").message).toBe("");
  });

  test("a tab that was not listed gets its own error, the same one every time", () => {
    const errors = new TabErrors(["one"]);

    errors.of("late").message = "late failure";

    expect(errors.of("late").message).toBe("late failure");
    expect(errors.of("one").message).toBe("");
  });

  test("dismiss removes the message from every tab that shows it and leaves the others", () => {
    const errors = new TabErrors(["one", "two", "three"]);
    errors.of("one").message = "held back";
    errors.of("two").message = "disk full";
    errors.of("three").message = "held back";

    errors.dismiss("held back");

    expect(errors.of("one").message).toBe("");
    expect(errors.of("two").message).toBe("disk full");
    expect(errors.of("three").message).toBe("");
  });

  test("a tab's own dismiss removes only the message it names", () => {
    const errors = new TabErrors(["one", "two"]);
    errors.of("one").message = "disk full";
    errors.of("two").message = "disk full";

    errors.of("one").dismiss("another message");
    expect(errors.of("one").message).toBe("disk full");

    errors.of("one").dismiss("disk full");
    expect(errors.of("one").message).toBe("");
    expect(errors.of("two").message).toBe("disk full");
  });

  test("retract takes away the message its receipt was given for", () => {
    const error = new TabErrors(["one"]).of("one");

    const receipt = error.show("the engines could not be listed");
    expect(error.message).toBe("the engines could not be listed");

    error.retract(receipt);
    expect(error.message).toBe("");
  });

  test("retract leaves a message written since, even one that says the same", () => {
    const error = new TabErrors(["one"]).of("one");

    const receipt = error.show("disk full");
    error.message = "disk full"; // another failure, with the same words

    error.retract(receipt);
    expect(error.message).toBe("disk full");
  });

  test("retract leaves a tab that was cleared and written again alone, and an older receipt does nothing", () => {
    const error = new TabErrors(["one"]).of("one");

    const older = error.show("first");
    const newer = error.show("second");
    error.retract(older);
    expect(error.message).toBe("second");

    error.message = "";
    error.message = "third";
    error.retract(newer);
    expect(error.message).toBe("third");
  });

  test("dismissing the empty message changes nothing", () => {
    const errors = new TabErrors(["one"]);
    errors.of("one").message = "disk full";

    errors.dismiss("");
    errors.of("one").dismiss("");

    expect(errors.of("one").message).toBe("disk full");
  });
});
