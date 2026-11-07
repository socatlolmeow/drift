use std::collections::HashSet;

use crate::model::Track;

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Summary {
    pub saved: usize,
    pub failed: usize,
    pub first_error: Option<String>,
}

pub fn run(
    tracks: Vec<Track>,
    mut save: impl FnMut(&Track) -> Result<(), String>,
    mut progress: impl FnMut(usize, usize) -> bool,
) -> Result<Summary, String> {
    let mut seen = HashSet::new();
    let tracks: Vec<_> = tracks
        .into_iter()
        .filter(|track| seen.insert(track.id.clone()))
        .collect();
    if tracks.is_empty() {
        return Err("No songs to download".into());
    }
    let total = tracks.len();
    let mut summary = Summary::default();
    for (index, track) in tracks.iter().enumerate() {
        if !progress(index, total) {
            return Err("Download cancelled".into());
        }
        match save(track) {
            Ok(()) => summary.saved += 1,
            Err(error) => {
                summary.failed += 1;
                summary
                    .first_error
                    .get_or_insert_with(|| format!("{}: {error}", track.title));
            }
        }
    }
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(id: &str) -> Track {
        Track {
            id: id.into(),
            title: id.into(),
            artist: "Artist".into(),
            artists: Vec::new(),
            album: None,
            duration: None,
        }
    }

    #[test]
    fn deduplicates_and_continues_after_failures() {
        let mut saved = Vec::new();
        let mut updates = Vec::new();
        let summary = run(
            vec![track("a"), track("b"), track("a"), track("c")],
            |track| {
                saved.push(track.id.clone());
                if track.id == "b" {
                    Err("unavailable".into())
                } else {
                    Ok(())
                }
            },
            |completed, total| {
                updates.push((completed, total));
                true
            },
        )
        .unwrap();
        assert_eq!(saved, ["a", "b", "c"]);
        assert_eq!(updates, [(0, 3), (1, 3), (2, 3)]);
        assert_eq!(summary.saved, 2);
        assert_eq!(summary.failed, 1);
        assert_eq!(summary.first_error.as_deref(), Some("b: unavailable"));
    }

    #[test]
    fn rejects_empty_batches() {
        assert!(run(Vec::new(), |_| panic!(), |_, _| panic!()).is_err());
    }

    #[test]
    fn stops_when_receiver_is_gone() {
        assert!(run(vec![track("a")], |_| panic!(), |_, _| false).is_err());
    }
}
