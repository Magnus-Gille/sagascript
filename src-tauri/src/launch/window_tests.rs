use super::*;

#[derive(Default)]
struct MockMainWindow {
    operations: std::cell::RefCell<Vec<&'static str>>,
    fail_at: Option<&'static str>,
}

impl MainWindowVisibility for MockMainWindow {
    fn activate_app(&self) {
        self.operations.borrow_mut().push("activate_app");
    }

    fn reset_to_normal_level(&self) -> Result<(), tauri::Error> {
        self.operations.borrow_mut().push("reset_to_normal_level");
        if self.fail_at == Some("reset_to_normal_level") {
            Err(tauri::Error::from(std::io::Error::other(
                "window level failed",
            )))
        } else {
            Ok(())
        }
    }

    fn unminimize(&self) -> Result<(), tauri::Error> {
        self.operations.borrow_mut().push("unminimize");
        if self.fail_at == Some("unminimize") {
            Err(tauri::Error::from(std::io::Error::other("restore failed")))
        } else {
            Ok(())
        }
    }

    fn show(&self) -> Result<(), tauri::Error> {
        self.operations.borrow_mut().push("show");
        if self.fail_at == Some("show") {
            Err(tauri::Error::from(std::io::Error::other("show failed")))
        } else {
            Ok(())
        }
    }

    fn set_focus(&self) -> Result<(), tauri::Error> {
        self.operations.borrow_mut().push("set_focus");
        if self.fail_at == Some("set_focus") {
            Err(tauri::Error::from(std::io::Error::other("focus failed")))
        } else {
            Ok(())
        }
    }
}

#[test]
fn main_window_reveal_restores_before_showing_and_focusing() {
    let window = MockMainWindow::default();

    reveal_existing_main_window(&window).unwrap();

    assert_eq!(
        *window.operations.borrow(),
        [
            "reset_to_normal_level",
            "unminimize",
            "show",
            "set_focus",
            "activate_app"
        ]
    );
}

#[test]
fn main_window_reveal_reports_the_failed_step_and_stops() {
    let window = MockMainWindow {
        fail_at: Some("show"),
        ..Default::default()
    };

    let error = reveal_existing_main_window(&window).unwrap_err();

    assert!(matches!(error, WindowError::Show(_)), "{error:?}");
    let text = error.to_string();
    assert!(text.contains("show main window"), "{text}");
    assert!(text.contains("show failed"), "{text}");
    assert_eq!(
        *window.operations.borrow(),
        ["reset_to_normal_level", "unminimize", "show"]
    );
}

#[test]
fn main_window_reveal_continues_if_normal_window_level_cannot_be_restored() {
    let window = MockMainWindow {
        fail_at: Some("reset_to_normal_level"),
        ..Default::default()
    };

    reveal_existing_main_window(&window).unwrap();

    assert_eq!(
        *window.operations.borrow(),
        [
            "reset_to_normal_level",
            "unminimize",
            "show",
            "set_focus",
            "activate_app"
        ]
    );
}

#[test]
fn the_record_window_is_the_one_the_frontend_routes() {
    // `App.svelte` shows the recorder for `?window=record`.
    assert_eq!(RECORD_WINDOW_LABEL, "record");
    assert_eq!(RECORD_WINDOW_URL, "index.html?window=record");
}

#[test]
fn record_window_height_fits_the_screen() {
    assert_eq!(record_window_height(None), 680.0);
    assert_eq!(record_window_height(Some(1440.0)), 680.0);
    assert_eq!(record_window_height(Some(800.0)), 680.0);
    // A 768p laptop: 90% of the screen.
    assert!((record_window_height(Some(700.0)) - 630.0).abs() < 1e-9);
    // Never below the minimum.
    assert_eq!(record_window_height(Some(400.0)), 520.0);
}
