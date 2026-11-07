# Changelog

## [Unreleased]

### Added

- Download support for saving songs permanently to disk
  - Press `D` to download current song, `gd` for highlighted, `gD` for all in collection
  - Commands: `:download`, `:download selected`, `:download all` (alias `:dl`)
  - Configure save location with `download_dir` under `[audio]` in config.toml (default: `~/Music/drift`)
  - Background downloads with progress tracking and completion summary
  - Deduplication within batch, fails don't stop remaining downloads
  - Files saved in original audio format without transcoding
  - Configurable key bindings: `download`, `download_selected`, `download_all`

- Enhanced fullscreen lyrics and cover art display
  - Current line now bold white text on dark neutral band
  - Extra breathing room around current line with surrounding lines fading
  - Consistent styling across both fullscreen views

- Playlist pagination support
  - Browse collections beyond 50-item limit (tested up to 400+ tracks)
  - Proper continuation token handling and repetition detection
