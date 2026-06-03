"""Tests for the keyd config parser and value prettifier.

Run with: pytest   (import path provided by pyproject's [tool.pytest.ini_options]).
"""
import pytest

import keyd_cheatsheet as kc


# ----------------------------------------------------------------------- parse
def test_lettermod_tap_and_hold():
    cfg = kc.parse_text("[main]\nf = lettermod(nav, f, 150, 200)\n")
    assert cfg.holds == [("f", "nav", "layer", "f")]


def test_lettermod_to_modifier():
    cfg = kc.parse_text("[main]\nk = lettermod(control, k, 150, 200)\n")
    assert cfg.holds == [("k", "control", "mod", "k")]


def test_overload_tap_differs_from_key():
    # capslock taps Esc, holds Ctrl
    cfg = kc.parse_text("[main]\ncapslock = overload(control, esc)\n")
    assert cfg.holds == [("capslock", "control", "mod", "esc")]


def test_layer_is_pure_modifier_no_tap():
    cfg = kc.parse_text("[main]\ncapslock = layer(control)\n")
    assert cfg.holds == [("capslock", "control", "mod", None)]


def test_plain_remap():
    cfg = kc.parse_text("[main]\nleftcontrol = capslock\n")
    assert cfg.remaps == {"leftcontrol": "capslock"}
    assert cfg.holds == []


def test_toggle_chord():
    cfg = kc.parse_text("[main]\nleftshift+rightshift = toggle(game)\n")
    assert cfg.chords == [("leftshift+rightshift", "game")]


def test_layer_section_overrides():
    cfg = kc.parse_text("[nav]\nh = left\nj = down\n")
    assert cfg.layers["nav"] == {"h": "left", "j": "down"}


def test_ids_collected():
    cfg = kc.parse_text("[ids]\n04fe:0021\n04fe:0202\n")
    assert cfg.ids == ["04fe:0021", "04fe:0202"]


def test_full_line_comments_and_blanks_ignored():
    text = "# a comment\n\n[main]\n# another\nf = lettermod(nav, f, 1, 2)\n"
    cfg = kc.parse_text(text)
    assert len(cfg.holds) == 1


def test_empty_layer_section_registered():
    cfg = kc.parse_text("[sym]\n")
    assert "sym" in cfg.layers


# -------------------------------------------------------------------- prettify
@pytest.mark.parametrize(
    ("value", "expected"),
    [
        ("S-9", "("),
        ("S-0", ")"),
        ("S-minus", "_"),
        ("S-leftbrace", "{"),
        ("S-rightbrace", "}"),
        ("C-left", "⌃←"),
        ("C-right", "⌃→"),
        ("leftbrace", "["),
        ("backspace", "⌫"),
        ("esc", "Esc"),
        ("a", "A"),
    ],
)
def test_prettify(value, expected):
    assert kc.prettify(value) == expected


# ---------------------------------------------------------------------- layout
@pytest.mark.parametrize(
    ("path", "layout", "profile"),
    [
        ("/etc/keyd/hhkb.conf", kc.HHKB, "HHKB 60%"),
        ("/etc/keyd/laptop.conf", kc.ANSI60, "ANSI 60%"),
    ],
)
def test_layout_selection(path, layout, profile):
    phys, prof = kc.layout_for(path)
    assert phys is layout
    assert prof == profile
