//! Shared `--speakers` / `--min-speakers` / `--max-speakers` flags (issue #305).

use clap::Args;
use sagascript_core::speaker_hint::SpeakerCountHint;

/// Speaker-count hint flags, flattened into every command that clusters speakers.
#[derive(Args, Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SpeakerCountArgs {
    /// Number of speakers in the recording: groups voices into exactly this many speakers
    /// instead of using the distance threshold.
    #[arg(
        long,
        value_name = "N",
        value_parser = parse_speaker_count,
        conflicts_with_all = ["min_speakers", "max_speakers"]
    )]
    pub speakers: Option<usize>,

    /// Fewest speakers to expect: if the threshold finds fewer, voices are split into this many.
    #[arg(long, value_name = "N", value_parser = parse_speaker_count)]
    pub min_speakers: Option<usize>,

    /// Most speakers to expect: if the threshold finds more, voices are merged down to this many.
    #[arg(long, value_name = "N", value_parser = parse_speaker_count)]
    pub max_speakers: Option<usize>,
}

impl SpeakerCountArgs {
    /// The validated hint, or `None` when no flag was given.
    pub fn hint(&self) -> Result<Option<SpeakerCountHint>, String> {
        SpeakerCountHint::from_parts(self.speakers, self.min_speakers, self.max_speakers)
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
