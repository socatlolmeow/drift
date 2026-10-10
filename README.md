<div align="center">

<h1>Drift</h1>

[![Stars](https://img.shields.io/github/stars/socatlolmeow/drift?style=flat-square&logo=github)](https://github.com/socatlolmeow/drift/stargazers)
![License](https://img.shields.io/github/license/socatlolmeow/drift?style=flat-square)

### A music streaming client using Vim keybinds built in Rust.

Stream your favourite music from YouTube Music — all in one client, in your terminal.

</div>

---

<div align="center">
  <img src="https://github.com/socatlolmeow/drift/blob/main/examples/showcase.gif?raw=true" alt="Drift showcase" />
</div>

---

## Features

- **YouTube Music streaming** — search and stream tracks directly from YouTube Music via `yt-dlp` and `mpv`
- **Vim-style keybinds** — navigate with `j`/`k`, enter commands with `:`, quit with `q`
- **Queue management** — add, remove, reorder, and clear tracks in your queue
- **Playlists** — save, load, append, and delete named playlists; import from YouTube/YouTube Music URLs
- **Shuffle & repeat** — toggle shuffle and cycle through off / repeat-all / repeat-one modes
- **Volume & seek control** — adjust volume and seek with configurable step sizes
- **Session restore** — automatically restores your queue and playback position on next launch
- **Artist search** — drill into an artist's discography from the results view
- **Command palette** — full `:command` interface (`:search`, `:play`, `:volume`, `:seek`, `:save`, `:load`, `:import`, and more)
- **Autoplay** — optionally keep playing through results when the queue ends
- **TOML config** — persistent config at `~/.config/drift/config.toml` (your platform's config directory); drift never rewrites it, so comments are preserved

---

## Installation

### Dependencies

- [mpv](https://mpv.io/) — audio playback
- [yt-dlp](https://github.com/yt-dlp/yt-dlp) — YouTube stream extraction
- [Rust](https://rustup.rs/) (build only)

Install them with your package manager, e.g. on Arch:
```
sudo pacman -S mpv yt-dlp
```

### Build & install

```bash
git clone https://github.com/socatlolmeow/drift.git
cd drift
cargo build --release
```

Then copy the binary to somewhere on your `$PATH`:
```bash
sudo cp target/release/drift /usr/local/bin/
```

Or install directly with cargo:
```bash
cargo install --path .
```

This installs `drift` to `~/.cargo/bin/`, which is on your `$PATH` if you installed Rust via [rustup](https://rustup.rs/).

---

## Star History

[![Star History Chart](https://api.star-history.com/svg?repos=socatlolmeow/drift&type=Date)](https://star-history.com/#socatlolmeow/drift)

---

## License

[Drift](https://github.com/socatlolmeow/drift) is FOSS, released under the [GNU General Public License version 3 or later](https://www.gnu.org/licenses/gpl-3.0.html).
