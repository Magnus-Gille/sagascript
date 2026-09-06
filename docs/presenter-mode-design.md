# Presenter mode (#162): implementation contract

Status: runtime integration implemented, verification/review in progress; not
feature complete. Do not publish this as a completed feature until the runtime,
CLI, UI, independent review and physical acceptance checks below pass.

## Current implementation snapshot

The desktop runtime now has the Presenter state flow: an existing language
profile shortcut starts a session, the configured Finish shortcut stops and
transcribes it, and the optional Cancel shortcut cancels it. If Start omits a
profile ID, the default profile is selected when present, otherwise the first
resolved profile is used. The action and target are frozen per session and
generation checks reject stale callbacks. Presenter ownership is explicit: a
manual Dictate-button recording retains its normal completion/cancel pipeline
even when the configured shortcut mode is Presenter. All Presenter transports,
including hotkeys, dispatch their coordination onto the main thread.

On macOS, verified insertion additionally requires Presenter mode and
auto-paste to be enabled, a supported Accessibility target, released shortcut
modifiers, and observed post-insertion field state. A single two-second budget
allows held Finish modifiers to be released before insertion and bounds the
subsequent observation. Modifiers still held at expiry, unknown or
unsupported targets, target changes, missing evidence, and expired observation
deadlines retain any recognized text as a draft rather than submitting it.
Empty or failed transcription never submits and creates no new recognized draft.
Windows and Linux currently retain drafts for
manual copy; they do not advertise automatic target-aware submission.

Field contents and selections used for verification stay in bounded transient
process memory only. They are not written to logs, settings, IPC, or status
messages. A successful submit-key dispatch is not proof that the editor or an
application/server accepted the prompt; the `Sent` status means only that the
native key action reported success.

## Activation and configuration

Existing per-language profile shortcuts are the atomic **Start** actions in
Presenter mode. A separate global Finish shortcut stops the active presenter
recording and transcribes using its frozen profile. An optional Cancel shortcut
aborts the presenter session. Key release never finishes Presenter mode, and a
second Start while busy is a no-op. Push-to-talk and Toggle retain their current
semantics. Training recordings are never controlled by Presenter shortcuts.

The default finish action is Insert only. Return and Command+Return require an
explicit rule for the captured application's stable native identity. Rules are
not inferred from titles, websites, transcription text, or a process name.
Command+Return means the macOS Command modifier; unsupported platform actions
must fail closed, not silently turn into another keystroke.

Profile, Finish and Cancel bindings belong to one validated registration
transaction. Canonical aliases must not create duplicate bindings. A failed
registration must retain/restore the previous effective bindings and expose an
error; persistence must not falsely claim that new shortcuts are operational.
Changing active configuration during a recording must not retarget that session.

## Insertion and submission boundary

At Start, capture the target application and focused field before showing any
UI. Native handles stay on their required thread and are never serialized or
logged. Text used to verify insertion stays only in bounded session memory;
no field values, titles or clipboard contents enter diagnostics or IPC.
Unknown, inaccessible, secure or unsupported targets cannot authorize Submit.

The session freezes its action and target. Focus-change notifications invalidate
submission eligibility irreversibly for that session, including a change away
and back. Rechecking the current target is also required immediately before
insertion and before Submit. Notification support or native verification failure
must never degrade into unchecked Return injection.

Successful transcription and a nonempty result are necessary but insufficient.
The existing paste callback means the native paste call returned; it does **not**
prove the editor received text. Submit requires observable target-field state
matching the exact expected post-insertion value and selection. A deadline may
bound waiting for this evidence; elapsed time is never evidence of completion.
Do not alter the legacy paste path merely to accommodate Presenter mode.

Cancellation, stale callbacks, timeout, insertion failure, target change or
missing evidence must never submit. Consume a successful submission opportunity
at most once. Preserve recognized text as a draft/copy fallback with a clear
status, without moving focus while a pending native action could still execute.
Cancellation during inference must invalidate pending insertion and Submit, not
open a new recording while the old worker can still mutate controller state.

macOS integration will use its existing Accessibility permission; it must not
request or modify TCC approval in background. The platform adapter must verify
notification support because [AXObserverAddNotification](https://developer.apple.com/documentation/applicationservices/1462089-axobserveraddnotification)
can report unsupported notifications. The
[focused-element notification](https://developer.apple.com/documentation/applicationservices/kaxfocuseduielementchangednotification)
is an observation input, not authorization to synthesize a key.
Apple declares [accessibilitySubrole](https://developer.apple.com/documentation/appkit/nsaccessibility-c.protocol/accessibilitysubrole)
nullable. A subrole read reporting only the documented
[no-value or unsupported-attribute result](https://developer.apple.com/documentation/applicationservices/1462085-axuielementcopyattributevalue)
is treated as absent; transport/permission failures, malformed null successes,
wrong value types and explicit secure-field subroles remain rejected. The
required text role, field-value/selection, application identity and notification
checks still apply. Start revalidation reuses the current-target traversal
already performed by its snapshot check; native capture latency remains to be
measured on supported editors.
Windows/Linux require their own verified target/field adapters before equivalent
automatic submission can be advertised. Unsupported paths retain drafts.

## CLI parity and feedback

The CLI exposes explicit presenter Start, Finish and Cancel. The supported
request commands are:

```text
sagascript presenter start [PROFILE-ID]
sagascript presenter finish
sagascript presenter cancel
```

`PROFILE-ID` is optional and must use the bounded lowercase ASCII profile-ID
grammar. These commands send a private request to the installed desktop app;
the CLI process does not itself record, transcribe, insert text, or report
completion. The desktop handles the requested action. The existing
single-instance argv transport can carry fixed action markers but has no result
acknowledgment today. A launcher success may therefore only mean a request was
sent, never that recording or insertion succeeded. Keep the grammar bounded,
reject mixed/unknown markers, ignore caller cwd, and carry no text or paths.
Presenter dispatch must bypass Settings reveal and activation paths entirely.

Presenter configuration is available through the existing CLI config command;
these are the exact source-documented forms (not executed as part of this
documentation update):

```text
sagascript config set hotkey_mode presenter
sagascript config presenter show
sagascript config presenter finish 'Control+Shift+Enter'
sagascript config presenter cancel 'Control+Shift+Escape'
sagascript config presenter cancel
sagascript config presenter app com.example.editor command_return
sagascript config presenter remove-app com.example.editor
```

Application actions are explicit opt-ins and default to `insert_only`. Verified
native insertion in the runtime is gated by Presenter mode and auto-paste being
enabled; the app-specific Return actions additionally require the supported
macOS target path and Accessibility permission.

Expose Listening, Transcribing, Inserted, Sent, Cancelled, draft-on-target-change
and failure outcomes without transient messages falsely claiming delivery.
Settings should explain that Logitech Spotlight maps its long-press actions to
the two atomic shortcuts; Sagascript does not implement the remote's press
timing. The physical Spotlight mapping is external to the app and remains
unverified.

## Required verification

- Red/green tests for legacy settings compatibility, explicit opt-in rules,
  canonical shortcut collisions and atomic rejected updates.
- State tests for duplicate Start/Finish/Cancel, key releases, training isolation,
  late callbacks and configuration changes; PTT/Toggle regressions remain green.
- Injected insertion/target tests prove no Submit on empty/failed transcription,
  unknown/changed focus, unsupported notifications, timeout, cancellation or
  unmatched text; prove one Submit only after observed insertion.
- Pure UTF-16 range tests cover emoji boundaries and overflow without normalizing
  text, before using native selected-range coordinates.
- CLI parsing/transport tests prove no focus-taking operation and no false
  completion acknowledgment. Frontend tests and visual verification cover all
  new controls/statuses; full Rust/frontend checks precede cross-model review.
- Signed-device tests, not unsigned development or source inspection, must prove
  real focus continuity, insertion observation, permission behavior and Logitech
  Spotlight long-press mappings before the ticket's hardware acceptance is met.

Current verification status: the pure routing, state-machine, generation,
target-validation, UTF-16, observation-policy and CLI grammar tests are present;
native Accessibility, clipboard, key dispatch, signed-device permission and
physical Spotlight checks remain outstanding. Root's current UI check covered
draft, sent and submit-uncertain statuses at narrow and wide widths, with no
horizontal overflow and readable application-ID rows at 300 px. This does not
replace signed-device or hardware acceptance.
