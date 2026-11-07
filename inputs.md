
## Input methods

Tema speaks `text-input-v3`, `input-method-v2`, and `virtual-keyboard-v1`.
One input method can own the seat. A second one is told `unavailable`, so
fcitx5, ibus, squeekboard, and `wvkbd --auto` are alternatives, not a stack.
`wtype` and `wvkbd` without `--auto` only inject keys, and can run beside an
input method. See [ARCHITECTURE.md](ARCHITECTURE.md) for the relay, keymap
switch, and keyboard grab.

### fcitx5 and ibus

    # fcitx5
    GTK_IM_MODULE=fcitx QT_IM_MODULE=fcitx XMODIFIERS=@im=fcitx fcitx5

    # ibus
    GTK_IM_MODULE=ibus QT_IM_MODULE=ibus XMODIFIERS=@im=ibus ibus-daemon -rx

The daemon binds `input-method-v2` and types through a virtual keyboard.
GTK and Qt clients that use the variables above send `text-input-v3`.

### wtype

`wtype` injects keystrokes into the focused surface through
`virtual-keyboard-v1`. It does not become the input method.

    wtype 'hello'

### squeekboard

squeekboard is an input method, a virtual keyboard, and a layer-shell
surface. It shows itself when Tema sends `activate` and hides on
`deactivate`. Its layer uses `keyboard_interactivity = none`, so a tap keeps
the text field focused. It cannot run next to fcitx5 or ibus.

### wvkbd

`wvkbd-mobintl` (or `wvkbd-deskintl`) is a layer-shell keyboard that types
with `virtual-keyboard-v1`. Without `--auto` it does not take the input-method
seat: `SIGUSR1` hides it, `SIGUSR2` shows it, and `SIGRTMIN` toggles it.
`--auto` makes it an input method, so it shows and hides with the text field
and conflicts with fcitx5, ibus, and squeekboard. Taps do not move keyboard
focus.
