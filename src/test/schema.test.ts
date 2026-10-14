// The contract-test schemas (#347) must reject what Rust could send wrongly.
import { describe, expect, expectTypeOf, test } from "vitest";
import { array, boolean, ContractError, literal, nullable, number, object, openObject, record, string, union, type Infer } from "./schema";

const profile = object(
  { id: string, language: literal("en", "sv") },
  { optional: { toggle_shortcut: nullable(string) }, ignored: ["schema_version"] },
);

describe("object", () => {
  test("parses declared, optional and ignored fields, and drops the ignored ones", () => {
    expect(profile.parse({ id: "a", language: "sv", schema_version: 3 })).toEqual({ id: "a", language: "sv" });
    expect(profile.parse({ id: "a", language: "en", toggle_shortcut: null })).toEqual({ id: "a", language: "en", toggle_shortcut: null });
    expectTypeOf<Infer<typeof profile>>().toEqualTypeOf<{ id: string; language: "en" | "sv"; toggle_shortcut?: string | null }>();
  });

  test("a renamed field fails on both names", () => {
    expect(() => profile.parse({ identifier: "a", language: "sv" }, "profile")).toThrow("profile.id: missing");
    expect(() => profile.parse({ id: "a", identifier: "a", language: "sv" }, "profile")).toThrow(
      "profile.identifier: sent but not in the TypeScript type",
    );
  });

  test("a changed type or enum value fails with its path", () => {
    expect(() => profile.parse({ id: 1, language: "sv" }, "p")).toThrow("p.id: expected a string, got 1");
    expect(() => profile.parse({ id: "a", language: "fi" }, "p")).toThrow('p.language: expected one of "en", "sv", got "fi"');
    expect(() => profile.parse({ id: "a", language: "sv", toggle_shortcut: 5 }, "p")).toThrow(ContractError);
  });

  test("an open object keeps undeclared fields", () => {
    const report = openObject({ schema_version: number });
    expect(report.parse({ schema_version: 1, extra: true })).toEqual({ schema_version: 1, extra: true });
    expect(() => report.parse({ extra: true }, "r")).toThrow("r.schema_version: missing");
  });
});

test("containers check every element", () => {
  expect(array(number).parse([1, 2])).toEqual([1, 2]);
  expect(() => array(number).parse([1, "2"], "xs")).toThrow("xs[1]: expected a finite number");
  expect(() => record(boolean).parse({ a: true, b: 1 }, "m")).toThrow("m.b: expected a boolean");
  expect(() => number.parse(Number.NaN, "n")).toThrow("n: expected a finite number");
  expect(() => nullable(string).parse(undefined, "s")).toThrow("s: expected a string, got nothing");
});

test("a union names every alternative that failed", () => {
  const operation = union(object({ kind: literal("rename"), label: string }), object({ kind: literal("merge"), into: string }));
  expect(operation.parse({ kind: "merge", into: "s1" })).toEqual({ kind: "merge", into: "s1" });
  expect(() => operation.parse({ kind: "split" }, "op")).toThrow(
    /op: matches none of the alternatives:\n {2}op\.kind: expected one of "rename"/,
  );
});
