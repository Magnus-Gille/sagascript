use std::collections::{BTreeMap, HashSet};

use serde::{Deserialize, Serialize};

use super::{canonical_hotkey, validate_hotkey};

/// Action to take when a presenter dictation is finished for a known app.
///
/// The app-specific mapping is deliberately opt-in.  An empty mapping keeps
/// the safe default of inserting text without synthesizing an Enter key.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PresenterFinishAction {
    #[default]
    InsertOnly,
    Return,
    CommandReturn,
}

/// Settings for the opt-in presenter hotkey mode.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PresenterConfig {
    pub finish_shortcut: String,
    pub cancel_shortcut: Option<String>,
    pub app_actions: BTreeMap<String, PresenterFinishAction>,
}

impl Default for PresenterConfig {
    fn default() -> Self {
        Self {
            finish_shortcut: "Control+Shift+Enter".to_string(),
            cancel_shortcut: None,
            app_actions: BTreeMap::new(),
        }
    }
}

impl PresenterConfig {
    pub const MAX_APP_ACTIONS: usize = 32;

    /// Validate the presenter settings without considering profile shortcuts.
    /// Cross-role collisions are checked by `Settings` only when presenter
    /// mode is active, so an inactive configuration can be prepared safely.
    pub fn validate(&self) -> Result<(), String> {
        validate_hotkey(&self.finish_shortcut)
            .map_err(|error| format!("Invalid presenter finish shortcut: {error}"))?;
        let finish = canonical_hotkey(&self.finish_shortcut)
            .map_err(|error| format!("Invalid presenter finish shortcut: {error}"))?;
        let mut shortcuts = HashSet::from([finish]);

        if let Some(cancel) = &self.cancel_shortcut {
            validate_hotkey(cancel)
                .map_err(|error| format!("Invalid presenter cancel shortcut: {error}"))?;
            let canonical = canonical_hotkey(cancel)
                .map_err(|error| format!("Invalid presenter cancel shortcut: {error}"))?;
            if !shortcuts.insert(canonical) {
                return Err("Presenter finish and cancel shortcuts must differ".to_string());
            }
        }

        for app_id in self.app_actions.keys() {
            if app_id.is_empty() {
                return Err("Presenter app identifiers must not be empty".to_string());
            }
            if app_id.len() > 512 {
                return Err("Presenter app identifiers must be 512 bytes or fewer".to_string());
            }
            if app_id.chars().any(char::is_control) {
                return Err(
                    "Presenter app identifiers must not contain control characters".to_string(),
                );
            }
        }

        if self.app_actions.len() > Self::MAX_APP_ACTIONS {
            return Err(format!(
                "Presenter app actions must contain {} entries or fewer",
                Self::MAX_APP_ACTIONS
            ));
        }

        Ok(())
    }
}
