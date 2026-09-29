//! `vocab.txt` parsing: one `<piece> <id>` per line (the piece may itself contain spaces
//! only in theory; the id is always the last space-separated field).

use std::collections::HashSet;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Vocab {
    pieces: Vec<String>,
    blank_id: u32,
}

impl Vocab {
    /// Parse the file contents. Ids must be dense `0..n` (any order) and `<blk>` must exist.
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut entries: Vec<(u32, String)> = Vec::new();
        for (number, line) in text.lines().enumerate() {
            let line = line.trim_end_matches('\r');
            if line.is_empty() {
                continue;
            }
            let (piece, id) = line
                .rsplit_once(' ')
                .ok_or_else(|| format!("vocab line {} has no id", number + 1))?;
            let id: u32 = id
                .parse()
                .map_err(|_| format!("vocab line {} has a non-numeric id", number + 1))?;
            entries.push((id, piece.to_string()));
        }
        if entries.is_empty() {
            return Err("vocab is empty".into());
        }
        // Numeric sort, never lexicographic.
        entries.sort_by_key(|(id, _)| *id);
        let mut seen = HashSet::new();
        for (index, (id, _)) in entries.iter().enumerate() {
            if *id as usize != index || !seen.insert(*id) {
                return Err(format!("vocab ids are not dense (problem near id {id})"));
            }
        }
        let blank_id = entries
            .iter()
            .find(|(_, piece)| piece == "<blk>")
            .map(|(id, _)| *id)
            .ok_or_else(|| "vocab has no <blk> token".to_string())?;
        Ok(Self {
            pieces: entries.into_iter().map(|(_, piece)| piece).collect(),
            blank_id,
        })
    }

    pub fn len(&self) -> usize {
        self.pieces.len()
    }

    pub fn blank_id(&self) -> u32 {
        self.blank_id
    }

    /// Raw SentencePiece piece (word boundary `▁` preserved).
    pub fn piece(&self, id: u32) -> Option<&str> {
        self.pieces.get(id as usize).map(String::as_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_out_of_order_ids_numerically() {
        let v = Vocab::parse("<unk> 0\n▁hej 2\n<blk> 10\n").unwrap_err();
        assert!(v.contains("dense"));
        let text = (0..12)
            .rev()
            .map(|i| {
                if i == 11 {
                    "<blk> 11".to_string()
                } else {
                    format!("p{i} {i}")
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        let v = Vocab::parse(&text).unwrap();
        assert_eq!(v.len(), 12);
        assert_eq!(v.blank_id(), 11);
        assert_eq!(v.piece(10), Some("p10"));
        assert_eq!(v.piece(12), None);
    }

    #[test]
    fn keeps_word_boundary_marker_and_crlf() {
        let v = Vocab::parse("<unk> 0\r\n▁hej 1\r\n<blk> 2\r\n").unwrap();
        assert_eq!(v.piece(1), Some("▁hej"));
        assert_eq!(v.blank_id(), 2);
    }

    #[test]
    fn rejects_missing_blank_and_garbage() {
        assert!(Vocab::parse("a 0\nb 1\n").unwrap_err().contains("<blk>"));
        assert!(Vocab::parse("a x\n").unwrap_err().contains("non-numeric"));
        assert!(Vocab::parse("").is_err());
    }
}
