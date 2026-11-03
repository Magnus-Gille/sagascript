use super::Language;

/// One of Klang AI's Pianissimo checkpoints. Each serves exactly one language;
/// they share Parakeet TDT v3's architecture and tokenizer, so the engine
/// hosts load every one of them the same way.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PianissimoModel {
    Swedish,
    Danish,
}

impl PianissimoModel {
    pub const ALL: [PianissimoModel; 2] = [PianissimoModel::Swedish, PianissimoModel::Danish];

    /// Identifier in the settings file, the CLI and the frontend.
    pub fn id(self) -> &'static str {
        match self {
            PianissimoModel::Swedish => "pianissimo-sv",
            PianissimoModel::Danish => "pianissimo-da",
        }
    }

    /// The model with this identifier, if any.
    pub fn from_id(id: &str) -> Option<PianissimoModel> {
        Self::ALL.into_iter().find(|model| model.id() == id)
    }

    /// The only language this model transcribes.
    pub fn language(self) -> Language {
        match self {
            PianissimoModel::Swedish => Language::Swedish,
            PianissimoModel::Danish => Language::Danish,
        }
    }

    /// The Pianissimo model for `language`, if Klang publishes one.
    pub fn for_language(language: Language) -> Option<PianissimoModel> {
        Self::ALL
            .into_iter()
            .find(|model| model.language() == language)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_round_trip_and_name_their_language() {
        for model in PianissimoModel::ALL {
            assert_eq!(PianissimoModel::from_id(model.id()), Some(model));
            assert_eq!(PianissimoModel::for_language(model.language()), Some(model));
            assert!(model.id().ends_with(model.language().id()));
        }
        assert_eq!(PianissimoModel::from_id("pianissimo"), None);
        assert_eq!(PianissimoModel::for_language(Language::Finnish), None);
        assert_eq!(PianissimoModel::for_language(Language::Auto), None);
    }
}
