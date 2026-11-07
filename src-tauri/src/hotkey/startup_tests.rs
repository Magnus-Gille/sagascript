use super::*;
use sagascript_core::settings::{HotkeyProfile, Language, Settings};
use std::collections::VecDeque;

#[derive(Debug, Clone, PartialEq, Eq)]
enum Operation {
    Register,
    Unregister,
}

#[derive(Debug)]
struct ScriptedCall {
    operation: Operation,
    shortcuts: Vec<String>,
    result: Result<(), String>,
}

#[derive(Debug)]
struct StrictRegistrar {
    script: VecDeque<ScriptedCall>,
    calls: Vec<(Operation, Vec<String>)>,
}

impl StrictRegistrar {
    fn new(script: Vec<ScriptedCall>) -> Self {
        Self {
            script: script.into(),
            calls: Vec::new(),
        }
    }

    fn run(&mut self, operation: Operation, shortcuts: &[String]) -> Result<(), String> {
        let expected = self
            .script
            .pop_front()
            .unwrap_or_else(|| panic!("unexpected {operation:?} call for {shortcuts:?}"));
        assert_eq!(expected.operation, operation);
        assert_eq!(expected.shortcuts, shortcuts);
        self.calls.push((operation, shortcuts.to_vec()));
        expected.result
    }

    fn assert_consumed(self) {
        assert!(
            self.script.is_empty(),
            "unconsumed scripted calls: {self:?}"
        );
    }
}

impl hotkey::transition::Registrar for StrictRegistrar {
    fn register(&mut self, shortcuts: &[String]) -> Result<(), String> {
        self.run(Operation::Register, shortcuts)
    }

    fn unregister(&mut self, shortcuts: &[String]) -> Result<(), String> {
        self.run(Operation::Unregister, shortcuts)
    }
}

#[cfg(target_os = "macos")]
fn scripted_call(
    operation: Operation,
    shortcuts: &[&str],
    result: Result<(), &str>,
) -> ScriptedCall {
    ScriptedCall {
        operation,
        shortcuts: shortcuts
            .iter()
            .map(|shortcut| (*shortcut).to_string())
            .collect(),
        result: result.map_err(str::to_string),
    }
}

fn profile(id: &str, shortcut: &str) -> HotkeyProfile {
    HotkeyProfile {
        id: id.to_string(),
        name: id.to_string(),
        shortcut: shortcut.to_string(),
        language: Language::English,
        push_to_talk_shortcut: None,
        toggle_shortcut: None,
    }
}

fn legacy_settings() -> Settings {
    Settings {
        hotkey: "Option+Space".to_string(),
        ..Settings::default()
    }
}

fn explicit_bindings_settings() -> Settings {
    let mut first = profile("default", "Option+Space");
    first.push_to_talk_shortcut = Some("Control+Option+P".to_string());
    first.toggle_shortcut = Some("Control+Option+T".to_string());

    let mut second = profile("secondary", "Shift+F6");
    second.push_to_talk_shortcut = Some("Control+Shift+F6".to_string());

    Settings {
        hotkey_profiles: vec![first, second],
        ..Settings::default()
    }
}

#[test]
fn prepare_keeps_a_valid_legacy_shortcut() {
    let settings = legacy_settings();
    let original = settings.clone();

    let plan = prepare(&settings);

    assert_eq!(plan.shortcuts, vec!["Option+Space"]);
    assert_eq!(plan.validation_error, None);
    assert_eq!(settings.hotkey, original.hotkey);
    assert_eq!(settings.hotkey_profiles, original.hotkey_profiles);
}

#[test]
fn prepare_keeps_all_explicit_profile_bindings_in_order() {
    let settings = explicit_bindings_settings();

    let plan = prepare(&settings);

    assert_eq!(
        plan.shortcuts,
        vec!["Control+Option+P", "Control+Option+T", "Control+Shift+F6"]
    );
    assert_eq!(plan.validation_error, None);
}

#[test]
fn invalid_secondary_profile_uses_fallback_without_mutating_settings() {
    let mut settings = legacy_settings();
    let mut invalid_secondary = profile("secondary", "Shift+F6");
    invalid_secondary.shortcut = "not-a-hotkey".to_string();
    settings.hotkey_profiles = vec![profile("default", "Option+Space"), invalid_secondary];
    let original = settings.clone();

    let plan = prepare(&settings);

    assert_eq!(plan.shortcuts, vec![SAFE_FALLBACK_HOTKEY]);
    assert!(plan.validation_error.is_some());
    assert_eq!(settings.hotkey, original.hotkey);
    assert_eq!(settings.hotkey_profiles, original.hotkey_profiles);
}

#[test]
fn duplicate_binding_uses_fallback_without_mutating_settings() {
    let mut first = profile("default", "Option+Space");
    first.push_to_talk_shortcut = Some("Control+Option+P".to_string());
    let mut second = profile("secondary", "Shift+F6");
    second.toggle_shortcut = Some("Control+Option+P".to_string());
    let settings = Settings {
        hotkey_profiles: vec![first, second],
        ..Settings::default()
    };
    let original = settings.clone();

    let plan = prepare(&settings);

    assert_eq!(plan.shortcuts, vec![SAFE_FALLBACK_HOTKEY]);
    assert!(plan
        .validation_error
        .as_deref()
        .is_some_and(|message| message.contains("Duplicate hotkey")));
    assert_eq!(settings.hotkey, original.hotkey);
    assert_eq!(settings.hotkey_profiles, original.hotkey_profiles);
}

#[cfg(target_os = "macos")]
#[test]
fn macos_reserved_shortcuts_use_fallback_and_report_validation_error() {
    for (shortcut, reserved_action) in [
        ("Super+Q", "reserved for Quit on macOS"),
        ("Super+X", "reserved for Cut on macOS"),
        ("Super+B", "reserved for Bold Text on macOS"),
    ] {
        let mut settings = legacy_settings();
        settings.hotkey = shortcut.to_string();
        let original = settings.clone();
        let plan = prepare(&settings);
        let validation_error = plan.validation_error.clone().unwrap();
        assert!(validation_error.contains(reserved_action));
        assert_eq!(plan.shortcuts, vec![SAFE_FALLBACK_HOTKEY]);

        let mut registrar = StrictRegistrar::new(vec![scripted_call(
            Operation::Register,
            &[SAFE_FALLBACK_HOTKEY],
            Ok(()),
        )]);
        let result = apply(plan, &mut registrar);
        registrar.assert_consumed();

        assert_eq!(result.registration_error, None);
        assert_eq!(
            result.health_error,
            Some(format!(
                "{validation_error}; using safe fallback '{SAFE_FALLBACK_HOTKEY}'"
            ))
        );
        assert_eq!(
            result.operational_hotkey,
            hotkey::OperationalHotkey::registered(SAFE_FALLBACK_HOTKEY)
        );
        assert_eq!(settings.hotkey, original.hotkey);
    }
}

#[derive(Clone, Copy)]
enum Configuration {
    Valid,
    ExplicitBindings,
    InvalidSecondary,
    DuplicateBinding,
}

impl Configuration {
    fn settings(self) -> Settings {
        match self {
            Self::Valid => legacy_settings(),
            Self::ExplicitBindings => explicit_bindings_settings(),
            Self::InvalidSecondary => {
                let mut secondary = profile("secondary", "Shift+F6");
                secondary.shortcut = "not-a-hotkey".to_string();
                Settings {
                    hotkey_profiles: vec![profile("default", "Option+Space"), secondary],
                    ..Settings::default()
                }
            }
            Self::DuplicateBinding => {
                let mut first = profile("default", "Option+Space");
                first.push_to_talk_shortcut = Some("Control+Option+P".to_string());
                let mut second = profile("secondary", "Shift+F6");
                second.toggle_shortcut = Some("Control+Option+P".to_string());
                Settings {
                    hotkey_profiles: vec![first, second],
                    ..Settings::default()
                }
            }
        }
    }

    fn expected_shortcuts(self) -> Vec<String> {
        let shortcuts: &[&str] = match self {
            Self::Valid => &["Option+Space"],
            Self::ExplicitBindings => &["Control+Option+P", "Control+Option+T", "Control+Shift+F6"],
            Self::InvalidSecondary | Self::DuplicateBinding => &[SAFE_FALLBACK_HOTKEY],
        };
        shortcuts
            .iter()
            .map(|shortcut| (*shortcut).to_string())
            .collect()
    }

    fn name(self) -> &'static str {
        match self {
            Self::Valid => "valid legacy configuration",
            Self::ExplicitBindings => "valid explicit multi-profile configuration",
            Self::InvalidSecondary => "invalid secondary profile",
            Self::DuplicateBinding => "duplicate binding",
        }
    }
}

#[derive(Clone, Copy)]
enum RegistrationOutcome {
    Success,
    RegistrationFailureCleanupSuccess,
    RegistrationFailureCleanupFailure,
}

impl RegistrationOutcome {
    fn script(self, shortcuts: &[String]) -> Vec<ScriptedCall> {
        match self {
            Self::Success => vec![ScriptedCall {
                operation: Operation::Register,
                shortcuts: shortcuts.to_vec(),
                result: Ok(()),
            }],
            Self::RegistrationFailureCleanupSuccess => vec![
                ScriptedCall {
                    operation: Operation::Register,
                    shortcuts: shortcuts.to_vec(),
                    result: Err("registration rejected".to_string()),
                },
                ScriptedCall {
                    operation: Operation::Unregister,
                    shortcuts: shortcuts.to_vec(),
                    result: Ok(()),
                },
            ],
            Self::RegistrationFailureCleanupFailure => vec![
                ScriptedCall {
                    operation: Operation::Register,
                    shortcuts: shortcuts.to_vec(),
                    result: Err("registration rejected".to_string()),
                },
                ScriptedCall {
                    operation: Operation::Unregister,
                    shortcuts: shortcuts.to_vec(),
                    result: Err("cleanup rejected".to_string()),
                },
            ],
        }
    }

    fn expected_calls(self, shortcuts: &[String]) -> Vec<(Operation, Vec<String>)> {
        let mut calls = vec![(Operation::Register, shortcuts.to_vec())];
        if !matches!(self, Self::Success) {
            calls.push((Operation::Unregister, shortcuts.to_vec()));
        }
        calls
    }

    fn name(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::RegistrationFailureCleanupSuccess => "registration failure and cleanup success",
            Self::RegistrationFailureCleanupFailure => "registration and cleanup failure",
        }
    }
}

#[test]
fn startup_registration_matrix_cleans_only_failed_attempts_and_reports_health() {
    let configurations = [
        Configuration::Valid,
        Configuration::ExplicitBindings,
        Configuration::InvalidSecondary,
        Configuration::DuplicateBinding,
    ];
    let outcomes = [
        RegistrationOutcome::Success,
        RegistrationOutcome::RegistrationFailureCleanupSuccess,
        RegistrationOutcome::RegistrationFailureCleanupFailure,
    ];

    for configuration in configurations {
        for outcome in outcomes {
            let settings = configuration.settings();
            let original = settings.clone();
            let plan = prepare(&settings);
            let shortcuts = configuration.expected_shortcuts();
            assert_eq!(
                plan.shortcuts,
                shortcuts,
                "{} prepared shortcuts",
                configuration.name()
            );
            let validation_error = plan.validation_error.clone();
            let mut registrar = StrictRegistrar::new(outcome.script(&shortcuts));

            let result = apply(plan, &mut registrar);
            let actual_calls = registrar.calls.clone();
            registrar.assert_consumed();

            assert_eq!(
                actual_calls,
                outcome.expected_calls(&shortcuts),
                "{} with {}",
                configuration.name(),
                outcome.name()
            );
            assert_eq!(settings.hotkey, original.hotkey);
            assert_eq!(settings.hotkey_profiles, original.hotkey_profiles);

            let (expected_registration_error, expected_health_error, expected_operational) =
                match (validation_error, outcome) {
                    (None, RegistrationOutcome::Success) => (
                        None,
                        None,
                        hotkey::OperationalHotkey::registered_many(&shortcuts),
                    ),
                    (Some(validation), RegistrationOutcome::Success) => (
                        None,
                        Some(format!(
                            "{validation}; using safe fallback '{SAFE_FALLBACK_HOTKEY}'"
                        )),
                        hotkey::OperationalHotkey::registered_many(&shortcuts),
                    ),
                    (None, RegistrationOutcome::RegistrationFailureCleanupSuccess) => (
                        Some("registration rejected".to_string()),
                        Some("registration rejected".to_string()),
                        hotkey::OperationalHotkey::Inactive,
                    ),
                    (Some(validation), RegistrationOutcome::RegistrationFailureCleanupSuccess) => (
                        Some("registration rejected".to_string()),
                        Some(format!(
                            "{validation}; fallback '{SAFE_FALLBACK_HOTKEY}' also failed: registration rejected"
                        )),
                        hotkey::OperationalHotkey::Inactive,
                    ),
                    (None, RegistrationOutcome::RegistrationFailureCleanupFailure) => (
                        Some("registration rejected".to_string()),
                        Some("registration failed: registration rejected; partial-registration cleanup failed: cleanup rejected".to_string()),
                        hotkey::OperationalHotkey::Unknown,
                    ),
                    (Some(validation), RegistrationOutcome::RegistrationFailureCleanupFailure) => (
                        Some("registration rejected".to_string()),
                        Some(format!(
                            "{validation}; registration failed: registration rejected; partial-registration cleanup failed: cleanup rejected"
                        )),
                        hotkey::OperationalHotkey::Unknown,
                    ),
                };

            assert_eq!(
                result.registration_error,
                expected_registration_error,
                "{} with {} registration error",
                configuration.name(),
                outcome.name()
            );
            assert_eq!(
                result.health_error,
                expected_health_error,
                "{} with {} health error",
                configuration.name(),
                outcome.name()
            );
            assert_eq!(
                result.operational_hotkey,
                expected_operational,
                "{} with {}",
                configuration.name(),
                outcome.name()
            );
        }
    }
}

#[test]
fn startup_registration_matrix_preserves_explicit_bindings_on_success() {
    let settings = explicit_bindings_settings();
    let expected = vec![
        "Control+Option+P".to_string(),
        "Control+Option+T".to_string(),
        "Control+Shift+F6".to_string(),
    ];
    let mut registrar = StrictRegistrar::new(vec![ScriptedCall {
        operation: Operation::Register,
        shortcuts: expected.clone(),
        result: Ok(()),
    }]);

    let result = apply(prepare(&settings), &mut registrar);
    let calls = registrar.calls.clone();
    registrar.assert_consumed();

    assert_eq!(calls, vec![(Operation::Register, expected.clone())]);
    assert_eq!(result.registration_error, None);
    assert_eq!(result.health_error, None);
    assert_eq!(
        result.operational_hotkey,
        hotkey::OperationalHotkey::registered_many(&expected)
    );
}
