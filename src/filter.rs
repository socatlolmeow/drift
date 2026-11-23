use crate::model::Track;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Query {
    words: Vec<String>,
}

impl Query {
    pub fn parse(text: &str) -> Option<Query> {
        let words: Vec<String> = text.split_whitespace().map(str::to_lowercase).collect();
        (!words.is_empty()).then_some(Query { words })
    }

    pub fn matches_text(&self, text: &str) -> bool {
        let haystack = text.to_lowercase();
        self.words.iter().all(|word| haystack.contains(word.as_str()))
    }

    pub fn matches_track(&self, track: &Track) -> bool {
        let album = track.album.as_deref().unwrap_or("");
        let haystack = format!("{}\n{}\n{album}", track.title, track.artist);
        self.matches_text(&haystack)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(title: &str, artist: &str, album: Option<&str>) -> Track {
        Track {
            id: "aaaaaaaaaaa".into(),
            title: title.into(),
            artist: artist.into(),
            artists: Vec::new(),
            album: album.map(str::to_string),
            duration: None,
        }
    }

    #[test]
    fn blank_text_is_no_filter() {
        assert_eq!(Query::parse(""), None);
        assert_eq!(Query::parse("  \t "), None);
        assert!(Query::parse(" a ").is_some());
    }

    #[test]
    fn matches_substrings_ignoring_case() {
        let query = Query::parse("RAD").unwrap();
        assert!(query.matches_text("Radiohead"));
        assert!(query.matches_text("Paradise"));
        assert!(!query.matches_text("Muse"));
    }

    #[test]
    fn every_word_must_match_in_any_order() {
        let query = Query::parse("punk daft").unwrap();
        assert!(query.matches_text("Daft Punk"));
        assert!(!query.matches_text("Daft Hands"));
    }

    #[test]
    fn tracks_match_on_title_artist_and_album() {
        let song = track("Around the World", "Daft Punk", Some("Homework"));
        assert!(Query::parse("around").unwrap().matches_track(&song));
        assert!(Query::parse("daft").unwrap().matches_track(&song));
        assert!(Query::parse("homework").unwrap().matches_track(&song));
        assert!(Query::parse("daft homework world").unwrap().matches_track(&song));
        assert!(!Query::parse("discovery").unwrap().matches_track(&song));
    }

    #[test]
    fn a_word_does_not_match_across_fields() {
        let song = track("Alpha", "Beta", None);
        assert!(!Query::parse("alphabeta").unwrap().matches_track(&song));
    }

    #[test]
    fn missing_album_is_fine() {
        let song = track("Alpha", "Beta", None);
        assert!(Query::parse("beta").unwrap().matches_track(&song));
    }
}
