#!/usr/bin/env python3
"""keyd-cheatsheet — render keyd config(s) as a visual per-layer keyboard (HTML).

Parses keyd .conf files, understands lettermod()/overload() tap/hold duality,
toggle() chords, plain remaps, and per-layer overrides, then emits a
self-contained dark-themed HTML cheatsheet — one keyboard per config, one
board per layer, with a table of contents.

Usage:
    keyd-cheatsheet [CONF ...] [-o OUT.html] [--open]

With no CONF args it globs /etc/keyd/*.conf. Each config picks a physical
layout automatically: HHKB for *hhkb*, ANSI-60 otherwise.

The config is the source of truth: re-run after every edit to refresh.
Stdlib only — no dependencies.
"""
from __future__ import annotations

import argparse
import datetime as _dt
import glob
import html
import os
import re
import sys
from dataclasses import dataclass, field

# A physical keyboard layout: rows of (keyd-key-name, width-in-units).
Layout = list[list[tuple[str, float]]]

# ------------------------------------------------------------- physical layouts
# Keyed by keyd key-names. (name, width_units).
HHKB: Layout = [
    [("esc",1),("1",1),("2",1),("3",1),("4",1),("5",1),("6",1),("7",1),("8",1),
     ("9",1),("0",1),("minus",1),("equal",1),("backslash",1),("grave",1)],
    [("tab",1.5),("q",1),("w",1),("e",1),("r",1),("t",1),("y",1),("u",1),("i",1),
     ("o",1),("p",1),("leftbrace",1),("rightbrace",1),("backspace",1.5)],
    [("leftcontrol",1.75),("a",1),("s",1),("d",1),("f",1),("g",1),("h",1),("j",1),
     ("k",1),("l",1),("semicolon",1),("apostrophe",1),("enter",2.25)],
    [("leftshift",2.25),("z",1),("x",1),("c",1),("v",1),("b",1),("n",1),("m",1),
     ("comma",1),("dot",1),("slash",1),("rightshift",1.75),("fn",1)],
    [("leftalt",1.5),("leftmeta",1),("space",7),("rightmeta",1),("rightalt",1.5)],
]
ANSI60: Layout = [
    [("grave",1),("1",1),("2",1),("3",1),("4",1),("5",1),("6",1),("7",1),("8",1),
     ("9",1),("0",1),("minus",1),("equal",1),("backspace",2)],
    [("tab",1.5),("q",1),("w",1),("e",1),("r",1),("t",1),("y",1),("u",1),("i",1),
     ("o",1),("p",1),("leftbrace",1),("rightbrace",1),("backslash",1.5)],
    [("capslock",1.75),("a",1),("s",1),("d",1),("f",1),("g",1),("h",1),("j",1),
     ("k",1),("l",1),("semicolon",1),("apostrophe",1),("enter",2.25)],
    [("leftshift",2.25),("z",1),("x",1),("c",1),("v",1),("b",1),("n",1),("m",1),
     ("comma",1),("dot",1),("slash",1),("rightshift",2.75)],
    [("leftcontrol",1.25),("leftmeta",1.25),("leftalt",1.25),("space",6.25),
     ("rightalt",1.25),("rightmeta",1.25),("menu",1.25),("rightcontrol",1.25)],
]


def layout_for(path: str) -> tuple[Layout, str]:
    name = os.path.basename(path).lower()
    return (HHKB, "HHKB 60%") if "hhkb" in name else (ANSI60, "ANSI 60%")


# ----------------------------------------------------------------- legend maps
SHIFT_SYM = {
    "1":"!","2":"@","3":"#","4":"$","5":"%","6":"^","7":"&","8":"*","9":"(","0":")",
    "minus":"_","equal":"+","leftbrace":"{","rightbrace":"}","backslash":"|",
    "semicolon":":","apostrophe":'"',"comma":"<","dot":">","slash":"?","grave":"~",
}
LEGEND = {
    "minus":"-","equal":"=","backslash":"\\","grave":"`","leftbrace":"[",
    "rightbrace":"]","semicolon":";","apostrophe":"'","comma":",","dot":".",
    "slash":"/","esc":"Esc","tab":"Tab","backspace":"⌫","delete":"Del",
    "enter":"⏎","space":"Space","left":"←","right":"→","up":"↑",
    "down":"↓","home":"Home","end":"End","pageup":"PgUp","pagedown":"PgDn",
    "capslock":"Caps","menu":"Menu","leftshift":"⇧","rightshift":"⇧",
    "leftcontrol":"Ctrl","leftctrl":"Ctrl","rightcontrol":"Ctrl",
    "leftalt":"Alt","rightalt":"Alt","leftmeta":"◇","rightmeta":"◇","fn":"Fn",
}
MODS = {"control","shift","alt","meta","altgr"}
MOD_GLYPH = {"C":"⌃","S":"⇧","A":"⌥","M":"◇","G":"AltGr"}
MOD_NAME = {"control":"Ctrl","shift":"Shift","alt":"Alt","meta":"Super","altgr":"AltGr"}
TAPHOLD = ("lettermod", "overload", "overloadi", "overloadt", "overloadt2")
ACCENT = {"nav":"#4aa3ff","num":"#3ddc84","sym":"#c792ea","control":"#ff6b6b",
          "game":"#9aa0a6"}
DEFAULT_ACCENT = "#ffb454"
REMAP_ACCENT = "#ffb454"


def accent_for(name: str) -> str:
    return ACCENT.get(name, DEFAULT_ACCENT)


def base_legend(keyname: str) -> str:
    if keyname in LEGEND:
        return LEGEND[keyname]
    if len(keyname) == 1 and keyname.isalpha():
        return keyname.upper()
    if keyname.isdigit():
        return keyname
    return keyname


def prettify(value: str) -> str:
    """Turn a keyd binding value into a human glyph (handles S-/C-/A-/M- mods)."""
    m = re.fullmatch(r"((?:[CSAMG]-)+)(.+)", value)
    if m:
        mods = re.findall(r"([CSAMG])-", m.group(1))
        base = m.group(2)
        if mods == ["S"] and base in SHIFT_SYM:
            return SHIFT_SYM[base]
        glyphs = "".join(MOD_GLYPH.get(x, x) for x in mods)
        return glyphs + base_legend(base)
    return base_legend(value)


# --------------------------------------------------------------------- parsing
# A tap/hold binding: (key, target_layer_or_mod, "layer"|"mod", tapkey-or-None).
Hold = tuple[str, str, str, str | None]


@dataclass
class Config:
    ids: list[str] = field(default_factory=list)
    layers: dict[str, dict[str, str]] = field(default_factory=dict)  # incl. game
    holds: list[Hold] = field(default_factory=list)
    chords: list[tuple[str, str]] = field(default_factory=list)      # chord, target
    remaps: dict[str, str] = field(default_factory=dict)             # plain key->val


def parse(path: str) -> Config:
    with open(path, encoding="utf-8") as fh:
        return parse_text(fh.read())


def parse_text(text: str) -> Config:
    """Parse keyd config text into a Config (pure; no I/O — used by parse() & tests)."""
    cfg = Config()
    section = None
    for raw in text.splitlines():
        line = raw.split("#", 1)[0].strip()   # keyd has full-line comments only
        if not line:
            continue
        sec = re.fullmatch(r"\[(\w+)\]", line)
        if sec:
            section = sec.group(1)
            if section not in ("ids", "main"):
                cfg.layers.setdefault(section, {})
            continue
        if section == "ids":
            cfg.ids.append(line)
            continue
        if "=" not in line:
            continue
        key, val = (p.strip() for p in line.split("=", 1))
        if section == "main":
            fn = re.fullmatch(r"(\w+)\((.*)\)", val)
            if fn and fn.group(1) in TAPHOLD:
                args = [a.strip() for a in fn.group(2).split(",")]
                target, tapkey = args[0], (args[1] if len(args) > 1 else key)
                kind = "mod" if target in MODS else "layer"
                cfg.holds.append((key, target, kind, tapkey))
            elif fn and fn.group(1) == "toggle":
                cfg.chords.append((key, fn.group(2).strip()))
            elif fn and fn.group(1) == "layer":
                arg = fn.group(2).strip()          # momentary hold, no tap action
                cfg.holds.append((key, arg, "mod" if arg in MODS else "layer", None))
            elif fn:
                cfg.remaps[key] = val          # other macro: show raw-ish
            else:
                cfg.remaps[key] = val          # plain remap, e.g. leftcontrol=capslock
        else:
            cfg.layers[section][key] = val
    return cfg


# --------------------------------------------------------------------- render
CSS = """
:root { --u: 46px; --gap: 5px; }
* { box-sizing: border-box; }
body { margin: 0; padding: 28px 24px 60px; background: #11151c; color: #e6e6e6;
       font: 14px/1.45 'JetBrains Mono','Fira Code',ui-monospace,monospace; }
h1 { font-size: 21px; margin: 0 0 4px; }
.sub { color: #7d8694; font-size: 12px; margin-bottom: 18px; }
.sub code { color: #9aa7b8; }
.toc { background:#0c0f14; border:1px solid #1e2630; border-radius:10px;
       padding:12px 16px; margin:0 0 26px; display:inline-block; }
.toc b { font-size:12px; color:#7d8694; text-transform:uppercase;
         letter-spacing:.05em; }
.toc a { color:#9ecbff; text-decoration:none; margin-right:16px; }
.toc a:hover { text-decoration:underline; }
.kbgroup { border-top:1px solid #1e2630; padding-top:18px; margin-top:26px; }
.kbgroup > h2 { font-size:17px; margin:0 0 2px; }
.kbgroup > .meta { color:#7d8694; font-size:12px; margin-bottom:16px; }
.kbgroup > .meta code { color:#9aa7b8; }
.board { margin: 0 0 26px; }
.board h3 { font-size: 14px; margin: 0 0 9px; display: flex; align-items: center;
            gap: 10px; font-weight:600; }
.tag { font-size: 11px; font-weight: 700; padding: 2px 8px; border-radius: 6px;
       color: #11151c; }
.hint { color: #7d8694; font-weight: 400; font-size: 12px; }
.kb { display: inline-block; background: #0c0f14; padding: 10px; border-radius: 12px;
      border: 1px solid #1e2630; }
.row { display: flex; gap: var(--gap); margin-bottom: var(--gap);
       justify-content: center; }
.row:last-child { margin-bottom: 0; }
.key { height: var(--u); border-radius: 7px; background: #1b2129;
       border: 1px solid #2a3340; border-bottom-width: 3px; position: relative;
       display: flex; align-items: center; justify-content: center;
       font-size: 13px; color: #cfd6df; padding: 2px; overflow: hidden; }
.key.dim { color: #4a525e; background: #161b22; }
.key.act { box-shadow: inset 0 0 0 2px currentColor; }
.key .main { font-size: 15px; }
.key .ov { font-size: 16px; font-weight: 700; }
.badge { position: absolute; left: 4px; bottom: 2px; font-size: 9px;
         font-weight: 700; line-height: 1; padding: 1px 3px; border-radius: 4px;
         color: #11151c; }
.tl { position: absolute; right: 4px; top: 3px; font-size: 9px; color: #5b6573; }
.legend { display: flex; gap: 18px; flex-wrap: wrap; font-size: 12px;
          color: #9aa7b8; margin: 4px 0 8px; }
.legend b { color: #e6e6e6; }
"""


def esc(s: str) -> str:
    return html.escape(s)


def keycap(inner: str, width: float, extra_cls: str = "", color: str = "") -> str:
    style = f"width: calc(var(--u) * {width} + var(--gap) * {width - 1});"
    if color:
        style += f" color: {color};"
    return f'<div class="key {extra_cls}" style="{style}">{inner}</div>'


def render_base(cfg: Config, phys: Layout) -> str:
    holds = {k: (t, kind, tap) for k, t, kind, tap in cfg.holds}
    chord_keys: dict[str, str] = {}
    for chord, target in cfg.chords:
        for part in chord.split("+"):
            chord_keys[part.strip()] = target
    rows = []
    for prow in phys:
        cells = []
        for name, w in prow:
            cls, col = "", ""
            if name in holds:
                target, kind, tap = holds[name]
                col = accent_for(target if kind == "layer" else "control")
                label = MOD_NAME.get(target, target) if kind == "mod" else target
                if tap is None:
                    # pure momentary modifier/layer — the key simply *is* that function
                    inner = f'<span class="ov">{esc(label)}</span>'
                    inner += f'<span class="tl">{esc(base_legend(name))}</span>'
                else:
                    inner = f'<span class="main">{esc(prettify(tap))}</span>'
                    inner += (f'<span class="badge" style="background:{col}">'
                              f'↓{esc(label)}</span>')
            elif name in cfg.remaps:
                col = REMAP_ACCENT
                inner = f'<span class="ov">{esc(prettify(cfg.remaps[name]))}</span>'
                inner += f'<span class="tl">{esc(base_legend(name))}</span>'
            else:
                inner = f'<span class="main">{esc(base_legend(name))}</span>'
            if name in chord_keys:
                tcol = accent_for(chord_keys[name])
                inner += (f'<span class="badge" style="background:{tcol};left:auto;'
                          f'right:4px">⇧⇧</span>')
            cells.append(keycap(inner, w, cls, col))
        rows.append('<div class="row">' + "".join(cells) + "</div>")
    return ('<div class="board"><h3>Base layer '
            '<span class="hint">tap = legend · ↓badge = hold · orange = remap</span>'
            '</h3><div class="kb">' + "".join(rows) + "</div></div>")


def render_layer(cfg: Config, name: str, ov: dict[str, str], phys: Layout) -> str:
    accent = accent_for(name)
    act_key = next((k for k, t, _, _ in cfg.holds if t == name), None)
    chord = next((c for c, t in cfg.chords if t == name), None)
    if name == "game":
        if chord:
            keys = esc(" + ".join(base_legend(p) for p in chord.split("+")))
            how = f'toggle: <b style="color:{accent}">{keys}</b>'
        else:
            how = "toggle layer"
        hint = "passthrough — these revert to plain keys (gaming)"
    else:
        how = (f'hold <b style="color:{accent}">{esc(base_legend(act_key))}</b>'
               if act_key else "")
        hint = "highlighted keys change while held"
    rows = []
    for prow in phys:
        cells = []
        for nm, w in prow:
            if nm in ov:
                glyph = (esc(base_legend(nm)) if name == "game"
                         else esc(prettify(ov[nm])))
                inner = f'<span class="ov">{glyph}</span>'
                if name != "game":
                    inner += f'<span class="tl">{esc(base_legend(nm))}</span>'
                cells.append(keycap(inner, w, "", accent))
            elif nm == act_key:
                inner = (f'<span class="main">{esc(base_legend(nm))}</span>'
                         f'<span class="badge" style="background:{accent}">HOLD</span>')
                cells.append(keycap(inner, w, "act", accent))
            else:
                inner = f'<span class="main">{esc(base_legend(nm))}</span>'
                cells.append(keycap(inner, w, "dim"))
        rows.append('<div class="row">' + "".join(cells) + "</div>")
    tag = f'<span class="tag" style="background:{accent}">{esc(name.upper())}</span>'
    return ('<div class="board"><h3>' + tag + f'<span class="hint">{how}</span>'
            f'<span class="hint">· {hint}</span></h3>'
            '<div class="kb">' + "".join(rows) + "</div></div>")


def slug(path: str) -> str:
    return re.sub(r"[^a-z0-9]+", "-", os.path.basename(path).lower())


def render_config(cfg: Config, src: str, phys: Layout, prof: str) -> str:
    boards = [render_base(cfg, phys)]
    order = [n for n in cfg.layers if n != "game"] + (
        ["game"] if "game" in cfg.layers else [])
    for name in order:
        boards.append(render_layer(cfg, name, cfg.layers[name], phys))
    ids = ", ".join(cfg.ids) or "—"
    return (f'<div class="kbgroup" id="cfg-{slug(src)}">'
            f'<h2>{esc(os.path.basename(src))}</h2>'
            f'<div class="meta">{esc(prof)} · ids: {esc(ids)} · '
            f'<code>{esc(src)}</code></div>'
            + "".join(boards) + "</div>")


def render_page(items) -> str:
    when = _dt.datetime.now().strftime("%Y-%m-%d %H:%M")
    toc = "".join(
        f'<a href="#cfg-{slug(src)}">{esc(os.path.basename(src))}</a>'
        for _, src, _, _ in items)
    legend = (
        '<div class="legend">'
        '<span><b>tap</b> = key legend</span>'
        '<span><b>↓badge</b> = hold engages that layer/mod</span>'
        '<span><b>HOLD</b> = key you hold to reach a layer</span>'
        '<span><b>orange</b> = plain remap</span>'
        '<span><b>⇧⇧</b> = both-shift chord</span>'
        '<span>top-right ghost = the key’s normal legend</span></div>')
    groups = "".join(render_config(c, s, p, pr) for c, s, p, pr in items)
    return f"""<!doctype html><html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>keyd cheatsheet</title><style>{CSS}</style></head><body>
<h1>keyd layout cheatsheet</h1>
<div class="sub">generated {when}</div>
<div class="toc"><b>keyboards:</b><br>{toc}</div>
{legend}
{groups}
</body></html>"""


# ----------------------------------------------------------------------- main
def main() -> int:
    ap = argparse.ArgumentParser(
        description="Render keyd config(s) as an HTML cheatsheet.")
    ap.add_argument("conf", nargs="*",
                    help="keyd config path(s) (default: glob /etc/keyd/*.conf)")
    ap.add_argument("-o", "--out",
                    default=os.path.expanduser("~/.cache/keyd-cheatsheet/keyd.html"),
                    help="output HTML path")
    ap.add_argument("--open", action="store_true", help="open in default browser")
    args = ap.parse_args()

    confs = args.conf or sorted(glob.glob("/etc/keyd/*.conf"))
    if not confs:
        print("error: no keyd configs found in /etc/keyd/", file=sys.stderr)
        return 1
    items = []
    for path in confs:
        if not os.path.exists(path):
            print(f"warning: skipping missing {path}", file=sys.stderr)
            continue
        phys, prof = layout_for(path)
        items.append((parse(path), os.path.abspath(path), phys, prof))
    if not items:
        return 1

    os.makedirs(os.path.dirname(args.out), exist_ok=True)
    with open(args.out, "w", encoding="utf-8") as fh:
        fh.write(render_page(items))
    print(f"wrote {args.out}  ({len(items)} keyboard(s))")
    if args.open:
        import subprocess
        subprocess.Popen(["xdg-open", args.out],
                         stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
