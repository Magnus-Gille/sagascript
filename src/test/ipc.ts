// Typed Tauri IPC mock for frontend tests (#361).
//
//   const ipc = mockIpc({
//     get_settings: () => settingsFixture,
//     set_language: ({ language }) => (language === "fi" ? ["notice"] : []),
//   });
//   await setLanguage("fi");
//   expect(ipc.calls).toEqual([{ command: "set_language", args: { language: "fi" } }]);
//   ipc.emit("state-changed", "recording");
//
// Handlers are keyed by command name, with argument and result types from
// CommandMap, which is generated from the Rust command signatures (#347);
// src/lib/api.test.ts checks every wrapper against it. Event payloads are
// typed by EventMap (src/lib/events.ts). Every call is recorded in order. A
// command without a handler rejects, and src/test/setup.ts fails the test even
// if the code under test swallowed that rejection.
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";
import { SERIALIZE_TO_IPC_FN, type InvokeArgs } from "@tauri-apps/api/core";
import {
  BACKEND_ERROR_COMMANDS,
  COMMAND_ARGUMENTS,
  COMMAND_NAMES as GENERATED_COMMAND_NAMES,
  type CommandMap,
} from "../lib/ipc-contract.generated";
import { isBackendError, type BackendError } from "../lib/errors";
import type { EventMap, EventName } from "../lib/events";

declare global {
  interface Window {
    /** Set by `mockIPC`; the real one is injected by the Tauri webview. */
    __TAURI_INTERNALS__?: { runCallback?: (id: number, data: unknown) => void };
  }
}

/**
 * Every command the backend registers, with the argument object the frontend
 * sends and the result it resolves with: generated from the Rust command
 * signatures (src/lib/ipc-contract.generated.ts, #347), so the mock cannot
 * drift from the commands. Plugin commands such as `plugin:dialog|open` are
 * not covered yet and fail as unmocked.
 */
export type { CommandMap };

export type CommandName = keyof CommandMap;
export type CommandArgs<C extends CommandName> = CommandMap[C]["args"];
export type CommandResult<C extends CommandName> = CommandMap[C]["result"];
/**
 * What a fake handler throws to mimic the command's Rust `Err`, which the
 * wrapper rejects with: a string for `Result<T, String>`, a `BackendError`
 * (`backendError()`) for `Result<T, CommandError>`; `CommandMap[C]["error"]`
 * says which. The mock fails the test when the fake rejects with the other
 * shape, so a fake cannot keep an error shape Rust no longer sends (ADR 0006).
 */
export type CommandHandler<C extends CommandName> = (args: CommandArgs<C>) => CommandResult<C> | Promise<CommandResult<C>>;
export type CommandHandlers = { [C in CommandName]?: CommandHandler<C> };
/** One recorded invocation; narrow on `command` to get typed `args`. */
export type IpcCall = { [C in CommandName]: { command: C; args: CommandArgs<C> } }[CommandName];

/**
 * The error a command that returns `Result<T, CommandError>` rejects with, in
 * the wire shape Rust serializes (contracts/transport/command_errors.json).
 * `retryable` is false unless given.
 */
export function backendError(code: string, message: string, retryable = false): BackendError {
  return { code, message, retryable };
}

/** Every registered command name, sorted. */
export const COMMAND_NAMES: readonly CommandName[] = GENERATED_COMMAND_NAMES;
const COMMAND_SET: ReadonlySet<string> = new Set(COMMAND_NAMES);

export function isCommandName(name: string): name is CommandName {
  return COMMAND_SET.has(name);
}

/**
 * Why an argument object is not what the Rust command reads (a required
 * parameter missing, or a key that is no parameter), or null. Checked on every
 * mocked call, so a wrapper that drifted from the Rust signature fails at run
 * time too, not only in svelte-check.
 */
export function argumentProblem(command: CommandName, args: InvokeArgs | undefined): string | null {
  if (args !== undefined && (typeof args !== "object" || Array.isArray(args) || ArrayBuffer.isView(args) || args instanceof ArrayBuffer)) {
    return null; // a raw binary body; no command takes one yet
  }
  const sent = (args ?? {}) as Record<string, unknown>;
  const { required, optional } = COMMAND_ARGUMENTS[command] as { required: readonly string[]; optional: readonly string[] };
  const missing = required.filter((key) => sent[key] === undefined);
  const unknown = Object.keys(sent).filter((key) => !required.includes(key) && !optional.includes(key));
  if (missing.length === 0 && unknown.length === 0) return null;
  const parameters = [...required, ...optional.map((key) => `${key}?`)].join(", ") || "none";
  const problems = [
    ...(missing.length > 0 ? [`missing ${missing.join(", ")}`] : []),
    ...(unknown.length > 0 ? [`no parameter ${unknown.join(", ")}`] : []),
  ];
  return `IPC command "${command}" was invoked with arguments the Rust command does not read: ${problems.join("; ")} (parameters: ${parameters})`;
}

// Events and their payloads: the frontend half of the event catalogue (#347).
export type { EventMap, EventName } from "../lib/events";

export interface IpcMock {
  /** Every command invoked so far, in order, with a copy of its arguments. */
  readonly calls: readonly IpcCall[];
  /** The arguments of each call to one command, in order. */
  callsTo<C extends CommandName>(command: C): CommandArgs<C>[];
  /** Commands invoked without a handler (each one also failed its caller). */
  readonly unmocked: readonly string[];
  /** Add or replace a handler, e.g. to change a result between steps. */
  handle<C extends CommandName>(command: C, handler: CommandHandler<C>): void;
  /**
   * Deliver an event to the listeners registered with `listen()`, as the
   * backend would. Throws when nothing listens (usually a test that emits
   * before an asynchronous `listen` resolved). Returns the listener count.
   */
  emit<E extends EventName>(event: E, payload: EventMap[E]): number;
  /** How many `listen()` registrations for an event are active. */
  listenerCount(event: EventName): number;
  /**
   * The event of every active registration, sorted (an event listened to
   * twice appears twice), including events the frontend does not map, such as
   * the webview's `tauri://drag-*`. `[]` after a component unmounted shows it
   * unlistened everything.
   */
  listeners(): string[];
}

interface Listener {
  event: string;
  handler: number;
}

interface ActiveMock {
  calls: IpcCall[];
  unmocked: string[];
  invalid: string[];
  handlers: Map<string, (args: unknown) => unknown>;
  listeners: Listener[];
}

let active: ActiveMock | null = null;

// Arguments, results and event payloads cross a JSON boundary in the real app:
// the webview serializes invoke arguments with JSON.stringify and Rust answers
// in JSON. Copy them the same way, so a test's fixtures and recorded calls stay
// independent, an `undefined` field disappears as it does over IPC, and Svelte
// `$state` proxies (which components pass as arguments, and which
// structuredClone rejects) are serialized like plain objects.
function copy<T>(value: T): T {
  return value === undefined ? value : (JSON.parse(JSON.stringify(value, ipcReplacer)) as T);
}

// The replacer of Tauri's injected IPC serializer (tauri 2.10.2,
// scripts/process-ipc-message-fn.js): a Map becomes an object, a Uint8Array or
// ArrayBuffer an array of numbers, and an object with a SERIALIZE_TO_IPC_FN
// method (a Channel, a Resource) serializes itself. Not modelled: Tauri sends
// a top-level ArrayBuffer, ArrayBuffer view or Array payload as raw
// application/octet-stream (scripts/ipc-protocol.js), which the typed wrappers
// never do.
function ipcReplacer(_key: string, value: unknown): unknown {
  if (value instanceof Map) return Object.fromEntries(value.entries());
  if (value instanceof Uint8Array) return Array.from(value);
  if (value instanceof ArrayBuffer) return Array.from(new Uint8Array(value));
  if (typeof value === "object" && value !== null && SERIALIZE_TO_IPC_FN in value) {
    return (value as { [SERIALIZE_TO_IPC_FN]: () => unknown })[SERIALIZE_TO_IPC_FN]();
  }
  return value;
}

const BACKEND_ERROR_SET: ReadonlySet<string> = new Set(BACKEND_ERROR_COMMANDS);

function describeRejection(error: unknown): string {
  if (typeof error === "string") return "a string";
  if (typeof error === "object" && error !== null) return "an error object";
  return String(error);
}

/**
 * Why a fake's rejection is not what the Rust command rejects with (a string,
 * or a `BackendError` for `Result<T, CommandError>`), or null. `Error`
 * instances are tolerated for commands that still reject with strings: Rust
 * never sends one, but tests use them for failures that are not a command's.
 */
export function rejectionProblem(command: CommandName, error: unknown): string | null {
  if (BACKEND_ERROR_SET.has(command)) {
    if (isBackendError(error)) return null;
    return (
      `IPC command "${command}" returns Result<_, CommandError>: its fake must reject with backendError(code, message) ` +
      `from src/test/ipc.ts, not with ${describeRejection(error)}`
    );
  }
  if (typeof error === "string" || error instanceof Error) return null;
  return `IPC command "${command}" returns Result<_, String>: its fake must reject with a string, not with ${describeRejection(error)}`;
}

function record(state: ActiveMock, command: CommandName, args: unknown): void {
  // The command name and the argument shape belong together: the wrapper
  // that sent them is typed by CommandMap.
  state.calls.push({ command, args: copy(args) } as IpcCall);
}

function dispatch(state: ActiveMock, event: string, payload: unknown): number {
  const run = window.__TAURI_INTERNALS__?.runCallback;
  const listeners = state.listeners.filter((listener) => listener.event === event);
  for (const listener of listeners) {
    run?.(listener.handler, { event, id: listener.handler, payload: copy(payload) });
  }
  return listeners.length;
}

function eventPlugin(state: ActiveMock, command: string, args: InvokeArgs | undefined): unknown {
  const fields = (args ?? {}) as Record<string, unknown>;
  switch (command) {
    case "plugin:event|listen": {
      const handler = Number(fields.handler);
      state.listeners.push({ event: String(fields.event), handler });
      // The event id doubles as the callback id, so `unlisten()` (which
      // unregisters the callback by event id) releases the right one.
      return handler;
    }
    case "plugin:event|unlisten":
      state.listeners = state.listeners.filter((listener) => !(listener.event === fields.event && listener.handler === fields.eventId));
      return null;
    case "plugin:event|emit":
      dispatch(state, String(fields.event), fields.payload);
      return null;
    default:
      throw new Error(`The IPC mock does not support ${command} yet.`);
  }
}

/**
 * Install the IPC mock for the current test. `src/test/setup.ts` removes it
 * after each test and fails the test if an unmocked command was invoked, or
 * a command with arguments its Rust signature does not have.
 */
export function mockIpc(handlers: CommandHandlers = {}): IpcMock {
  resetIpcMock();
  const state: ActiveMock = { calls: [], unmocked: [], invalid: [], handlers: new Map(), listeners: [] };
  for (const [command, handler] of Object.entries(handlers)) {
    if (handler) state.handlers.set(command, handler as (args: unknown) => unknown);
  }
  active = state;

  mockIPC(async (command, args) => {
    if (command.startsWith("plugin:event|")) return eventPlugin(state, command, args);
    if (isCommandName(command)) {
      const problem = argumentProblem(command, args);
      if (problem) {
        state.invalid.push(problem);
        throw new Error(problem);
      }
      record(state, command, args);
    }
    const handler = state.handlers.get(command);
    if (!handler) {
      state.unmocked.push(command);
      throw new Error(
        `IPC command "${command}" has no mock handler. Pass one to mockIpc({ ${command}: … }); a new ` +
          `Rust command must first be in the generated CommandMap (contracts/README.md).`,
      );
    }
    try {
      return copy(await handler(copy(args ?? {})));
    } catch (error) {
      if (isCommandName(command)) {
        const problem = rejectionProblem(command, error);
        if (problem) state.invalid.push(problem);
      }
      // An error object crosses the IPC boundary as JSON, like a result.
      throw error instanceof Error ? error : copy(error);
    }
  });

  return {
    get calls() {
      return state.calls;
    },
    callsTo<C extends CommandName>(command: C): CommandArgs<C>[] {
      return state.calls.flatMap((call) => (call.command === command ? [call.args as CommandArgs<C>] : []));
    },
    get unmocked() {
      return state.unmocked;
    },
    handle(command, handler) {
      state.handlers.set(command, handler as (args: unknown) => unknown);
    },
    emit(event, payload) {
      const delivered = dispatch(state, event, payload);
      if (delivered === 0) {
        throw new Error(`No listener for "${event}": await the component's listen() before emitting.`);
      }
      return delivered;
    },
    listenerCount(event) {
      return state.listeners.filter((listener) => listener.event === event).length;
    },
    listeners() {
      return state.listeners.map((listener) => listener.event).sort();
    },
  };
}

/**
 * Remove the active mock (called after every test by src/test/setup.ts).
 * Throws when the test invoked a command without a handler, or with
 * arguments the Rust command does not read.
 */
export function resetIpcMock(): void {
  const state = active;
  active = null;
  if (typeof window !== "undefined") clearMocks();
  if (state && state.invalid.length > 0) {
    throw new Error(state.invalid[0]);
  }
  if (state && state.unmocked.length > 0) {
    throw new Error(`Unmocked IPC commands were invoked: ${[...new Set(state.unmocked)].join(", ")}`);
  }
}
