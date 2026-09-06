#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Decision {
    Wait,
    Proven,
    Draft,
}

pub(crate) fn decide(
    target_matches: bool,
    observed_matches: bool,
    before_deadline: bool,
    modifiers_released: bool,
) -> Decision {
    if !target_matches || !before_deadline {
        Decision::Draft
    } else if observed_matches && modifiers_released {
        Decision::Proven
    } else {
        Decision::Wait
    }
}

#[cfg(test)]
mod tests {
    use super::{decide, Decision};

    #[test]
    fn decision_truth_table_covers_all_inputs() {
        for target_matches in [false, true] {
            for observed_matches in [false, true] {
                for before_deadline in [false, true] {
                    for modifiers_released in [false, true] {
                        let expected = if !target_matches || !before_deadline {
                            Decision::Draft
                        } else if observed_matches && modifiers_released {
                            Decision::Proven
                        } else {
                            Decision::Wait
                        };
                        assert_eq!(
                            decide(
                                target_matches,
                                observed_matches,
                                before_deadline,
                                modifiers_released,
                            ),
                            expected,
                            "target={target_matches} observed={observed_matches} before_deadline={before_deadline} modifiers_released={modifiers_released}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn late_observation_cannot_prove_target_that_left_and_returned() {
        assert_eq!(decide(false, true, false, false), Decision::Draft);
    }

    #[test]
    fn held_modifiers_keep_a_valid_observation_waiting() {
        assert_eq!(decide(true, true, true, false), Decision::Wait);
    }

    #[test]
    fn proof_progression_waits_for_release_and_rejects_late_or_changed_targets() {
        assert_eq!(decide(true, false, true, false), Decision::Wait);
        assert_eq!(decide(true, true, true, false), Decision::Wait);
        assert_eq!(decide(true, true, true, true), Decision::Proven);
        assert_eq!(decide(true, true, false, true), Decision::Draft);
        assert_eq!(decide(false, true, true, true), Decision::Draft);
    }
}
