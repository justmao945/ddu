# Text size: omarchy shell / GTK / terminals

One switch changes all three; the implementation is
`/usr/share/omarchy/bin/omarchy-display-text-size`.

```
omarchy display text size          # show current: px / GTK factor / terminal pt
omarchy display text size 16       # set it, integer 9–20 (unit: px)
omarchy display text size reset    # back to the default 12px / 1.0 / 9pt
```

All three are anchored at the shell default of **12px**. The "Text scale" entry in
the settings UI *is* this switch (it records the value under `written.text-scale`
in `~/.config/omarchy/omasettings.json`).

| Target | Lands in | Anchor | Current |
| --- | --- | --- | --- |
| omarchy shell (bar / OSD / panels) | `~/.config/omarchy/shell.toml` → `[font] base-size` | the px itself (the shell's rem root) | `16` |
| GTK apps | dconf `org.gnome.desktop.interface text-scaling-factor` | 12px = `1.0` | `1.3333` |
| Terminals | one key per terminal, see below | 12px = `9pt` | `12` pt |

## GTK: text-scaling-factor

```
gsettings get   org.gnome.desktop.interface text-scaling-factor
gsettings set   org.gnome.desktop.interface text-scaling-factor 1.3333
gsettings reset org.gnome.desktop.interface text-scaling-factor
```

Do not just write `px / 12`. Quantize to whole points, or the UI font lands on a
fractional point size and GTK4 menus clip their ascenders on a scale-1 display:

```
factor = round(font_pt * px / 12) / font_pt
```

`font_pt` is the point size inside `org.gnome.desktop.interface font-name` — on
this machine `Adwaita Sans 9`, so 16px → `round(9 × 16 / 12) / 9` = `12 / 9` =
`1.3333`.

## Terminals: point size

```
pt = round(px * 9 / 12)            # 16px → 12pt
```

| Terminal | key | Takes effect |
| --- | --- | --- |
| alacritty | `~/.config/alacritty/alacritty.toml` → `[font] size = 12` | new window |
| kitty | `~/.config/kitty/kitty.conf` → `font_size 12.0` | `pkill -USR1 kitty` (hot reload) |
| ghostty | `~/.config/ghostty/config` → `font-size = 12` | `pkill -SIGUSR2 ghostty` (hot reload) |
| foot | `~/.config/foot/foot.ini` → `font=JetBrainsMono Nerd Font:size=12` | no reload signal: new windows only, running ones must restart (the script pops a notification) |

The font **family** is a different switch, `omarchy font set`; these commands do
not touch it.
