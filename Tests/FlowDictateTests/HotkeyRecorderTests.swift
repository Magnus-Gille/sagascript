import XCTest
import AppKit
import Carbon.HIToolbox
@testable import FlowDictate

final class HotkeyRecorderTests: XCTestCase {

    // MARK: - Basic State Tests

    func testNotRecordingReturnsIgnored() {
        let result = processKeyEvent(
            keyCode: UInt16(kVK_ANSI_A),
            modifierFlags: .command,
            isRecording: false
        )
        XCTAssertEqual(result, .ignored)
    }

    func testEscapeCancelsRecording() {
        let result = processKeyEvent(
            keyCode: UInt16(kVK_Escape),
            modifierFlags: [],
            isRecording: true
        )
        XCTAssertEqual(result, .cancelled)
    }

    // MARK: - Modifier Key Tests

    func testModifierOnlyKeyIsAccepted() {
        // Modifier-only combos are accepted (to allow single-modifier hotkeys)
        let shiftResult = processKeyEvent(
            keyCode: UInt16(kVK_Shift),
            modifierFlags: .shift,
            isRecording: true
        )
        XCTAssertEqual(shiftResult, .accepted(keyCode: kVK_Shift, modifiers: Int(shiftKey)))

        let cmdResult = processKeyEvent(
            keyCode: UInt16(kVK_Command),
            modifierFlags: .command,
            isRecording: true
        )
        XCTAssertEqual(cmdResult, .accepted(keyCode: kVK_Command, modifiers: Int(cmdKey)))

        let ctrlResult = processKeyEvent(
            keyCode: UInt16(kVK_Control),
            modifierFlags: .control,
            isRecording: true
        )
        XCTAssertEqual(ctrlResult, .accepted(keyCode: kVK_Control, modifiers: Int(controlKey)))

        let optResult = processKeyEvent(
            keyCode: UInt16(kVK_Option),
            modifierFlags: .option,
            isRecording: true
        )
        XCTAssertEqual(optResult, .accepted(keyCode: kVK_Option, modifiers: Int(optionKey)))
    }

    // MARK: - Regular Key Tests

    func testRegularKeyWithoutModifierAccepted() {
        // Regular keys without modifiers are accepted
        let result = processKeyEvent(
            keyCode: UInt16(kVK_ANSI_A),
            modifierFlags: [],
            isRecording: true
        )
        XCTAssertEqual(result, .accepted(keyCode: kVK_ANSI_A, modifiers: 0))
    }

    func testRegularKeyWithCommandAccepted() {
        // Cmd+A
        let result = processKeyEvent(
            keyCode: UInt16(kVK_ANSI_A),
            modifierFlags: .command,
            isRecording: true
        )
        XCTAssertEqual(result, .accepted(keyCode: kVK_ANSI_A, modifiers: Int(cmdKey)))
    }

    func testRegularKeyWithControlAccepted() {
        // Ctrl+A
        let result = processKeyEvent(
            keyCode: UInt16(kVK_ANSI_A),
            modifierFlags: .control,
            isRecording: true
        )
        XCTAssertEqual(result, .accepted(keyCode: kVK_ANSI_A, modifiers: Int(controlKey)))
    }

    func testRegularKeyWithShiftAccepted() {
        // Shift+A
        let result = processKeyEvent(
            keyCode: UInt16(kVK_ANSI_A),
            modifierFlags: .shift,
            isRecording: true
        )
        XCTAssertEqual(result, .accepted(keyCode: kVK_ANSI_A, modifiers: Int(shiftKey)))
    }

    func testRegularKeyWithOptionAccepted() {
        // Option+A
        let result = processKeyEvent(
            keyCode: UInt16(kVK_ANSI_A),
            modifierFlags: .option,
            isRecording: true
        )
        XCTAssertEqual(result, .accepted(keyCode: kVK_ANSI_A, modifiers: Int(optionKey)))
    }

    func testMultipleModifiersAccepted() {
        // Cmd+Shift+A
        let result = processKeyEvent(
            keyCode: UInt16(kVK_ANSI_A),
            modifierFlags: [.command, .shift],
            isRecording: true
        )
        XCTAssertEqual(result, .accepted(keyCode: kVK_ANSI_A, modifiers: Int(cmdKey) | Int(shiftKey)))
    }

    func testControlShiftSpaceAccepted() {
        // Ctrl+Shift+Space (the default hotkey)
        let result = processKeyEvent(
            keyCode: UInt16(kVK_Space),
            modifierFlags: [.control, .shift],
            isRecording: true
        )
        XCTAssertEqual(result, .accepted(keyCode: kVK_Space, modifiers: Int(controlKey) | Int(shiftKey)))
    }

    // MARK: - Function Key Tests

    func testFunctionKeyWithoutModifierAccepted() {
        // F1 without modifiers should be accepted
        let f1Result = processKeyEvent(
            keyCode: UInt16(kVK_F1),
            modifierFlags: [],
            isRecording: true
        )
        XCTAssertEqual(f1Result, .accepted(keyCode: kVK_F1, modifiers: 0))

        // F12 without modifiers should be accepted
        let f12Result = processKeyEvent(
            keyCode: UInt16(kVK_F12),
            modifierFlags: [],
            isRecording: true
        )
        XCTAssertEqual(f12Result, .accepted(keyCode: kVK_F12, modifiers: 0))
    }

    func testFunctionKeyWithModifierAccepted() {
        // Cmd+F1
        let result = processKeyEvent(
            keyCode: UInt16(kVK_F1),
            modifierFlags: .command,
            isRecording: true
        )
        XCTAssertEqual(result, .accepted(keyCode: kVK_F1, modifiers: Int(cmdKey)))
    }

    // MARK: - Fn Modifier Tests

    func testFnModifierWithKey() {
        // Fn+Z should have the Fn bit set
        let result = processKeyEvent(
            keyCode: UInt16(kVK_ANSI_Z),
            modifierFlags: .function,
            isRecording: true
        )
        XCTAssertEqual(result, .accepted(keyCode: kVK_ANSI_Z, modifiers: kModsFnBit))
    }

    func testFnWithOtherModifiers() {
        // Fn+Cmd+Z
        let result = processKeyEvent(
            keyCode: UInt16(kVK_ANSI_Z),
            modifierFlags: [.function, .command],
            isRecording: true
        )
        XCTAssertEqual(result, .accepted(keyCode: kVK_ANSI_Z, modifiers: kModsFnBit | Int(cmdKey)))
    }

    // MARK: - Carbon Key Code Value Tests

    func testCarbonKeyCodesAreCorrect() {
        // Verify the Carbon key codes we use are the expected values
        // This documents the non-sequential nature of F-key codes
        XCTAssertEqual(kVK_F1, 0x7A)   // 122
        XCTAssertEqual(kVK_F2, 0x78)   // 120
        XCTAssertEqual(kVK_F3, 0x63)   // 99
        XCTAssertEqual(kVK_F12, 0x6F)  // 111

        // Verify F1 > F12 (which is why range doesn't work)
        XCTAssertGreaterThan(kVK_F1, kVK_F12)

        XCTAssertEqual(kVK_Escape, 0x35)  // 53
        XCTAssertEqual(kVK_Space, 0x31)   // 49
        XCTAssertEqual(kVK_ANSI_A, 0x00)  // 0
    }

    // MARK: - HotkeyRecorder Class Tests

    func testHotkeyRecorderInitialState() {
        let recorder = HotkeyRecorder()
        XCTAssertFalse(recorder.isRecording)
    }

    func testHotkeyRecorderStartsRecording() {
        let recorder = HotkeyRecorder()
        recorder.startRecording { _, _ in }
        XCTAssertTrue(recorder.isRecording)
        recorder.stopRecording()
    }

    func testHotkeyRecorderStopsRecording() {
        let recorder = HotkeyRecorder()
        recorder.startRecording { _, _ in }
        XCTAssertTrue(recorder.isRecording)
        recorder.stopRecording()
        XCTAssertFalse(recorder.isRecording)
    }

    func testHotkeyRecorderDoesNotDoubleStart() {
        let recorder = HotkeyRecorder()
        recorder.startRecording { _, _ in }
        recorder.startRecording { _, _ in }

        // Should still be recording (didn't restart)
        XCTAssertTrue(recorder.isRecording)
        recorder.stopRecording()
    }

    // MARK: - Shortcut Model Tests

    func testModifierMaskFromNSEventFlags() {
        // Command
        XCTAssertEqual(modifierMask(from: .command), Int(cmdKey))

        // Option
        XCTAssertEqual(modifierMask(from: .option), Int(optionKey))

        // Control
        XCTAssertEqual(modifierMask(from: .control), Int(controlKey))

        // Shift
        XCTAssertEqual(modifierMask(from: .shift), Int(shiftKey))

        // Function (Fn)
        XCTAssertEqual(modifierMask(from: .function), kModsFnBit)

        // Combined
        let combined: NSEvent.ModifierFlags = [.command, .shift, .function]
        XCTAssertEqual(modifierMask(from: combined), Int(cmdKey) | Int(shiftKey) | kModsFnBit)
    }

    func testCarbonModifiersStripping() {
        // Should strip Fn bit
        let withFn = Int(cmdKey) | Int(shiftKey) | kModsFnBit
        XCTAssertEqual(carbonModifiers(from: withFn), UInt32(cmdKey) | UInt32(shiftKey))

        // Without Fn should be unchanged
        let withoutFn = Int(cmdKey) | Int(shiftKey)
        XCTAssertEqual(carbonModifiers(from: withoutFn), UInt32(cmdKey) | UInt32(shiftKey))
    }

    func testIsModifierKeyCode() {
        // Modifier keys
        XCTAssertTrue(isModifierKeyCode(kVK_Command))
        XCTAssertTrue(isModifierKeyCode(kVK_RightCommand))
        XCTAssertTrue(isModifierKeyCode(kVK_Shift))
        XCTAssertTrue(isModifierKeyCode(kVK_RightShift))
        XCTAssertTrue(isModifierKeyCode(kVK_Control))
        XCTAssertTrue(isModifierKeyCode(kVK_RightControl))
        XCTAssertTrue(isModifierKeyCode(kVK_Option))
        XCTAssertTrue(isModifierKeyCode(kVK_RightOption))
        XCTAssertTrue(isModifierKeyCode(kVK_Function))

        // Non-modifier keys
        XCTAssertFalse(isModifierKeyCode(kVK_ANSI_A))
        XCTAssertFalse(isModifierKeyCode(kVK_Space))
        XCTAssertFalse(isModifierKeyCode(kVK_F1))
        XCTAssertFalse(isModifierKeyCode(kVK_Escape))
    }

    func testRequiresCGEventTapBackend() {
        // Modifiers-only requires CGEventTap
        XCTAssertTrue(requiresCGEventTapBackend(keyCode: kKeyCodeModifiersOnly, modifiers: Int(cmdKey)))

        // Fn modifier requires CGEventTap
        XCTAssertTrue(requiresCGEventTapBackend(keyCode: kVK_ANSI_Z, modifiers: kModsFnBit))
        XCTAssertTrue(requiresCGEventTapBackend(keyCode: kVK_ANSI_Z, modifiers: kModsFnBit | Int(cmdKey)))

        // Normal shortcuts don't require CGEventTap
        XCTAssertFalse(requiresCGEventTapBackend(keyCode: kVK_Space, modifiers: Int(controlKey) | Int(shiftKey)))
        XCTAssertFalse(requiresCGEventTapBackend(keyCode: kVK_ANSI_A, modifiers: Int(cmdKey)))
    }

    // MARK: - Shortcut Description Tests

    func testDescribeShortcutNormal() {
        // Ctrl+Shift+Space (default)
        let desc = describeShortcut(keyCode: kVK_Space, modifiers: Int(controlKey) | Int(shiftKey))
        XCTAssertEqual(desc, "⌃⇧Space")

        // Cmd+A
        let cmdA = describeShortcut(keyCode: kVK_ANSI_A, modifiers: Int(cmdKey))
        XCTAssertEqual(cmdA, "⌘A")

        // Option+Z
        let optZ = describeShortcut(keyCode: kVK_ANSI_Z, modifiers: Int(optionKey))
        XCTAssertEqual(optZ, "⌥Z")
    }

    func testDescribeShortcutWithFn() {
        // Fn+Z
        let fnZ = describeShortcut(keyCode: kVK_ANSI_Z, modifiers: kModsFnBit)
        XCTAssertEqual(fnZ, "Fn+Z")

        // Fn+Cmd+Z
        let fnCmdZ = describeShortcut(keyCode: kVK_ANSI_Z, modifiers: kModsFnBit | Int(cmdKey))
        XCTAssertEqual(fnCmdZ, "⌘Fn+Z")
    }

    func testDescribeShortcutModifiersOnly() {
        // Command alone
        let cmdOnly = describeShortcut(keyCode: kKeyCodeModifiersOnly, modifiers: Int(cmdKey))
        XCTAssertEqual(cmdOnly, "⌘")

        // Cmd+Option
        let cmdOpt = describeShortcut(keyCode: kKeyCodeModifiersOnly, modifiers: Int(cmdKey) | Int(optionKey))
        XCTAssertEqual(cmdOpt, "⌥⌘")

        // Fn alone
        let fnOnly = describeShortcut(keyCode: kKeyCodeModifiersOnly, modifiers: kModsFnBit)
        XCTAssertEqual(fnOnly, "Fn")
    }

    // MARK: - Constants Tests

    func testShortcutConstants() {
        // Fn bit should not conflict with Carbon modifier bits
        // Carbon uses: cmdKey (256/0x100), shiftKey (512/0x200), optionKey (2048/0x800), controlKey (4096/0x1000)
        XCTAssertEqual(kModsFnBit, 1 << 16)
        XCTAssertTrue(kModsFnBit > Int(controlKey))

        // Modifiers-only sentinel
        XCTAssertEqual(kKeyCodeModifiersOnly, -1)
    }
}
