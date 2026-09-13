# Input-method panel: fcitx5

Unrelated to `omarchy display text size` — that switch does not manage the IME
panel; the panel's font size is controlled by classicui alone.

Config file: `~/.config/fcitx5/conf/classicui.conf`

Apply changes:

```
systemctl --user restart omarchy-fcitx5.service
```

(Run by hand, `fcitx5 -r` is equivalent.)

## Font size

- `Font` — the candidate popup
- `MenuFont` — the right-click menu that pops up from the tray icon
- `TrayFont` — the tray menu

The value is a Pango description `"<family> <point size>"`, for example
`Noto Sans CJK SC 13`. **The point size is not screen pixels**: the theme sets
`ScaleWithDPI=True`, so the panel scales along with the output's scale — don't
guess the real size from the number, change it and look. The upstream default is
`Sans 10`.

The panel font size inside X11 clients (a browser moved to X11, say) does not follow
this path — see "The X11 candidate popup is smaller than Wayland's" at the end.

## Theme

fcitx5 ships two, installed with the fcitx5 package, under
`/usr/share/fcitx5/themes/`:

- `default` — light, white background, black text
- `default-dark` — dark, black background

Which one is used is decided by two keys:

- `UseDarkTheme=True` → use `DarkTheme` (set to `default-dark` on this machine)
- `UseDarkTheme=False` → use `Theme`

`omarchy theme` switching the desktop theme does **not** affect it; this one is
separate.

For a third-party theme, pacman has `fcitx5-material-color`, `fcitx5-nord`,
`fcitx5-breeze` (more in the AUR); after installing, point `Theme=` (or
`DarkTheme=`) at it. To change the colors yourself: drop a `theme.conf` into
`~/.local/share/fcitx5/themes/<name>/` to override. Inside `[InputPanel]`,
`NormalColor` / `HighlightCandidateColor` / `HighlightColor` /
`HighlightBackgroundColor` govern the normal candidate text, the highlighted
candidate text, the text drawn on the highlight background, and the highlight
background respectively.

## Switching Chinese/English: tap the left Shift

It is on by default; no config needed. The factory value of `AltTriggerKeys` is
`Shift_L` — **the left Shift only**; the right Shift is not bound.

- The press is a "tap": released within 250ms (`ModifierOnlyKeyTimeout=250`) with
  no other key in between. Holding Shift down and then pressing a letter types an
  uppercase letter as usual and does not switch.
- The effect is a toggle between "the active input method" and "the first input
  method of the group". The first one in this machine's group is `keyboard-us`, so
  that side is English.
- It only covers half the switching: it fires only while the state is active, or
  when Shift was the key that switched away last time (upstream `canAltTrigger`).
  After switching to English with `Control+space`, pressing Shift does nothing —
  you have to switch back with `Control+space`.
- With a single input method in the group the whole switch is dead (`canTrigger`
  requires >1).

**Root cause located 2026-09-13**: fcitx5 was not failing to receive the key.
Hyprland's `kb_options = "ctrl:swapcaps,shift:both_capslock_cancel"`
(`input.lua`) defines LFSH as `{[ Shift_L, Caps_Lock ]}` — while Shift is held,
that key's keysym *is* Caps_Lock. fcitx5's AltTrigger flow: on press the modifier
state is 0 → the key translates to Shift_L and is recorded as a pending trigger;
on release the xkb state still has Shift down → the same keycode 50 translates to
`Shift+Caps_Lock`, `isReleaseOfModifier(Shift_L)` fails to match on keysym, and
the trigger condition is never satisfied (the keyHandlers release branch in
`instance.cpp`). The key_trace log shows it directly: `Shift_L IsRelease=0`
followed by `Shift+Caps_Lock IsRelease=1`.

Conclusion: `shift:both_capslock_cancel` and fcitx5's "tap Shift to switch"
**exclude each other**; pick one:

- Want the Shift switch → delete `shift:both_capslock_cancel` from `kb_options` in
  `input.lua`. The only cost is losing the "press both Shifts to toggle Caps Lock"
  shortcut; Caps Lock itself survives (`ctrl:swapcaps` moves it onto the left Ctrl
  key).
- Want to keep "both Shifts toggle Caps Lock" → move the switch to an unaffected
  modifier, e.g. `AltTriggerKeys=Alt_L` (write it in the `[Hotkey]` section of
  `~/.config/fcitx5/config`), and Shift stays dead.
- A real fix has to be upstream: fcitx5 matches releases by keysym rather than
  keycode, so any xkb option where one key carries different keysyms at different
  levels mismatches; worth filing an issue against fcitx5.

**Outcome**: `shift:both_capslock_cancel` was deleted from `input.lua`, and in
practice tapping the left Shift switches again. Caps Lock still works from the
left Ctrl key (`ctrl:swapcaps` kept).

To add the right Shift or rebind the switch, write `~/.config/fcitx5/config`
(this machine has no such file — the whole global config is the defaults compiled
into libFcitx5Core):

```
[Hotkey]
AltTriggerKeys=Shift_L,Shift_R
```

To see the values actually in effect at runtime (no config file to read, no
defaults to guess):

```
busctl --user call org.fcitx.Fcitx5 /controller org.fcitx.Fcitx.Controller1 \
  GetConfig s "fcitx://config/global"
```

The output is one long line; look for the `AltTriggerKeys` part.

## Simplified vs Traditional: `Ctrl+Shift+F`

No separate input method is involved. The `chttrans` module post-processes
whatever the active engine produces, through OpenCC, and it is attached to the
pinyin IM by name:

`~/.config/fcitx5/conf/chttrans.conf`

```
[EnabledIM]
0=pinyin
```

That is the whole switch — the pinyin IM here is called `pinyin` (not
`pinyin-simp`), and `[EnabledIM]` lists the IMs the converter is enabled on. The
toggle key is `chttrans`'s own `Hotkey`, default `Control+Shift+F`, so **it is the
converter that flips, not the engine**: after a toggle you keep typing pinyin and
the candidates come out Traditional (a notification says
"Traditional Chinese is enabled." / "Simplified Chinese is enabled.").

The state is runtime-only — it is not written to `profile` — so it does not
survive a restart of fcitx5 and a stray keypress is the usual cause of a sudden
switch to Traditional.

To turn it off for good, write an **empty** section in the same file — commenting
the keys out is not enough, because a missing key falls back to the compiled-in
default and you keep the `Ctrl+Shift+F` toggle:

```
[Hotkey]

[EnabledIM]
0=pinyin
```

Either empty `[Hotkey]` (the binding goes away, the converter stays attached) or
drop `0=pinyin` from `[EnabledIM]` (nothing is converted even if the key is
pressed) — verified 2026-09-13 by reading the running config back after
`ReloadAddonConfig`: both settle to `""`. Reload without a restart:

```
busctl --user call org.fcitx.Fcitx5 /controller org.fcitx.Fcitx.Controller1 \
  ReloadAddonConfig s "chttrans"
```

(Or `systemctl --user restart omarchy-fcitx5.service`.)

To read the values actually in effect:

```
busctl --user call org.fcitx.Fcitx5 /controller org.fcitx.Fcitx.Controller1 \
  GetConfig s "fcitx://config/addon/chttrans"
```

(Note this is `.../addon/chttrans`, not `.../global` as for `AltTriggerKeys`.)
Useful profiles for `OpenCCS2TProfile`: `s2t.json` (plain), `s2tw.json` (Taiwan
phrases), `s2hk.json` (Hong Kong).

## Chromium-family candidate popup misplaced → the browser on X11

**Symptom**: in Wayland mode, the candidate popup floats above the input box or at
the top of the window and does not line up with the cursor; native Wayland apps
such as foot are fine. Hits every Chromium-family browser (Edge, Chromium, Chrome,
Brave); `~/.config/chromium-flags.conf` carries the same fix as Edge's file below.

**Root cause** (probed 2026-09-13 with `WAYLAND_DEBUG=1` plus CDP injection):
Chromium's text-input-v3 implementation waits for the compositor's `done` serial to
catch up with its own commit count before sending `set_cursor_rectangle` (the gate
in `ZwpTextInputV3Impl::SendPendingImeData`), while Hyprland only sends `done` when
the IME pushes a preedit, never in reply to a client's own commit. On every newly
focused input box Hyprland thus has no cursor rectangle before the first commit,
`CInputPopup::updateBox()` takes its fallback and treats the whole window as the
cursor box → the overflow check always trips → the popup is pinned to the top of
the window. foot sends the cursor rectangle unconditionally, so it is unaffected.

Both sides are known upstream bugs and unfixed: Chromium issue 384531043,
sway#8884, Hyprland #15258 (closed not_planned).

**The obvious-looking way out — wayland text-input-v1 — does not work.** Chromium
still ships `zwp_text_input_v1` and `--enable-wayland-ime
--wayland-text-input-version=1` switches to it, and v1 has no `done` handshake at
all, so its `set_cursor_rectangle` is sent unconditionally (measured: 42 sends where
v3 sent 0). But Hyprland's v1 popup repositioning is driven **only** by the client's
`commit_state` (`CTextInputV1::m_events.onCommit` has exactly one producer —
`setCommitState`), and Chromium's v1 implementation never sends it: `zwp_text_input_v1.cc`
has no `commit_state` call, and the packet log confirms 47 `set_cursor_rectangle`
against 0 `commit_state`. The popup is therefore positioned once at focus time and
never follows the caret as the preedit grows — tried 2026-09-13, visibly worse than
v3: the panel drifts out from under the text and lags behind every keystroke. Do
not retry this.

**Local fix**: the browser moved to X11 (XWayland),
`~/.config/microsoft-edge-stable-flags.conf` (and `chromium-flags.conf` for Chromium
itself):

```
--ozone-platform=x11
--force-device-scale-factor=1.5
```

Under X11 the popup lands correctly because the cursor rectangle travels a
different path: the browser drives fcitx5's input context itself over D-Bus
(`org.freedesktop.portal.Fcitx` → `/org/freedesktop/portal/inputcontext/N`,
interface `org.fcitx.Fcitx.InputContext1`; watch it with
`busctl --user monitor org.freedesktop.portal.Fcitx`) and `SetCursorRect` carries
**X11 global coordinates**, so Hyprland is not in the loop at all — no `done`
handshake to stall on. `--force-device-scale-factor=1.5` combined with Hyprland's
`force_zero_scaling=true` renders at physical pixels, so the UI is not blurry.
Window management (tiling / workspaces / keybindings) still applies to XWayland
windows as usual.

**It is not ibus.** An earlier version of this note said X11 goes through the ibus
protocol; that is wrong. Probed 2026-09-13: the browser process maps
`im-fcitx5.so` (the GTK immodule) and `libgtk-3.so` and **no** `libibus*`; fcitx5
registers the X11 client as `frontend:dbus`, not ibus; and on
`org.freedesktop.IBus` there is no traffic at all — fcitx5 merely owns the bus
name (`libibusfrontend.so`) so that programs hardwired to ibus find *something*.
The real interface is fcitx's own D-Bus frontend.

**The flags file must not contain comments** (learned the hard way, 2026-09-13):
the launch script `microsoft-edge-stable` does `EDGE_USER_FLAGS="$(cat ...)"` and
splits the result **unquoted** into the command line — inside a variable expansion
`#` is not a comment, so every word of a comment line becomes an argument, and Edge
takes any argument not starting with `-` as a URL: a pile of junk tabs on startup
and comment fragments in the address bar. The file holds one argument per line and
nothing else; the explanation belongs here.

## The X11 candidate popup is smaller than Wayland's → Xft.dpi=144

**Symptom**: after a Chromium-family browser moved to X11, the candidate popup's
font inside X11 is only about 2/3 the size of the Wayland panel's.

**Root cause**: classicui's Wayland panel renders at output scale 1.5 (144 DPI
equivalent); the X11 panel goes through DPI computation: Xft.dpi (unset) → the X
screen's physical DPI. Under `force_zero_scaling` XWayland reports a fake physical
size (measured 3840x2160 = 1016x571mm, exactly 96 DPI), so the X11 panel draws at
96 DPI, only 2/3 of Wayland's. The display's real physical DPI is 162 (600x340mm),
but XWayland does not pass it through.

**Fix**: fcitx5's xcb UI reads `Xft.dpi:\t<N>` from the X root window's
`RESOURCE_MANAGER` property and prefers it over the screen DPI. xrdb is not
installed here, so `~/.local/bin/xwayland-xft-dpi` (python3 + libX11) writes the
property directly and restarts fcitx5 (fcitx5 reads it only when the X11 UI
initializes, and the autostart order races, hence the `try-restart` in the script).
`o.launch_on_start("xwayland-xft-dpi")` in `~/.config/hypr/autostart.lua` runs it on
every login.

144 = 96 × 1.5, following Hyprland's monitor scale; change one and you must change
the other. Side effect: every X11 app that reads Xft.dpi (GTK3 / Qt on X11, …) also
renders at 1.5x — they were undersized at 96 DPI too, so this fixes them on the
side. A browser running on X11 uses its own `--force-device-scale-factor` and is
unaffected.
