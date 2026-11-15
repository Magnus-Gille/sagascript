// The personal dictionary editor rendered on its own (SAGA-379): its draft and
// dirty state, the explicit save, and the guard (`requestLeave`) the window's tab
// router asks before it leaves the Settings tab.
import { render, screen, waitFor, within } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import type { ComponentProps } from "svelte";
import { describe, expect, test, vi } from "vitest";
import settingsFixture from "../../../contracts/transport/get_settings.json";
import type { Settings } from "../api";
import { isBackendError, toErrorMessage } from "../errors";
import { backendError, mockIpc } from "../../test/ipc";
import { deferred, settle } from "../../test/deferred";
import GlossaryEditor from "./GlossaryEditor.svelte";

const customised = settingsFixture.customised as Settings;
const SAVED = "mergea = mördsa";
const TYPED = `${SAVED}\nOpenAI = open eye`;

type View = ReturnType<typeof render<typeof GlossaryEditor>>;
type Props = ComponentProps<typeof GlossaryEditor>;

interface MountOptions {
  active?: boolean;
  /** Runs inside the fake backend's write; throw `backendError(...)` to reject it. */
  write?: () => void | Promise<void>;
}

/** The editor with a fake backend and the window's save-then-read-back around it. */
async function mountEditor({ active = true, write }: MountOptions = {}) {
  let current: Settings = customised;
  const ref: { view: View | null } = { view: null };
  const ipc = mockIpc({
    set_profile_glossary: async ({ profileId, source }) => {
      await write?.();
      current = { ...current, profile_glossaries: { ...current.profile_glossaries, [profileId]: source } };
    },
  });
  const applySetting: Props["applySetting"] = async (mutate, errorSink) => {
    try {
      await mutate();
    } catch (error) {
      if (errorSink) {
        errorSink.value = toErrorMessage(error, "Failed to save setting.");
        errorSink.code = isBackendError(error) ? error.code : null;
      }
      return false;
    }
    await ref.view?.rerender({ settings: current });
    return true;
  };
  const readSettings: Props["readSettings"] = async () => {
    await ref.view?.rerender({ settings: current });
    return current;
  };
  const view = render(GlossaryEditor, {
    props: { settings: current, active, languageSaving: false, settingsError: "", applySetting, readSettings },
  });
  ref.view = view;
  const user = userEvent.setup();
  return { view, ipc, user, editor: view.component };
}

const dictionary = () => screen.queryByRole("textbox", { name: "Personal dictionary" }) as HTMLTextAreaElement | null;
const picker = () => screen.getByRole("combobox") as HTMLSelectElement;
const question = () => screen.queryByRole("dialog", { name: "Unsaved dictionary changes" });

async function typeIntoDictionary(user: ReturnType<typeof userEvent.setup>) {
  const textarea = await screen.findByRole("textbox", { name: "Personal dictionary" });
  await user.type(textarea, "\nOpenAI = open eye");
  return textarea as HTMLTextAreaElement;
}

/** Edit the dictionary and ask the guard to leave; returns the open question. */
async function askToLeave(setup: Awaited<ReturnType<typeof mountEditor>>) {
  const leave = vi.fn();
  await typeIntoDictionary(setup.user);
  setup.editor.requestLeave(leave);
  const dialog = await screen.findByRole("dialog", { name: "Unsaved dictionary changes" });
  return { leave, dialog };
}

describe("what the editor shows", () => {
  test("shows the first profile's dictionary when no profile is called default, clean", async () => {
    await mountEditor();

    expect(await screen.findByRole("textbox", { name: "Personal dictionary" })).toHaveValue(SAVED);
    expect(picker()).toHaveValue("swedish");
    expect(screen.getByRole("button", { name: "Save changes" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Discard changes" })).toBeDisabled();
    expect(screen.queryByText("Unsaved changes")).toBeNull();
  });

  test("lists every profile in the scope picker with its language name", async () => {
    await mountEditor();
    await screen.findByRole("textbox", { name: "Personal dictionary" });

    expect(
      within(picker())
        .getAllByRole("option")
        .map((option) => option.textContent),
    ).toEqual(["Svenska · Swedish", "Suomi · Finnish"]);
  });

  test("renders nothing of the editor while its tab is not active", async () => {
    await mountEditor({ active: false });
    await settle();

    expect(dictionary()).toBeNull();
    expect(screen.queryByRole("combobox")).toBeNull();
    expect(screen.queryByRole("button", { name: "Save changes" })).toBeNull();
  });

  test("keeps the unsaved draft while the tab is hidden and shows it again", async () => {
    const { view, user, editor } = await mountEditor();
    await typeIntoDictionary(user);

    await view.rerender({ active: false });
    await waitFor(() => expect(dictionary()).toBeNull());
    expect(editor.hasUnsavedChanges()).toBe(true);

    await view.rerender({ active: true });
    expect(await screen.findByRole("textbox", { name: "Personal dictionary" })).toHaveValue(TYPED);
    expect(editor.hasUnsavedChanges()).toBe(true);
    expect(screen.getByText("Unsaved changes")).toBeInTheDocument();
  });
});

describe("dirty state", () => {
  test("typing makes it dirty and typing back to the saved text makes it clean", async () => {
    const { user, editor } = await mountEditor();
    const textarea = await typeIntoDictionary(user);

    expect(editor.hasUnsavedChanges()).toBe(true);
    expect(screen.getByText("Unsaved changes")).toBeInTheDocument();

    await user.clear(textarea);
    await user.type(textarea, SAVED);
    expect(editor.hasUnsavedChanges()).toBe(false);
    expect(screen.queryByText("Unsaved changes")).toBeNull();
  });

  test("hands out the draft of the edited profile only while it is dirty", async () => {
    const { user, editor } = await mountEditor();
    expect(editor.unsavedDraftOf("swedish")).toBeNull();

    await typeIntoDictionary(user);
    expect(editor.unsavedDraftOf("swedish")).toBe(TYPED);
    expect(editor.unsavedDraftOf("finnish")).toBeNull();

    await user.click(screen.getByRole("button", { name: "Discard changes" }));
    expect(editor.unsavedDraftOf("swedish")).toBeNull();
  });
});

describe("saving", () => {
  test("sends the profile, the draft and the saved text once, shows Saving while held, then is clean", async () => {
    const held = deferred<void>();
    const { ipc, user, editor } = await mountEditor({ write: () => held.promise });
    await typeIntoDictionary(user);

    await user.click(screen.getByRole("button", { name: "Save changes" }));
    expect(await screen.findByRole("button", { name: "Saving…" })).toBeDisabled();
    expect(editor.isSaving()).toBe(true);
    expect(ipc.callsTo("set_profile_glossary")).toEqual([{ profileId: "swedish", source: TYPED, expectedSource: SAVED }]);

    await user.click(screen.getByRole("button", { name: "Saving…" }));
    await settle();
    expect(ipc.callsTo("set_profile_glossary")).toHaveLength(1);

    held.resolve();
    await waitFor(() => expect(editor.isSaving()).toBe(false));
    expect(editor.hasUnsavedChanges()).toBe(false);
    expect(screen.queryByText("Unsaved changes")).toBeNull();
    expect(screen.getByRole("button", { name: "Save changes" })).toBeDisabled();
    expect(dictionary()).toHaveValue(TYPED);
  });

  test("a rejected save keeps the draft and stays dirty, without the conflict hint for other codes", async () => {
    const { user, editor } = await mountEditor({
      write: () => {
        throw backendError("busy", "The dictionary write was refused.");
      },
    });
    const textarea = await typeIntoDictionary(user);
    await user.click(screen.getByRole("button", { name: "Save changes" }));

    await waitFor(() => expect(editor.isSaving()).toBe(false));
    await waitFor(() => expect(screen.getByRole("button", { name: "Save changes" })).toBeEnabled());
    expect(textarea).toHaveValue(TYPED);
    expect(editor.hasUnsavedChanges()).toBe(true);
    expect(screen.queryByText(/This dictionary changed elsewhere\. Your draft is preserved/)).toBeNull();
  });

  test("a save rejected because the dictionary changed elsewhere shows the conflict hint", async () => {
    const { user, editor } = await mountEditor({
      write: () => {
        throw backendError("dictionary_changed_elsewhere", "Dictionary changed elsewhere.");
      },
    });
    const textarea = await typeIntoDictionary(user);
    await user.click(screen.getByRole("button", { name: "Save changes" }));

    expect(await screen.findByText(/This dictionary changed elsewhere\. Your draft is preserved/)).toBeInTheDocument();
    expect(textarea).toHaveValue(TYPED);
    expect(editor.hasUnsavedChanges()).toBe(true);
  });

  test("Discard restores the saved text without any IPC call", async () => {
    const { ipc, user, editor } = await mountEditor();
    const textarea = await typeIntoDictionary(user);

    await user.click(screen.getByRole("button", { name: "Discard changes" }));
    expect(textarea).toHaveValue(SAVED);
    expect(editor.hasUnsavedChanges()).toBe(false);
    expect(ipc.calls).toEqual([]);
  });

  test("a language change in flight disables the textbox, the picker, Save and Discard", async () => {
    const { view, user } = await mountEditor();
    const textarea = await typeIntoDictionary(user);

    await view.rerender({ languageSaving: true });
    await waitFor(() => expect(textarea).toBeDisabled());
    expect(picker()).toBeDisabled();
    expect(screen.getByRole("button", { name: "Save changes" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Discard changes" })).toBeDisabled();
  });
});

describe("the dirty guard", () => {
  test("leaves at once when nothing is unsaved", async () => {
    const { editor } = await mountEditor();
    await screen.findByRole("textbox", { name: "Personal dictionary" });
    const leave = vi.fn();

    editor.requestLeave(leave);
    expect(leave).toHaveBeenCalledTimes(1);
    expect(editor.isAsking()).toBe(false);
    expect(question()).toBeNull();
  });

  test("asks before leaving while changes are unsaved", async () => {
    const setup = await mountEditor();
    const { leave, dialog } = await askToLeave(setup);

    expect(within(dialog).getByText("Save changes before leaving Settings?")).toBeInTheDocument();
    expect(setup.editor.isAsking()).toBe(true);
    expect(leave).not.toHaveBeenCalled();
  });

  test("Stay closes the question, never leaves and keeps the draft", async () => {
    const setup = await mountEditor();
    const { leave, dialog } = await askToLeave(setup);

    await setup.user.click(within(dialog).getByRole("button", { name: "Stay" }));
    expect(question()).toBeNull();
    expect(setup.editor.isAsking()).toBe(false);
    expect(leave).not.toHaveBeenCalled();
    expect(dictionary()).toHaveValue(TYPED);
  });

  test("Escape in the question acts as Stay", async () => {
    const setup = await mountEditor();
    const { leave, dialog } = await askToLeave(setup);
    await waitFor(() => expect(within(dialog).getByRole("button", { name: "Stay" })).toHaveFocus());

    await setup.user.keyboard("{Escape}");
    await waitFor(() => expect(question()).toBeNull());
    expect(leave).not.toHaveBeenCalled();
    expect(dictionary()).toHaveValue(TYPED);
  });

  test("Discard leaves once, drops the draft and writes nothing", async () => {
    const setup = await mountEditor();
    const { leave, dialog } = await askToLeave(setup);

    await setup.user.click(within(dialog).getByRole("button", { name: "Discard" }));
    expect(leave).toHaveBeenCalledTimes(1);
    expect(question()).toBeNull();
    expect(dictionary()).toHaveValue(SAVED);
    expect(setup.editor.hasUnsavedChanges()).toBe(false);
    expect(setup.ipc.callsTo("set_profile_glossary")).toEqual([]);
  });

  test("Save writes the dictionary and then leaves once", async () => {
    const setup = await mountEditor();
    const { leave, dialog } = await askToLeave(setup);

    await setup.user.click(within(dialog).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(leave).toHaveBeenCalledTimes(1));
    expect(setup.ipc.callsTo("set_profile_glossary")).toEqual([{ profileId: "swedish", source: TYPED, expectedSource: SAVED }]);
    expect(question()).toBeNull();
    expect(setup.editor.hasUnsavedChanges()).toBe(false);
  });

  test("a rejected Save does not leave and shows the error inside the open question", async () => {
    const setup = await mountEditor({
      write: () => {
        throw backendError("busy", "The dictionary write was refused.");
      },
    });
    const { leave, dialog } = await askToLeave(setup);

    await setup.user.click(within(dialog).getByRole("button", { name: "Save" }));
    expect(await within(dialog).findByRole("alert")).toHaveTextContent("The dictionary write was refused.");
    expect(question()).not.toBeNull();
    expect(leave).not.toHaveBeenCalled();
    expect(setup.editor.hasUnsavedChanges()).toBe(true);
  });

  test("does nothing while a save is in flight", async () => {
    const held = deferred<void>();
    const setup = await mountEditor({ write: () => held.promise });
    await typeIntoDictionary(setup.user);
    await setup.user.click(screen.getByRole("button", { name: "Save changes" }));
    await screen.findByRole("button", { name: "Saving…" });
    const leave = vi.fn();

    setup.editor.requestLeave(leave);
    await settle();
    expect(leave).not.toHaveBeenCalled();
    expect(question()).toBeNull();
    expect(setup.editor.isAsking()).toBe(false);

    held.resolve();
    await waitFor(() => expect(setup.editor.isSaving()).toBe(false));
  });

  test("switching profile while dirty asks and keeps the picker on the current profile until decided", async () => {
    const { user, editor } = await mountEditor();
    await typeIntoDictionary(user);

    await user.selectOptions(picker(), "finnish");
    const dialog = await screen.findByRole("dialog", { name: "Unsaved dictionary changes" });
    expect(within(dialog).getByText("Save changes before switching to Suomi?")).toBeInTheDocument();
    expect(picker()).toHaveValue("swedish");
    expect(editor.isAsking()).toBe(true);
    expect(dictionary()).toHaveValue(TYPED);
  });
});

describe("drafts preserved for recovery", () => {
  test("preserveDraft lists the draft once as a read-only textbox, even when preserved twice", async () => {
    const { editor } = await mountEditor();
    await screen.findByRole("textbox", { name: "Personal dictionary" });

    editor.preserveDraft("finnish", "kept = draft");
    editor.preserveDraft("finnish", "kept = draft");

    const kept = await screen.findByRole("textbox", { name: "Unsaved draft for Suomi" });
    expect(kept).toHaveValue("kept = draft");
    expect(kept).toHaveAttribute("readonly");
    expect(screen.getAllByRole("textbox", { name: "Unsaved draft for Suomi" })).toHaveLength(1);
  });

  test("discardChangesOf restores the saved text only for the profile being edited", async () => {
    const { user, editor } = await mountEditor();
    const textarea = await typeIntoDictionary(user);

    editor.discardChangesOf("finnish");
    await settle();
    expect(textarea).toHaveValue(TYPED);
    expect(editor.hasUnsavedChanges()).toBe(true);

    editor.discardChangesOf("swedish");
    await waitFor(() => expect(textarea).toHaveValue(SAVED));
    expect(editor.hasUnsavedChanges()).toBe(false);
  });
});
