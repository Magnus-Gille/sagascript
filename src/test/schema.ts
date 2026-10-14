// Runtime schemas for the transport contract tests (#347).
//
// A schema parses an unknown JSON value into a typed one, or throws a
// ContractError that names the JSON path and what was expected. `Infer<S>` is
// the type a schema produces. src/test/transport.test.ts pins every schema to
// the frontend type it mirrors (`expectTypeOf(...).toEqualTypeOf<...>()`, a
// svelte-check error when they differ) and parses the shared fixtures in
// contracts/transport/ with it (a Vitest failure when Rust sends something
// else). Objects are strict: a field Rust sends must be declared, optional or
// explicitly ignored, so a renamed field fails on both of its names.

export class ContractError extends Error {
  override name = "ContractError";
}

export interface Schema<T> {
  /** Returns the value, typed, or throws a ContractError naming `path`. */
  parse(value: unknown, path?: string): T;
}

export type Infer<S> = S extends Schema<infer T> ? T : never;

function show(value: unknown): string {
  return value === undefined ? "nothing" : JSON.stringify(value);
}

function fail(path: string, expected: string, value: unknown): never {
  throw new ContractError(`${path}: expected ${expected}, got ${show(value)}`);
}

function schema<T>(parse: (value: unknown, path: string) => T): Schema<T> {
  return { parse: (value, path = "$") => parse(value, path) };
}

function isObject(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

export const string: Schema<string> = schema((value, path) => (typeof value === "string" ? value : fail(path, "a string", value)));

export const number: Schema<number> = schema((value, path) =>
  typeof value === "number" && Number.isFinite(value) ? value : fail(path, "a finite number", value),
);

export const boolean: Schema<boolean> = schema((value, path) => (typeof value === "boolean" ? value : fail(path, "a boolean", value)));

/** Exactly one of `values` (an enum on the Rust side). */
export function literal<const L extends readonly (string | number | boolean | null)[]>(...values: L): Schema<L[number]> {
  return schema((value, path) =>
    values.some((allowed) => allowed === value) ? (value as L[number]) : fail(path, `one of ${values.map(show).join(", ")}`, value),
  );
}

/** `T | null`: a Rust `Option` that serializes `None` as `null`. */
export function nullable<T>(inner: Schema<T>): Schema<T | null> {
  return schema((value, path) => (value === null ? null : inner.parse(value, path)));
}

export function array<T>(item: Schema<T>): Schema<T[]> {
  return schema((value, path) =>
    Array.isArray(value) ? value.map((entry, index) => item.parse(entry, `${path}[${index}]`)) : fail(path, "an array", value),
  );
}

/** A map with string keys (a Rust `BTreeMap<String, T>`). */
export function record<T>(entry: Schema<T>): Schema<Record<string, T>> {
  return schema((value, path) => {
    if (!isObject(value)) return fail(path, "an object", value);
    return Object.fromEntries(Object.entries(value).map(([key, item]) => [key, entry.parse(item, `${path}.${key}`)]));
  });
}

/** The first member that parses the value (a Rust enum with data). */
export function union<const S extends readonly Schema<unknown>[]>(...members: S): Schema<Infer<S[number]>> {
  return schema((value, path) => {
    const errors: string[] = [];
    for (const member of members) {
      try {
        return member.parse(value, path) as Infer<S[number]>;
      } catch (error) {
        if (!(error instanceof ContractError)) throw error;
        errors.push(error.message);
      }
    }
    throw new ContractError(`${path}: matches none of the alternatives:\n  ${errors.join("\n  ")}`);
  });
}

type Shape = Record<string, Schema<unknown>>;
type Simplify<T> = { [K in keyof T]: T[K] } & {};
type Fields<R extends Shape, O extends Shape> = { [K in keyof R]: Infer<R[K]> } & { [K in keyof O]?: Infer<O[K]> };

export interface ObjectOptions<O extends Shape> {
  /** Fields the TypeScript type marks optional (`field?:`); they may be absent. */
  optional?: O;
  /** Fields Rust sends that the frontend deliberately does not read or type. */
  ignored?: readonly string[];
}

function parseFields(
  value: unknown,
  path: string,
  required: Shape,
  optional: Shape,
  extra: (key: string) => boolean,
): Record<string, unknown> {
  if (!isObject(value)) return fail(path, "an object", value);
  const result: Record<string, unknown> = {};
  for (const [key, field] of Object.entries(required)) {
    if (!Object.hasOwn(value, key)) {
      throw new ContractError(`${path}.${key}: missing (the TypeScript type requires it; was it renamed or removed?)`);
    }
    result[key] = field.parse(value[key], `${path}.${key}`);
  }
  for (const [key, field] of Object.entries(optional)) {
    if (Object.hasOwn(value, key)) result[key] = field.parse(value[key], `${path}.${key}`);
  }
  for (const key of Object.keys(value)) {
    if (Object.hasOwn(required, key) || Object.hasOwn(optional, key)) continue;
    if (!extra(key)) {
      throw new ContractError(
        `${path}.${key}: sent but not in the TypeScript type; declare it, or list it as ignored if the frontend never reads it`,
      );
    }
    result[key] = value[key];
  }
  return result;
}

/** A struct: every required field present, nothing undeclared. */
export function object<R extends Shape, O extends Shape = Record<never, never>>(
  required: R,
  options: ObjectOptions<O> = {},
): Schema<Simplify<Fields<R, O>>> {
  const ignored = new Set(options.ignored ?? []);
  return schema((value, path) => {
    const fields = parseFields(value, path, required, options.optional ?? {}, (key) => ignored.has(key));
    for (const key of ignored) delete fields[key];
    return fields as Simplify<Fields<R, O>>;
  });
}

/** A struct whose TypeScript type also has `[key: string]: unknown`. */
export function openObject<R extends Shape, O extends Shape = Record<never, never>>(
  required: R,
  options: Omit<ObjectOptions<O>, "ignored"> = {},
): Schema<Simplify<Fields<R, O> & { [key: string]: unknown }>> {
  return schema(
    (value, path) =>
      parseFields(value, path, required, options.optional ?? {}, () => true) as Simplify<Fields<R, O> & { [key: string]: unknown }>,
  );
}
