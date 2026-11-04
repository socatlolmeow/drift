# Changelog

Changes to Drift. Loosely follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## Unreleased

### Added

- Persistent listening history with Recently Played and Most Played views.
- `:recent` and `:mostplayed` commands, including playback, queueing, and song actions from history.
- Unified `/` search for songs, albums, and playlists. Use Up/Down to choose the search type and Ctrl+Up/Ctrl+Down to browse search history.
- A queue sidebar on wide terminals showing the current track and upcoming tracks. Press `Q` to show or hide it, or `gq` for the full queue view.
- Configurable `toggle_queue_sidebar` key action, including support for rebinding or disabling it in `[keys]`.

### Changed

- Expanded view navigation to include Recently Played and Most Played.
- Refreshed the search palette with active search-type tabs and a clearer recent-search section.
- Playlist persistence now locks the read/merge/write cycle on Unix so concurrent Drift instances do not overwrite each other's changes.

### Fixed

- Corrupt playlist files are no longer treated as missing files during save, preventing accidental replacement of damaged data.
- Ambiguous custom key sequences such as `g` and `gg` are rejected instead of silently making one binding unreachable.
- The search palette's row sizing now passes strict Clippy checks.
