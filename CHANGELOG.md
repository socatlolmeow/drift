# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [1.7.0] - 2026-11-15

### Added

- Tabbed help menu with six pages (Start, Navigate, Playback, Library, Menus,
  Commands). Switch pages with `←`/`→`, `h`/`l`, `Tab` or `1`-`6`; `j`/`k` still scroll. (#19)
- MPRIS documentation and setup guide. (#22)
- Config options are now documented in the README.

### Changed

- Tracks start playing much faster. Resolving a song used to take around 4-6s. (#20)
  - Streams are resolved with yt-dlp's `visionos` client, which is faster at the same quality.
  - Resolved URLs are cached for 4 hours.
  - The next queued track is resolved while the current one plays.
  - `mpv` is spawned as soon as a track is requested, not after resolving finishes.
- Updated dependencies: ratatui 0.30.2 (#23), dirs 7.0.0, souvlaki 0.9.3.
