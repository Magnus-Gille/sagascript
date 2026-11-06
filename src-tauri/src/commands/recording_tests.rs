use super::*;

#[test]
fn the_default_folder_is_stored_as_empty() {
    assert_eq!(recording_directory_to_store("").expect("empty"), "");
    assert_eq!(recording_directory_to_store("  \t ").expect("blank"), "");
}

#[test]
fn an_absolute_folder_is_stored_as_given_without_the_padding() {
    let dir = std::env::temp_dir();
    let given = format!("  {}  ", dir.display());
    assert_eq!(
        recording_directory_to_store(&given).expect("absolute"),
        dir.to_string_lossy()
    );
}

#[test]
fn a_leading_tilde_is_the_home_folder() {
    let Some(home) = dirs::home_dir() else {
        return;
    };
    assert_eq!(
        recording_directory_to_store("~/Takes").expect("home"),
        home.join("Takes").to_string_lossy()
    );
}

#[test]
fn a_relative_folder_is_refused_with_a_message_without_the_path() {
    let error = recording_directory_to_store("Takes/today").expect_err("relative");
    assert_eq!(error.code(), "invalid_input");
    assert!(!error.message().contains("Takes"), "{}", error.message());
}

#[test]
fn the_unfinished_transcription_command_says_it_is_unavailable() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("runtime");
    let error = runtime
        .block_on(transcribe_recording("x".into()))
        .expect_err("not yet");
    assert_eq!(error.code(), "unavailable");
}
