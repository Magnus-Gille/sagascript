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
import type { InvokeArgs } from "@tauri-apps/api/core";
import { COMMAND_NAMES as GENERATED_COMMAND_NAMES, type CommandMap } from "../lib/ipc-contract.generated";
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
/** Throw a string to mimic a Rust `Err(String)`; the wrapper rejects with it. */
export type CommandHandler<C extends CommandName> = (args: CommandArgs<C>) => CommandResult<C> | Promise<CommandResult<C>>;
export type CommandHandlers = { [C in CommandName]?: CommandHandler<C> };
/** One recorded invocation; narrow on `command` to get typed `args`. */
export type IpcCall = { [C in CommandName]: { command: C; args: CommandArgs<C> } }[CommandName];

/** Every registered command name, sorted. */
export const COMMAND_NAMES: readonly CommandName[] = GENERATED_COMMAND_NAMES;
const COMMAND_SET: ReadonlySet<string> = new Set(COMMAND_NAMES);

export function isCommandName(name: string): name is CommandName {
  return COMMAND_SET.has(name);
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
}

interface Listener {
  event: string;
  handler: number;
}

interface ActiveMock {
  calls: IpcCall[];
  unmocked: string[];
  handlers: Map<string, (args: unknown) => unknown>;
  listeners: Listener[];
}

let active: ActiveMock | null = null;

// Arguments and results cross a serialization boundary in the real app;
// copying them keeps a test's fixtures and recorded calls independent.
function copy<T>(value: T): T {
  return value === undefined ? value : structuredClone(value);
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
 * after each test and fails the test if an unmocked command was invoked.
 */
export function mockIpc(handlers: CommandHandlers = {}): IpcMock {
  resetIpcMock();
  const state: ActiveMock = { calls: [], unmocked: [], handlers: new Map(), listeners: [] };
  for (const [command, handler] of Object.entries(handlers)) {
    if (handler) state.handlers.set(command, handler as (args: unknown) => unknown);
  }
  active = state;

  mockIPC(async (command, args) => {
    if (command.startsWith("plugin:event|")) return eventPlugin(state, command, args);
    if (isCommandName(command)) record(state, command, args);
    const handler = state.handlers.get(command);
    if (!handler) {
      state.unmocked.push(command);
      throw new Error(
        `IPC command "${command}" has no mock handler. Pass one to mockIpc({ ${command}: … }) ` +
          `or add the command to CommandMap in src/test/ipc.ts.`,
      );
    }
    return copy(await handler(copy(args ?? {})));
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
  };
}

/**
 * Remove the active mock (called after every test by src/test/setup.ts).
 * Throws when the test invoked a command without a handler.
 */
export function resetIpcMock(): void {
  const state = active;
  active = null;
  if (typeof window !== "undefined") clearMocks();
  if (state && state.unmocked.length > 0) {
    throw new Error(`Unmocked IPC commands were invoked: ${[...new Set(state.unmocked)].join(", ")}`);
  }
}
