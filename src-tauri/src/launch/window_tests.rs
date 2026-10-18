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

    fn reset_to_normal_level(&self) -> Result<(), String> {
        self.operations.borrow_mut().push("reset_to_normal_level");
        if self.fail_at == Some("reset_to_normal_level") {
            Err("window level failed".to_string())
        } else {
            Ok(())
        }
    }

    fn unminimize(&self) -> Result<(), String> {
        self.operations.borrow_mut().push("unminimize");
        if self.fail_at == Some("unminimize") {
            Err("restore failed".to_string())
        } else {
            Ok(())
        }
    }

    fn show(&self) -> Result<(), String> {
        self.operations.borrow_mut().push("show");
        if self.fail_at == Some("show") {
            Err("show failed".to_string())
        } else {
            Ok(())
        }
    }

    fn set_focus(&self) -> Result<(), String> {
        self.operations.borrow_mut().push("set_focus");
        if self.fail_at == Some("set_focus") {
            Err("focus failed".to_string())
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

    assert!(error.contains("show main window"));
    assert!(error.contains("show failed"));
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
