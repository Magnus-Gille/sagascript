use super::*;

#[test]
fn second_instance_reveals_settings_except_for_background_startup() {
    assert!(second_instance_requests_settings(&[
        "sagascript".to_string()
    ]));
    assert!(second_instance_requests_settings(&[
        "sagascript".to_string(),
        sagascript_cli::open::GUI_OPEN_ARG.to_string()
    ]));
    assert!(second_instance_requests_settings(&[
        sagascript_cli::open::GUI_OPEN_ARG.to_string()
    ]));
    assert!(!second_instance_requests_settings(&[
        "sagascript".to_string(),
        sagascript_cli::open::GUI_BACKGROUND_ARG.to_string()
    ]));
    assert!(!second_instance_requests_settings(&[
        sagascript_cli::open::GUI_BACKGROUND_ARG.to_string(),
        "future-metadata".to_string()
    ]));
    let update_args = [
        "sagascript".to_string(),
        sagascript_cli::open::GUI_UPDATE_ARG.to_string(),
    ];
    assert!(!second_instance_requests_settings(&update_args));
    assert!(second_instance_requests_update(&update_args));
}

#[test]
fn completed_onboarding_normal_launch_opens_settings() {
    assert_eq!(
        initial_window_request(true, GuiLaunchMode::Standard),
        InitialWindowRequest::Settings
    );
}

#[test]
fn dictation_state_never_turns_a_reopen_event_into_a_settings_window() {
    assert!(should_reveal_for_reopen(AppState::Idle));
    assert!(!should_reveal_for_reopen(AppState::Recording));
    assert!(!should_reveal_for_reopen(AppState::Transcribing));
}

#[test]
fn explicit_open_starts_on_the_settings_view() {
    assert_eq!(
        initial_window_request(true, GuiLaunchMode::ShowSettings),
        InitialWindowRequest::Settings
    );
}

#[test]
fn completed_onboarding_background_launch_stays_hidden() {
    assert_eq!(
        initial_window_request(true, GuiLaunchMode::Background),
        InitialWindowRequest::Hidden
    );
}

#[test]
fn update_launch_stays_hidden_until_the_updater_restarts_the_app() {
    assert_eq!(
        initial_window_request(true, GuiLaunchMode::InstallUpdate),
        InitialWindowRequest::Hidden
    );
}

#[test]
fn incomplete_onboarding_starts_on_the_onboarding_view_even_when_headless() {
    assert_eq!(
        initial_window_request(false, GuiLaunchMode::Background),
        InitialWindowRequest::Onboarding
    );
}

#[test]
fn version_and_help_flags_request_console_output() {
    use std::ffi::OsString;
    let wants = |args: &[&str]| {
        cli_args_request_console_output(&args.iter().map(OsString::from).collect::<Vec<_>>())
    };

    assert!(wants(&["sagascript", "--version"]));
    assert!(wants(&["sagascript", "-V"]));
    assert!(wants(&["sagascript", "--help"]));
    assert!(wants(&["sagascript", "-h"]));
    assert!(wants(&["sagascript", "help"]));
    assert!(wants(&["sagascript", "transcribe", "--help"]));
    // Bare launch, GUI markers and ordinary subcommands never attach.
    assert!(!wants(&["sagascript"]));
    assert!(!wants(&[
        "sagascript",
        sagascript_cli::open::GUI_BACKGROUND_ARG
    ]));
    assert!(!wants(&["sagascript", "transcribe", "file.wav"]));
    // argv[0] alone is never an argument.
    assert!(!wants(&["--version"]));
}

#[test]
fn private_gui_markers_are_accepted_only_as_the_sole_argument() {
    use std::ffi::OsString;

    let mode = |args: &[&str]| gui_launch_mode(args.iter().map(OsString::from).collect::<Vec<_>>());

    assert_eq!(mode(&["sagascript"]), GuiLaunchMode::Standard);
    assert_eq!(
        mode(&["sagascript", sagascript_cli::open::GUI_OPEN_ARG]),
        GuiLaunchMode::ShowSettings
    );
    assert_eq!(
        mode(&["sagascript", sagascript_cli::open::GUI_BACKGROUND_ARG]),
        GuiLaunchMode::Background
    );
    assert_eq!(
        mode(&["sagascript", sagascript_cli::open::GUI_UPDATE_ARG]),
        GuiLaunchMode::InstallUpdate
    );
    assert_eq!(
        mode(&["sagascript", "config", sagascript_cli::open::GUI_OPEN_ARG]),
        GuiLaunchMode::Standard
    );
    assert_eq!(
        mode(&["sagascript", "presenter", "start"]),
        GuiLaunchMode::Standard
    );
}

#[test]
fn retired_presenter_markers_cannot_bypass_cli_or_reveal_settings() {
    for marker in [
        "--presenter-start",
        "--presenter-start=swedish",
        "--presenter-finish",
        "--presenter-cancel",
        "--presenter-unknown",
    ] {
        assert_eq!(
            gui_launch_mode(["sagascript".into(), marker.into()]),
            GuiLaunchMode::Standard,
        );
        assert!(!second_instance_requests_settings(&[marker.to_string()]));
        assert!(!second_instance_requests_settings(&[
            "sagascript".to_string(),
            marker.to_string(),
        ]));
        assert!(!second_instance_requests_settings(&[
            "/Applications/Sagascript.app/Contents/MacOS/sagascript".to_string(),
            marker.to_string(),
            "unexpected".to_string(),
        ]));
    }
}

#[cfg(unix)]
#[test]
fn ordinary_non_text_cli_argument_stays_on_standard_cli_path() {
    use std::os::unix::ffi::OsStringExt;

    let args = vec![
        std::ffi::OsString::from("sagascript"),
        std::ffi::OsString::from_vec(vec![b'/', b't', b'm', b'p', 0x80]),
    ];
    assert_eq!(gui_launch_mode(args), GuiLaunchMode::Standard);
}

#[test]
fn normal_gui_launches_never_check_for_updates_implicitly() {
    for mode in [
        GuiLaunchMode::Standard,
        GuiLaunchMode::ShowSettings,
        GuiLaunchMode::Background,
    ] {
        assert!(!should_install_update_at_launch(mode));
    }
    assert!(should_install_update_at_launch(
        GuiLaunchMode::InstallUpdate
    ));
}
