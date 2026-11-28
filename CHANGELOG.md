# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Media keys and system media controls on Windows (System Media Transport
  Controls) and macOS (Now Playing), in addition to MPRIS on Linux.
- Sleep timer: `:sleep 30` stops playback after 30 minutes, `:sleep song`
  stops it when the current song ends, and `:sleep off` cancels. The time left
  is shown in the player bar.
- Shell completions: `drift --completions <shell>` prints a completion script
  for bash, zsh, fish, elvish or powershell.
- A fourth fullscreen layout, the card view: press `F` once more after the
  stage view to get a wide card with the cover on the left and the title,
  artist, album, a small lyric window and the progress bar on the right.
  `F` now cycles lyrics, cover, stage, card, then back to the player.
- A third fullscreen layout, the stage view: press `F` once more after the
  cover view to get a centered cover, title, artist and progress bar with a
  small fading window of lyrics underneath. `F` now cycles lyrics, cover,
  stage, then back to the player.
- Last.fm scrobbling. Set `api_key` and `api_secret` under the new `[lastfm]`
  section, then run `drift --lastfm-login` once (`--lastfm-logout` signs out).
  Plays are scrobbled by Last.fm's rules, the current song is sent as "now
  playing", and scrobbles that cannot be delivered are queued on disk and
  retried.
- First-run setup. When there is no `config.toml`, drift opens a short
  full-screen setup that asks for the accent color, volume, session restore,
  streaming or downloading, the download folder, browser cookies, Last.fm
  (including sign-in) and Discord presence. It then writes `config.toml` and
  starts the player. Every question has a default, and `Esc` on the first
  screen skips the setup.
- `:speed` (alias `:spd`) sets the playback speed: `:speed 1.25`, `:speed +0.25`,
  `:speed reset`. The range is 0.25x to 4x, pitch is kept, and the speed stays
  for the rest of the session.
- Liked Songs: press `f` (or use `:like` / `:unlike`) to like a song. Liked
  songs are marked with a ♥ in every list, in the player bar and in the cover
  view, and are kept in a Liked Songs playlist that is always listed first.
  `f` also works in fullscreen lyrics, and in the main view it can be rebound
  with the new `like` action.

### Changed

- Command-line arguments are now parsed with clap. The existing flags and
  `drift [QUERY...]` work as before, and `--help` output is generated.
- Without a structured artist list (older saves), a credit is only split on
  `feat.`, `ft.` and `×`. Names like "Hall & Oates" or "Blood, Sweat & Tears"
  are no longer broken into pieces, and the hardcoded list of exceptions is gone.
- Added timeouts for yt-dlp subprocesses.

### Fixed

- Saving an album from a song now uses the song's own artist list when it has one.
- The cover on the Discord Rich Presence now has no black bars like before.

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
