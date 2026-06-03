# keyd-cheatsheet

Render your [keyd](https://github.com/rvaiya/keyd) configs as a visual,
per-layer keyboard cheatsheet — one self-contained HTML page, no dependencies.

keyd is a brilliant config-file keyboard remapper, but unlike QMK/VIA there's no
GUI to *see* your layout. Once you've got a few tap/hold layers going, it's easy
to forget what `hold space` or `hold f` actually does. This parses your `.conf`
files and draws them.

![screenshot](docs/screenshot.png)

## Why

- **No keyd visualizer exists.** [keymap-drawer](https://github.com/caksoylar/keymap-drawer)
  parses QMK/ZMK, not keyd.
- **It understands keyd's tap/hold model.** `lettermod()`, `overload()`,
  `layer()`, `toggle()` chords, and plain remaps each render distinctly — tap
  legend on the key, hold action as a colored badge.
- **Your config is the source of truth.** Re-run after any edit; the cheatsheet
  always matches what keyd is actually doing.
- **Zero dependencies.** Pure Python stdlib. One file.

## Install

```sh
git clone https://github.com/coffeeowl-labs/keyd-cheatsheet
cd keyd-cheatsheet
# run directly...
./keyd_cheatsheet.py --open
# ...or symlink it onto your PATH so it tracks the repo:
ln -s "$PWD/keyd_cheatsheet.py" ~/.local/bin/keyd-cheatsheet
```

## Usage

```sh
keyd-cheatsheet                      # render every /etc/keyd/*.conf into one page
keyd-cheatsheet --open               # ...and open it in your browser
keyd-cheatsheet examples/hhkb.conf   # render specific config(s)
keyd-cheatsheet -o /tmp/kb.html      # choose the output path
```

Default output: `~/.cache/keyd-cheatsheet/keyd.html`.

## What it renders

For each config it draws a **base board** plus **one board per layer**:

| keyd binding                         | shown as |
| ------------------------------------ | -------- |
| `f = lettermod(nav, f, ...)`         | key `F` with a `↓nav` badge; a `NAV` board where the held key is outlined `HOLD` |
| `k = lettermod(control, k, ...)`     | key `K` with a `↓Ctrl` badge |
| `capslock = overload(control, esc)`  | key shows tap legend `Esc` + `↓Ctrl` badge |
| `capslock = layer(control)`          | key shows `Ctrl` (pure modifier) with its original legend ghosted |
| `leftcontrol = capslock`             | key shows `Caps` (plain remap), original legend ghosted |
| `leftshift+rightshift = toggle(game)`| `⇧⇧` chord badge; a `GAME` passthrough board |
| `[nav] h = left`                     | `←` on the NAV board, with the key's normal legend ghosted in the corner |

### Physical layouts

The board is chosen per config by filename: `*hhkb*` → an HHKB 60% layout,
everything else → ANSI 60% (with a CapsLock key). Both are 60% views — they show
the remapped keys clearly rather than reproducing every physical key.

## Tests

Stdlib only, no install:

```sh
python -m unittest discover -s tests
```

## Roadmap

- More / configurable physical layouts (TKL, ortho, split)
- Optional PNG export
- Packaging (PyPI / AUR)

## License

MIT
