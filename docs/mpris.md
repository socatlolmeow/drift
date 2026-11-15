## MPRIS, playerctl and Waybar

On Linux, Drift registers itself on the session D-Bus as an [MPRIS](https://specifications.freedesktop.org/mpris-spec/latest/) player. Anything that speaks MPRIS can see what Drift is playing and control it: `playerctl`, media keys, Waybar, Polybar, eww, and the media widgets in GNOME and KDE.

This guide covers setting that up. Nothing needs to be enabled in `config.toml`.

### Requirements

- Linux with a running D-Bus **session** bus (any normal desktop session has one)
- [`playerctl`](https://github.com/altdesktop/playerctl) for the command line and key bindings

| Distro | Command |
| --- | --- |
| Arch | `sudo pacman -S playerctl` |
| Debian / Ubuntu | `sudo apt install playerctl` |
| Fedora | `sudo dnf install playerctl` |
| Nix | `nix-shell -p playerctl` |

### 1. Check that Drift shows up

Start Drift, play a song, then in another terminal run:

```bash
playerctl -l
```

You should see Drift in the list of players. In the examples below the player name is `drift`; if `playerctl -l` prints something different (for example `drift.instance12345`), use that name instead.

```bash
playerctl -p drift status      # Playing / Paused / Stopped
playerctl -p drift metadata    # title, artist, album, length, cover art URL
```

If both commands work, MPRIS is working and everything below will too.

### 2. Control Drift with playerctl

```bash
playerctl -p drift play-pause
playerctl -p drift next
playerctl -p drift previous
playerctl -p drift position 90      # seek to 1:30
playerctl -p drift position 10+     # forward 10 seconds
playerctl -p drift position 10-     # back 10 seconds
playerctl -p drift volume 0.5       # 50%
playerctl -p drift shuffle toggle
playerctl -p drift loop Playlist    # None, Track or Playlist
```

Print the current song in your own format:

```bash
playerctl -p drift metadata --format '{{ artist }} - {{ title }}'
playerctl -p drift metadata --format '{{ emoji(status) }} {{ artist }} - {{ title }} ({{ duration(position) }}/{{ duration(mpris:length) }})'
```

Drop `-p drift` to control whichever player is active. Use `-a` to target all players at once.

### 3. Media keys

Desktop environments such as GNOME, KDE, XFCE and Cinnamon route media keys to MPRIS players automatically, so there is nothing to configure.

On a standalone window manager, bind the keys to `playerctl` yourself.

**Hyprland** (`hyprland.conf`):

```ini
bindl = , XF86AudioPlay, exec, playerctl play-pause
bindl = , XF86AudioNext, exec, playerctl next
bindl = , XF86AudioPrev, exec, playerctl previous
```

**Sway** (`~/.config/sway/config`) and **i3** (`~/.config/i3/config`):

```
bindsym XF86AudioPlay exec playerctl play-pause
bindsym XF86AudioNext exec playerctl next
bindsym XF86AudioPrev exec playerctl previous
```

Without `-p`, these keys act on the most recently active player. To make that "most recent" behavior reliable when several players are open, start the `playerctld` daemon with your session:

```bash
playerctld daemon
```

### 4. Waybar

There are two ways to show Drift in Waybar. Use the built-in module for a quick setup, or a custom module for full control over the text.

#### Option A: built-in `mpris` module

In `~/.config/waybar/config.jsonc`, add `"mpris"` to one of your module lists, then define it:

```jsonc
"mpris": {
  "player": "drift",
  "format": "{player_icon} {dynamic}",
  "format-paused": "{status_icon} <i>{dynamic}</i>",
  "dynamic-order": ["artist", "title"],
  "title-len": 30,
  "artist-len": 20,
  "player-icons": {
    "default": "▶",
    "drift": "♪"
  },
  "status-icons": {
    "paused": "⏸"
  },
  "on-click": "playerctl -p drift play-pause",
  "on-click-right": "playerctl -p drift next",
  "on-scroll-up": "playerctl -p drift position 5+",
  "on-scroll-down": "playerctl -p drift position 5-"
}
```

Optional styling in `~/.config/waybar/style.css`:

```css
#mpris {
  padding: 0 10px;
}

#mpris.paused {
  opacity: 0.6;
}
```

Waybar's `mpris` module has more options (`ignored-players`, `max-length`, `tooltip-format` and others). See `man 5 waybar-mpris`.

#### Option B: custom module with playerctl

This is useful if you want your own text format, or the module to disappear when Drift isn't running. `--follow` makes `playerctl` print a new line each time the song or status changes.

```jsonc
"custom/drift": {
  "exec": "playerctl -p drift --follow metadata --format '{{ emoji(status) }} {{ artist }} - {{ title }}'",
  "format": "{}",
  "on-click": "playerctl -p drift play-pause",
  "on-click-right": "playerctl -p drift next",
  "on-click-middle": "playerctl -p drift previous",
  "max-length": 50
}
```

When Drift quits, `playerctl --follow` prints nothing and the module empties out. Add `"custom/drift"` to `modules-left`, `modules-center` or `modules-right`.

### 5. Other bars and widgets

Anything that supports MPRIS will pick Drift up on its own:

- **Polybar**: use the `custom/script` module with the same `playerctl --follow` command as above.
- **eww / AGS / Quickshell**: use the MPRIS integration these provide, or poll `playerctl metadata --format`.
- **GNOME Shell / KDE Plasma**: the media controls in the system tray or lock screen show Drift automatically.
- **Discord, scripts, notification daemons**: anything on the session bus can read Drift's state.

For a song-change notification, for example:

```bash
playerctl -p drift --follow metadata --format '{{ artist }}|{{ title }}' |
while IFS='|' read -r artist title; do
  notify-send "Now playing" "$artist - $title"
done
```

### Troubleshooting

**`playerctl -l` says "No players found".**
Drift must be running and have started playback at least once. If it still doesn't show, check that the session bus is visible to your shell: `echo $DBUS_SESSION_BUS_ADDRESS` should not be empty. This is often the problem over SSH, in `tmux` sessions started before login, or in a bare TTY. In those cases start a session bus with `dbus-run-session` or export the variable from your desktop session.

**Drift is listed under a different name.**
Run `playerctl -l` and use the name it prints with `-p`. If several Drift windows are open, `playerctl` lists each one; only control the one you want with its full name.

**Waybar shows nothing.**
Test the same command in a terminal first (`playerctl -p drift metadata`). If that works but Waybar doesn't, check that the player name in your config matches, and restart Waybar. If Waybar is installed as a Flatpak or Snap, it may be sandboxed away from the session bus.

**Media keys control the wrong player.**
Without `-p`, `playerctl` picks the most recently active player. Run `playerctld daemon` at login, or pin the keys to Drift with `-p drift`.

**Cover art doesn't appear in the widget.**
Some widgets only show art when the player provides it. Drift only fetches artwork when it needs it (see [Lyrics](#lyrics)), and the art shown by your widget depends on whether that widget can load remote image URLs.

**Check what Drift is exposing on D-Bus.**

```bash
busctl --user list | grep -i mpris
```
