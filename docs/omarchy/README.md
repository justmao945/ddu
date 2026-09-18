# Omarchy tweak notes

Notes on what was changed in Omarchy and how. Only the *how* — the configs
themselves are not kept here.

- [text-size.md](text-size.md) — text size across the omarchy shell / GTK / terminals
- [cn-mirrors.md](cn-mirrors.md) — the domestic mirrors this host pulls
  through: mise (`url_replacements` for GitHub release assets, node tarballs,
  uv/pip/rustup), `~/.npmrc`, the crates.io source replacement, and the pacman
  mirrorlist kept alive by a `pre-refresh-pacman` hook
- [cc-switch.md](cc-switch.md) — the local proxy that lets `claude` (Claude Code)
  reach the Command Code open models, and the two systemd user units that keep
  it up across reboots
- [fcitx5.md](fcitx5.md) — the input-method panel (fcitx5): font size and theme,
  Shift to switch Chinese/English, Simplified vs Traditional (`Ctrl+Shift+F`), the
  Chromium-family misplaced candidate popup (moving the browser to X11) and the X11
  panel DPI fix
