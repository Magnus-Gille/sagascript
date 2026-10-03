//! Shared `--speakers` / `--min-speakers` / `--max-speakers` / `--force-speakers` flags (issue #305).

use clap::Args;
use sagascript_core::speaker_hint::SpeakerCountHint;

/// Speaker-count hint flags, flattened into every command that clusters speakers.
#[derive(Args, Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SpeakerCountArgs {
    /// Expected number of speakers, counting everyone who speaks, including a chair or moderator.
    /// Voices that are clearly different are not merged to reach it (the result says if it fell
    /// short); if you are unsure of the count, use --min-speakers.
    #[arg(
        long,
        value_name = "N",
        value_parser = parse_speaker_count,
        conflicts_with_all = ["min_speakers", "max_speakers", "force_speakers"]
    )]
    pub speakers: Option<usize>,

    /// Fewest speakers to expect: if fewer are found, voices are split to reach this many.
    #[arg(long, value_name = "N", value_parser = parse_speaker_count, conflicts_with = "force_speakers")]
    pub min_speakers: Option<usize>,

    /// Most speakers to expect: if more are found, close voices are merged, but clearly different
    /// voices stay separate and the result says if the maximum was not reached.
    #[arg(long, value_name = "N", value_parser = parse_speaker_count, conflicts_with = "force_speakers")]
    pub max_speakers: Option<usize>,

    /// Exactly this many speakers, even if that merges clearly different people (an undercount
    /// puts one person's words under another's name).
    #[arg(long, value_name = "N", value_parser = parse_speaker_count)]
    pub force_speakers: Option<usize>,
}

impl SpeakerCountArgs {
    /// The validated hint, or `None` when no flag was given.
    pub fn hint(&self) -> Result<Option<SpeakerCountHint>, String> {
        match self.force_speakers {
            Some(n) => Ok(Some(SpeakerCountHint::forced(n))),
            None => SpeakerCountHint::from_parts(self.speakers, self.min_speakers, self.max_speakers, false),
        }
    }
}

fn parse_speaker_count(value: &str) -> Result<usize, String> {
    let count = value
        .parse::<usize>()
        .map_err(|_| format!("'{value}' is not a whole number of speakers"))?;
    if count == 0 {
        return Err("the number of speakers must be at least 1".to_string());
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Parser)]
    struct Cli {
        #[command(flatten)]
        speakers: SpeakerCountArgs,
    }

    fn parse(args: &[&str]) -> Result<Option<SpeakerCountHint>, String> {
        let argv = std::iter::once("x").chain(args.iter().copied());
        Cli::try_parse_from(argv)
            .map_err(|e| e.to_string())?
            .speakers
            .hint()
    }

    #[test]
    fn no_flags_means_no_hint() {
        assert_eq!(parse(&[]), Ok(None));
    }

    #[test]
    fn exact_and_range_flags_build_hints() {
        assert_eq!(parse(&["--speakers", "3"]), Ok(Some(SpeakerCountHint::exact(3))));
        assert_eq!(
            parse(&["--min-speakers", "2", "--max-speakers", "4"]),
            Ok(Some(SpeakerCountHint::range(Some(2), Some(4))))
        );
        assert_eq!(parse(&["--min-speakers", "2"]), Ok(Some(SpeakerCountHint::range(Some(2), None))));
        assert_eq!(parse(&["--max-speakers", "2"]), Ok(Some(SpeakerCountHint::range(None, Some(2)))));
        assert_eq!(
            parse(&["--min-speakers", "3", "--max-speakers", "3"]),
            Ok(Some(SpeakerCountHint::range(Some(3), Some(3))))
        );
    }

    #[test]
    fn force_speakers_is_an_exclusive_forced_exact_count() {
        assert_eq!(parse(&["--force-speakers", "2"]), Ok(Some(SpeakerCountHint::forced(2))));
        for other in ["--speakers", "--min-speakers", "--max-speakers"] {
            assert!(parse(&["--force-speakers", "2", other, "2"]).is_err(), "{other}");
        }
        assert!(parse(&["--force-speakers", "0"]).is_err());
    }

    #[test]
    fn rejects_zero_and_garbage() {
        for flag in ["--speakers", "--min-speakers", "--max-speakers"] {
            assert!(parse(&[flag, "0"]).is_err(), "{flag} 0");
            assert!(parse(&[flag, "-1"]).is_err(), "{flag} -1");
            assert!(parse(&[flag, "two"]).is_err(), "{flag} two");
            assert!(parse(&[flag, "1.5"]).is_err(), "{flag} 1.5");
        }
    }

    #[test]
    fn rejects_min_above_max() {
        let error = parse(&["--min-speakers", "4", "--max-speakers", "2"]).unwrap_err();
        assert!(error.contains("must not exceed"), "{error}");
    }

    #[test]
    fn exact_count_excludes_min_and_max() {
        assert!(parse(&["--speakers", "2", "--min-speakers", "1"]).is_err());
        assert!(parse(&["--speakers", "2", "--max-speakers", "3"]).is_err());
    }
}
