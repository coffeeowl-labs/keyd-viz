"""Unit tests for the keyd config parser and value prettifier.

Stdlib only — run any of:
    python -m unittest discover -s tests
    python tests/test_parser.py
    pytest
"""
import pathlib
import sys
import unittest

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1]))
import keyd_cheatsheet as kc  # noqa: E402


class TestParse(unittest.TestCase):
    def test_lettermod_tap_and_hold(self):
        cfg = kc.parse_text("[main]\nf = lettermod(nav, f, 150, 200)\n")
        self.assertEqual(cfg.holds, [("f", "nav", "layer", "f")])

    def test_lettermod_to_modifier(self):
        cfg = kc.parse_text("[main]\nk = lettermod(control, k, 150, 200)\n")
        self.assertEqual(cfg.holds, [("k", "control", "mod", "k")])

    def test_overload_tap_differs_from_key(self):
        # capslock taps Esc, holds Ctrl
        cfg = kc.parse_text("[main]\ncapslock = overload(control, esc)\n")
        self.assertEqual(cfg.holds, [("capslock", "control", "mod", "esc")])

    def test_layer_is_pure_modifier_no_tap(self):
        cfg = kc.parse_text("[main]\ncapslock = layer(control)\n")
        self.assertEqual(cfg.holds, [("capslock", "control", "mod", None)])

    def test_plain_remap(self):
        cfg = kc.parse_text("[main]\nleftcontrol = capslock\n")
        self.assertEqual(cfg.remaps, {"leftcontrol": "capslock"})
        self.assertEqual(cfg.holds, [])

    def test_toggle_chord(self):
        cfg = kc.parse_text("[main]\nleftshift+rightshift = toggle(game)\n")
        self.assertEqual(cfg.chords, [("leftshift+rightshift", "game")])

    def test_layer_section_overrides(self):
        cfg = kc.parse_text("[nav]\nh = left\nj = down\n")
        self.assertEqual(cfg.layers["nav"], {"h": "left", "j": "down"})

    def test_ids_collected(self):
        cfg = kc.parse_text("[ids]\n04fe:0021\n04fe:0202\n")
        self.assertEqual(cfg.ids, ["04fe:0021", "04fe:0202"])

    def test_full_line_comments_and_blanks_ignored(self):
        cfg = kc.parse_text("# a comment\n\n[main]\n# another\nf = lettermod(nav, f, 150, 200)\n")
        self.assertEqual(len(cfg.holds), 1)

    def test_empty_layer_section_registered(self):
        cfg = kc.parse_text("[sym]\n")
        self.assertIn("sym", cfg.layers)


class TestPrettify(unittest.TestCase):
    def test_shift_number_to_symbol(self):
        self.assertEqual(kc.prettify("S-9"), "(")
        self.assertEqual(kc.prettify("S-0"), ")")
        self.assertEqual(kc.prettify("S-minus"), "_")

    def test_shift_brace(self):
        self.assertEqual(kc.prettify("S-leftbrace"), "{")
        self.assertEqual(kc.prettify("S-rightbrace"), "}")

    def test_ctrl_arrow(self):
        self.assertEqual(kc.prettify("C-left"), "⌃←")
        self.assertEqual(kc.prettify("C-right"), "⌃→")

    def test_plain_keynames(self):
        self.assertEqual(kc.prettify("leftbrace"), "[")
        self.assertEqual(kc.prettify("backspace"), "⌫")
        self.assertEqual(kc.prettify("esc"), "Esc")

    def test_letter_uppercased(self):
        self.assertEqual(kc.prettify("a"), "A")


class TestLayoutSelection(unittest.TestCase):
    def test_hhkb_name_picks_hhkb(self):
        phys, prof = kc.layout_for("/etc/keyd/hhkb.conf")
        self.assertIs(phys, kc.HHKB)
        self.assertEqual(prof, "HHKB 60%")

    def test_other_name_picks_ansi(self):
        phys, prof = kc.layout_for("/etc/keyd/laptop.conf")
        self.assertIs(phys, kc.ANSI60)
        self.assertEqual(prof, "ANSI 60%")


if __name__ == "__main__":
    unittest.main()
