//! Unit tests for the edit module (CST parse/serialize, mutation ops, refs).

    use super::*;

    fn kvp(s: &str) -> (&str, Option<&str>) {
        parse_kvp(s)
    }

    // ------------------------------------------------------- parse_kvp parity (ini.c)
    #[test]
    fn kvp_plain() {
        assert_eq!(kvp("a = b"), ("a", Some("b")));
        assert_eq!(kvp("a=b"), ("a", Some("b")));
        assert_eq!(kvp("a\t =\t b"), ("a", Some("b")));
    }

    #[test]
    fn kvp_value_may_contain_equals() {
        assert_eq!(kvp("a = b=c"), ("a", Some("b=c")));
    }

    #[test]
    fn kvp_equals_key_special_case() {
        // ini.c: "Allow the first character to be = as a special case."
        assert_eq!(kvp("= = a"), ("=", Some("a")));
        assert_eq!(kvp("==x"), ("=", Some("x")));
        assert_eq!(kvp("="), ("=", None));
        assert_eq!(kvp("=foo"), ("=foo", None));
        assert_eq!(kvp("=foo=bar"), ("=foo", Some("bar")));
    }

    #[test]
    fn kvp_valueless() {
        assert_eq!(kvp("0123:4567"), ("0123:4567", None));
        assert_eq!(kvp("*"), ("*", None));
    }

    #[test]
    fn kvp_empty_value() {
        assert_eq!(kvp("key ="), ("key", Some("")));
    }

    #[test]
    fn kvp_key_with_internal_space() {
        // Only the *trailing* space/tab run before '=' is trimmed (last_space logic).
        assert_eq!(kvp("a b = c"), ("a b", Some("c")));
    }

    // ------------------------------------------------------------ line classification
    #[test]
    fn header_grammar_matches_ini_c() {
        let cfg = EditConfig::parse("[a]b]\n[#x]\n#[y]\n[main ]\n");
        let names: Vec<&str> = cfg.sections.iter().map(|s| s.name.as_str()).collect();
        // `[a]b]` names `a]b`; `[#x]` is a header (the '[' case wins); `#[y]` is a
        // comment; `[main ]` keeps its inner space and is NOT [main].
        assert_eq!(names, ["a]b", "#x", "main "]);
        assert_eq!(cfg.sections[2].kind, SectionKind::Layer);
        assert!(matches!(cfg.sections[1].entries[0].kind, EntryKind::Comment));
    }

    #[test]
    fn unterminated_bracket_is_a_binding() {
        // ini.c: '[' without a closing ']' falls through to parse_kvp.
        let cfg = EditConfig::parse("[main]\n[foo = bar\n");
        match &cfg.sections[0].entries[0].kind {
            EntryKind::Binding { key, val, .. } => {
                assert_eq!(key, "[foo");
                assert_eq!(val.as_deref(), Some("bar"));
            }
            other => panic!("expected Binding, got {other:?}"),
        }
    }

    #[test]
    fn section_kinds_and_qualifiers() {
        let cfg = EditConfig::parse("[ids]\n[global]\n[aliases]\n[main]\n[nav:C]\n[a+b]\n");
        let kinds: Vec<SectionKind> = cfg.sections.iter().map(|s| s.kind).collect();
        use SectionKind::*;
        assert_eq!(kinds, [Ids, Global, Aliases, Main, Layer, Composite]);
        assert_eq!(cfg.sections[4].base_name(), "nav");
        assert_eq!(cfg.sections[4].qualifier(), Some("C"));
        assert_eq!(cfg.sections[3].qualifier(), None);
    }

    #[test]
    fn classification_is_section_aware() {
        let cfg = EditConfig::parse(
            "[global]\nmacro_timeout = 600\n[main]\na = b\nb = noop\nc = layer(nav)\n",
        );
        let typed = |s: usize, e: usize| match &cfg.sections[s].entries[e].kind {
            EntryKind::Binding { typed, .. } => typed.clone(),
            other => panic!("expected Binding, got {other:?}"),
        };
        assert_eq!(typed(0, 0), Typed::Raw); // [global] value is not a remap
        assert_eq!(typed(1, 0), Typed::Remap("b".into()));
        assert_eq!(typed(1, 1), Typed::Noop);
        assert_eq!(typed(1, 2), Typed::Raw); // actions not yet modeled
    }

    // ----------------------------------------------------------------------- mutation
    #[test]
    fn set_binding_regenerates_one_line_only() {
        let src = "# my config\n[ids]\n0123:4567\n\n[main]\n# capslock\ncapslock = esc\na = b\n";
        let mut cfg = EditConfig::parse(src);
        assert!(cfg.section_mut("main").unwrap().set_binding("a", "c"));
        let out = cfg.serialize();
        assert_eq!(out, src.replace("a = b", "a = c"));
    }

    #[test]
    fn set_binding_edits_the_last_duplicate() {
        // keyd applies entries in order; the last assignment wins, so that's the one
        // the editor must touch.
        let mut cfg = EditConfig::parse("[main]\na = x\na = y\n");
        assert!(cfg.section_mut("main").unwrap().set_binding("a", "z"));
        assert_eq!(cfg.serialize(), "[main]\na = x\na = z\n");
    }

    #[test]
    fn set_binding_missing_key_is_a_noop() {
        let src = "[main]\na = b\n";
        let mut cfg = EditConfig::parse(src);
        assert!(!cfg.section_mut("main").unwrap().set_binding("q", "x"));
        assert_eq!(cfg.serialize(), src);
    }

    #[test]
    fn add_binding_lands_before_trailing_blank_separator() {
        // The blank line separating sections must stay at the section's end.
        let src = "[main]\na = b\n\n[nav]\nh = left\n";
        let mut cfg = EditConfig::parse(src);
        cfg.target_section_mut("main").unwrap().set_or_add_binding("q", "esc", Eol::Lf);
        assert_eq!(cfg.serialize(), "[main]\na = b\nq = esc\n\n[nav]\nh = left\n");
    }

    #[test]
    fn add_binding_preserves_missing_final_newline() {
        let mut cfg = EditConfig::parse("[main]\na = b");
        cfg.target_section_mut("main").unwrap().set_or_add_binding("q", "esc", Eol::Lf);
        // The old last line gains its newline; the new last line takes the None.
        assert_eq!(cfg.serialize(), "[main]\na = b\nq = esc");
    }

    #[test]
    fn add_binding_to_empty_section_and_crlf_style() {
        let mut cfg = EditConfig::parse("[main]\r\n");
        cfg.target_section_mut("main").unwrap().set_or_add_binding("a", "b", Eol::CrLf);
        assert_eq!(cfg.serialize(), "[main]\r\na = b\r\n");
    }

    #[test]
    fn target_section_picks_the_last_duplicate() {
        // keyd merges duplicate sections in order; an appended line must land in
        // the LAST one so it out-ranks every earlier assignment.
        let src = "[nav]\nh = left\n[nav:C]\nj = down\n";
        let mut cfg = EditConfig::parse(src);
        cfg.target_section_mut("nav").unwrap().set_or_add_binding("k", "up", Eol::Lf);
        assert_eq!(cfg.serialize(), "[nav]\nh = left\n[nav:C]\nj = down\nk = up\n");
        // [ids]/[global] are never edit targets.
        assert!(cfg.target_section_mut("ids").is_none());
        assert!(cfg.target_section_mut("missing").is_none());
    }

    #[test]
    fn get_binding_last_duplicate_wins() {
        let cfg = EditConfig::parse("[main]\na = x\na = y\n");
        assert_eq!(cfg.sections[0].get_binding("a"), Some("y"));
        assert_eq!(cfg.sections[0].get_binding("q"), None);
    }

    #[test]
    fn dirty_tracks_edits_and_adds() {
        let mut cfg = EditConfig::parse("[main]\na = b\n");
        assert!(!cfg.is_dirty());
        cfg.target_section_mut("main").unwrap().set_or_add_binding("a", "c", Eol::Lf);
        assert!(cfg.is_dirty());
        let mut cfg = EditConfig::parse("[main]\n");
        cfg.target_section_mut("main").unwrap().set_or_add_binding("q", "esc", Eol::Lf);
        assert!(cfg.is_dirty());
    }

    // ----------------------------------------------------------------- remove/clear
    #[test]
    fn remove_binding_drops_just_that_line() {
        let src = "[main]\n# capslock\ncapslock = esc\na = b\n";
        let mut cfg = EditConfig::parse(src);
        assert!(cfg.section_mut("main").unwrap().remove_binding("capslock"));
        // The line goes; its comment is left as-is (we don't guess intent).
        assert_eq!(cfg.serialize(), "[main]\n# capslock\na = b\n");
    }

    #[test]
    fn remove_binding_drops_every_duplicate() {
        // keyd is last-wins, so transparency needs ALL assignments gone.
        let mut cfg = EditConfig::parse("[main]\na = x\nb = y\na = z\n");
        assert!(cfg.section_mut("main").unwrap().remove_binding("a"));
        assert_eq!(cfg.serialize(), "[main]\nb = y\n");
    }

    #[test]
    fn remove_binding_missing_key_is_a_noop() {
        let src = "[main]\na = b\n";
        let mut cfg = EditConfig::parse(src);
        assert!(!cfg.section_mut("main").unwrap().remove_binding("q"));
        assert_eq!(cfg.serialize(), src);
        assert!(!cfg.is_dirty());
    }

    #[test]
    fn remove_only_change_still_marks_dirty() {
        // A pure removal leaves no entry to carry a dirty flag — the section-level
        // flag is what keeps is_dirty() honest (else save/apply would stay off).
        let mut cfg = EditConfig::parse("[main]\na = b\n");
        assert!(!cfg.is_dirty());
        assert!(cfg.section_mut("main").unwrap().remove_binding("a"));
        assert!(cfg.is_dirty());
    }

    #[test]
    fn clear_binding_spans_every_merged_section() {
        // [nav] and [nav:C] both feed the "nav" board; clearing only one would
        // leave the key bound, so clear_binding must hit both.
        let src = "[nav]\nh = left\n[nav:C]\nh = right\nj = down\n";
        let mut cfg = EditConfig::parse(src);
        assert!(cfg.clear_binding("nav", "h"));
        assert_eq!(cfg.serialize(), "[nav]\n[nav:C]\nj = down\n");
        assert!(cfg.is_dirty());
    }

    #[test]
    fn clear_binding_missing_is_a_noop() {
        let src = "[nav]\nh = left\n";
        let mut cfg = EditConfig::parse(src);
        assert!(!cfg.clear_binding("nav", "q"));
        assert!(!cfg.clear_binding("missing", "h"));
        assert_eq!(cfg.serialize(), src);
        assert!(!cfg.is_dirty());
    }

    // ------------------------------------------------------------------- labels (E/v1.3)
    #[test]
    fn set_label_inserts_canonical_comment_before_binding() {
        let mut cfg = EditConfig::parse("[main]\ntab = layer(nav)\na = b\n");
        assert!(cfg.set_label("main", "tab", "Tab L"));
        assert_eq!(
            cfg.serialize(),
            "[main]\n# keyd-viz: tab = Tab L\ntab = layer(nav)\na = b\n"
        );
        assert!(cfg.is_dirty());
    }

    #[test]
    fn set_label_rewrites_existing_in_place() {
        let src = "[main]\n# keyd-viz: tab = Old\ntab = layer(nav)\n";
        let mut cfg = EditConfig::parse(src);
        assert!(cfg.set_label("main", "tab", "New Name"));
        // No duplicate, position preserved, only the text changed.
        assert_eq!(cfg.serialize(), "[main]\n# keyd-viz: tab = New Name\ntab = layer(nav)\n");
    }

    #[test]
    fn set_label_round_trips_and_passes_keyd_grammar() {
        // Our own emitted label line re-parses to the same comment entry.
        let mut cfg = EditConfig::parse("[main]\ntab = layer(nav)\n");
        cfg.set_label("main", "tab", "Tab L");
        assert!(round_trips(&cfg.serialize()));
    }

    #[test]
    fn set_label_on_orphan_key_appends_at_section_end() {
        // No binding for the key → comment lands at the section's end (orphan-tolerant).
        let mut cfg = EditConfig::parse("[main]\na = b\n");
        assert!(cfg.set_label("main", "tab", "Tab L"));
        assert_eq!(cfg.serialize(), "[main]\na = b\n# keyd-viz: tab = Tab L\n");
    }

    #[test]
    fn set_label_creates_main_when_board_is_include_only() {
        let mut cfg = EditConfig::parse("[ids]\n0123:4567\n");
        assert!(cfg.set_label("main", "tab", "Tab L"));
        assert_eq!(cfg.serialize(), "[ids]\n0123:4567\n\n[main]\n# keyd-viz: tab = Tab L\n");
    }

    #[test]
    fn set_label_on_missing_layer_is_false() {
        let mut cfg = EditConfig::parse("[main]\na = b\n");
        assert!(!cfg.set_label("nope", "a", "X"));
    }

    #[test]
    fn set_label_rewrites_the_label_not_a_later_plain_comment() {
        // label_index must match the key's label line (key && is-label), not the
        // last comment of any kind — else a trailing note gets clobbered.
        let mut cfg =
            EditConfig::parse("[main]\n# keyd-viz: tab = Old\ntab = layer(nav)\n# just a note\n");
        assert!(cfg.set_label("main", "tab", "New"));
        assert_eq!(
            cfg.serialize(),
            "[main]\n# keyd-viz: tab = New\ntab = layer(nav)\n# just a note\n"
        );
    }

    #[test]
    fn clear_label_leaves_unrelated_comments() {
        // clear_label's retain must drop only the key's label (key && is-label),
        // never every comment in the section.
        let mut cfg =
            EditConfig::parse("[main]\n# a plain note\n# keyd-viz: tab = Tab L\ntab = layer(nav)\n");
        assert!(cfg.clear_label("main", "tab"));
        assert_eq!(cfg.serialize(), "[main]\n# a plain note\ntab = layer(nav)\n");
    }

    #[test]
    fn set_label_on_orphan_preserves_missing_final_newline() {
        // Appending a label at section end must repair the prior line's missing
        // newline (push_comment `at == len`) without fusing the comment onto it.
        let mut cfg = EditConfig::parse("[main]\na = b"); // no trailing newline
        assert!(cfg.set_label("main", "tab", "Tab L"));
        assert_eq!(cfg.serialize(), "[main]\na = b\n# keyd-viz: tab = Tab L");
    }

    #[test]
    fn set_label_lands_beside_the_winning_binding_in_a_merged_section() {
        // The effective binding for `h` lives in [nav:C]; the label must sit with it.
        let src = "[nav]\nh = left\n[nav:C]\nh = right\n";
        let mut cfg = EditConfig::parse(src);
        assert!(cfg.set_label("nav", "h", "Home"));
        assert_eq!(
            cfg.serialize(),
            "[nav]\nh = left\n[nav:C]\n# keyd-viz: h = Home\nh = right\n"
        );
    }

    #[test]
    fn empty_text_clears_the_label() {
        let src = "[main]\n# keyd-viz: tab = Tab L\ntab = layer(nav)\na = b\n";
        let mut cfg = EditConfig::parse(src);
        assert!(cfg.set_label("main", "tab", "   "));
        assert_eq!(cfg.serialize(), "[main]\ntab = layer(nav)\na = b\n");
    }

    #[test]
    fn clear_label_spans_every_merged_section_and_leaves_bindings() {
        let src = "[nav]\n# keyd-viz: h = A\nh = left\n[nav:C]\n# keyd-viz: h = B\nh = right\n";
        let mut cfg = EditConfig::parse(src);
        assert!(cfg.clear_label("nav", "h"));
        assert_eq!(cfg.serialize(), "[nav]\nh = left\n[nav:C]\nh = right\n");
        assert!(cfg.is_dirty());
    }

    #[test]
    fn set_label_with_identical_text_is_not_dirty() {
        // Re-setting the same label must not flag the config dirty (no byte change).
        let mut cfg = EditConfig::parse("[main]\n# keyd-viz: tab = Tab L\ntab = layer(nav)\n");
        assert!(cfg.set_label("main", "tab", "Tab L"));
        assert!(!cfg.is_dirty(), "an identical rewrite is a no-op");
        // A genuine change still dirties.
        assert!(cfg.set_label("main", "tab", "Tab R"));
        assert!(cfg.is_dirty());
    }

    #[test]
    fn set_label_collapses_interior_newlines() {
        // A newline in label text would otherwise split into a bogus second line keyd
        // rejects. It's collapsed to a space, keeping the comment on one line.
        let mut cfg = EditConfig::parse("[main]\ntab = layer(nav)\n");
        cfg.set_label("main", "tab", "Tab\nLine\rTwo");
        let s = cfg.serialize();
        assert_eq!(s, "[main]\n# keyd-viz: tab = Tab Line Two\ntab = layer(nav)\n");
        assert!(round_trips(&s));
    }

    #[test]
    fn clear_label_missing_is_a_noop() {
        let src = "[main]\ntab = layer(nav)\n";
        let mut cfg = EditConfig::parse(src);
        assert!(!cfg.clear_label("main", "tab"));
        assert_eq!(cfg.serialize(), src);
        assert!(!cfg.is_dirty());
    }

    #[test]
    fn label_text_with_spaces_hashes_and_equals_is_preserved() {
        let mut cfg = EditConfig::parse("[main]\ntab = layer(nav)\n");
        cfg.set_label("main", "tab", "Nav = #1 (hold)");
        // Re-derive the label and confirm the full text survived parse_kvp.
        let s = cfg.serialize();
        let comment = s.lines().find(|l| l.contains("keyd-viz")).unwrap();
        assert_eq!(parse_label_comment(comment), Some(("tab", "Nav = #1 (hold)")));
    }

    fn orphans(src: &str) -> Vec<(String, String, String)> {
        EditConfig::parse(src)
            .orphan_layer_refs()
            .into_iter()
            .map(|o| (o.section, o.key, o.layer))
            .collect()
    }

    #[test]
    fn orphan_layer_reference_is_flagged() {
        let got = orphans("[main]\ncapslock = layer(symbols)\n");
        assert_eq!(got, vec![("main".into(), "capslock".into(), "symbols".into())]);
    }

    #[test]
    fn defined_layer_is_not_an_orphan() {
        // The reference resolves once the section exists — even modifier-qualified
        // (`[nav:C]` defines base `nav`).
        assert!(orphans("[main]\na = layer(nav)\n[nav:C]\nh = left\n").is_empty());
        assert!(orphans("[main]\na = toggle(nav)\n[nav]\n").is_empty());
    }

    #[test]
    fn modifier_target_is_never_an_orphan() {
        // overload(mod, tap) / layer(mod) target keyd's built-in modifier layers —
        // valid with no matching section.
        assert!(orphans("[main]\na = overload(shift, esc)\n").is_empty());
        assert!(orphans("[main]\na = oneshot(control)\n").is_empty());
    }

    #[test]
    fn composite_and_non_layer_values_are_skipped() {
        // a+b composite targets (subtle definition rules) and plain/non-fn values
        // never raise a false alarm.
        assert!(orphans("[main]\na = layer(nav+sym)\n").is_empty());
        assert!(orphans("[main]\na = esc\nb = C-c\nc = macro(h i)\n").is_empty());
    }

    #[test]
    fn taphold_layer_target_is_flagged_but_its_tap_is_not() {
        // overload(LAYER, tap): only arg0 is a layer; the tap key must not be scanned.
        let got = orphans("[main]\ncapslock = overload(nav, esc)\n");
        assert_eq!(got, vec![("main".into(), "capslock".into(), "nav".into())]);
    }

    // --------------------------------------------------------------- add/remove layer
    #[test]
    fn add_layer_appends_a_blank_then_header() {
        let mut cfg = EditConfig::parse("[ids]\n*\n\n[main]\na = b\n");
        cfg.add_layer("nav").unwrap();
        // A blank separator, then the new empty section, in the file's LF style.
        assert_eq!(cfg.serialize(), "[ids]\n*\n\n[main]\na = b\n\n[nav]\n");
        assert!(cfg.is_dirty());
        // It's a real editable target now (set_or_add_binding lands in it).
        cfg.target_section_mut("nav").unwrap().set_or_add_binding("h", "left", Eol::Lf);
        assert_eq!(cfg.serialize(), "[ids]\n*\n\n[main]\na = b\n\n[nav]\nh = left\n");
    }

    #[test]
    fn add_layer_into_empty_file_has_no_separator() {
        let mut cfg = EditConfig::parse("");
        cfg.add_layer("nav").unwrap();
        assert_eq!(cfg.serialize(), "[nav]\n");
    }

    #[test]
    fn add_layer_does_not_stack_a_second_blank() {
        // File already ends in a blank line: the new header reuses it, no double gap.
        let mut cfg = EditConfig::parse("[main]\na = b\n\n");
        cfg.add_layer("nav").unwrap();
        assert_eq!(cfg.serialize(), "[main]\na = b\n\n[nav]\n");
    }

    #[test]
    fn add_layer_preserves_crlf_and_missing_final_newline() {
        // CRLF style is inferred and the missing final newline is preserved: the old
        // last line gains its terminator, the new header becomes the unterminated tail.
        let mut cfg = EditConfig::parse("[main]\r\na = b");
        cfg.add_layer("nav").unwrap();
        assert_eq!(cfg.serialize(), "[main]\r\na = b\r\n\r\n[nav]");
    }

    #[test]
    fn add_layer_rejects_bad_names_and_duplicates() {
        let mut cfg = EditConfig::parse("[ids]\n*\n[main]\na = b\n[nav]\n");
        assert!(cfg.add_layer("").unwrap_err().contains("empty"));
        assert!(cfg.add_layer("  ").unwrap_err().contains("empty"));
        assert!(cfg.add_layer("a b").unwrap_err().contains("letters"));
        assert!(cfg.add_layer("a:b").unwrap_err().contains("letters"));
        assert!(cfg.add_layer("a+b").unwrap_err().contains("letters"));
        assert!(cfg.add_layer("ids").unwrap_err().contains("reserved"));
        assert!(cfg.add_layer("nav").unwrap_err().contains("exists"));
        // A modifier-qualified section already defines the base, so it's a duplicate.
        cfg.add_layer("sym").unwrap();
        // Whitespace is trimmed before all checks.
        assert!(cfg.add_layer("  sym  ").unwrap_err().contains("exists"));
        // Nothing above mutated the file except the one successful `sym`.
        assert!(cfg.section("sym").is_some());
    }

    #[test]
    fn add_layer_base_dup_check_spans_qualified_sections() {
        let mut cfg = EditConfig::parse("[main]\na = b\n[nav:C]\nh = left\n");
        // [nav:C] defines base `nav`; creating `[nav]` would silently merge.
        assert!(cfg.add_layer("nav").unwrap_err().contains("exists"));
    }

    #[test]
    fn remove_layer_drops_all_sections_for_the_base() {
        let src = "[ids]\n*\n\n[main]\na = layer(nav)\n\n[nav]\nh = left\n[nav:C]\nj = down\n";
        let mut cfg = EditConfig::parse(src);
        assert!(cfg.remove_layer("nav"));
        // Both [nav] and [nav:C] go; the dangling layer(nav) ref is left for the
        // orphan check to surface, not silently rewritten.
        assert_eq!(cfg.serialize(), "[ids]\n*\n\n[main]\na = layer(nav)\n\n");
        assert!(cfg.is_dirty());
        assert_eq!(cfg.orphan_layer_refs().len(), 1);
    }

    #[test]
    fn remove_layer_leaves_composites_and_missing_is_noop() {
        let mut cfg = EditConfig::parse("[main]\na = b\n[nav+sym]\nx = y\n");
        // base "nav" must not take the composite [nav+sym] with it.
        assert!(!cfg.remove_layer("nav"));
        assert!(!cfg.is_dirty());
        assert_eq!(cfg.serialize(), "[main]\na = b\n[nav+sym]\nx = y\n");
    }

    #[test]
    fn references_to_finds_every_activator() {
        let src = "[main]\na = layer(nav)\nb = oneshot(nav)\nc = overload(nav, esc)\n\
                   d = esc\n[fn]\ng = toggle(nav)\n[nav]\nh = left\n";
        let cfg = EditConfig::parse(src);
        let refs = cfg.references_to("nav");
        assert_eq!(
            refs,
            vec![
                ("main".into(), "a".into()),
                ("main".into(), "b".into()),
                ("main".into(), "c".into()),
                ("fn".into(), "g".into()),
            ]
        );
        assert!(cfg.references_to("sym").is_empty());
    }

    #[test]
    fn rename_layer_rewrites_headers_and_every_reference() {
        let src = "[ids]\n*\n\n[main]\na = layer(nav)\nb = oneshot(nav)\n\
                   c = lettermod(nav, 150, 200)\nd = overload(shift, esc)\n\
                   [nav]\nh = left\n[nav:C]\nj = down\n";
        let mut cfg = EditConfig::parse(src);
        assert_eq!(cfg.rename_layer("nav", "symbols").unwrap(), 3);
        assert_eq!(
            cfg.serialize(),
            "[ids]\n*\n\n[main]\na = layer(symbols)\nb = oneshot(symbols)\n\
             c = lettermod(symbols, 150, 200)\nd = overload(shift, esc)\n\
             [symbols]\nh = left\n[symbols:C]\nj = down\n"
        );
        assert!(cfg.is_dirty());
        // No orphans: every reference followed the rename, and `shift` was never a ref.
        assert!(cfg.orphan_layer_refs().is_empty());
    }

    #[test]
    fn rename_layer_covers_the_momentary_layer_variants() {
        // The …m/…k variants take the layer as arg 0 just like the plain forms; a miss
        // here would leave a dangling reference and report success (a config-corruption
        // bug caught in review). Nested refs inside an action arg are covered separately
        // by `rename_rewrites_layer_refs_nested_in_an_action`.
        let src = "[main]\na = layerm(nav, x)\nb = oneshotm(nav, x)\nc = oneshotk(nav, x)\n\
                   d = swapm(nav, x)\ne = togglem(nav, x)\n[nav]\nh = left\n";
        let mut cfg = EditConfig::parse(src);
        assert_eq!(cfg.rename_layer("nav", "sym").unwrap(), 5);
        assert_eq!(
            cfg.serialize(),
            "[main]\na = layerm(sym, x)\nb = oneshotm(sym, x)\nc = oneshotk(sym, x)\n\
             d = swapm(sym, x)\ne = togglem(sym, x)\n[sym]\nh = left\n"
        );
        assert!(cfg.orphan_layer_refs().is_empty());
    }

    #[test]
    fn orphan_scan_flags_a_missing_layer_via_a_momentary_variant() {
        // The orphan net shares LAYER_FNS, so it must see these forms too.
        let cfg = EditConfig::parse("[main]\na = togglem(gone, x)\n");
        let orphans = cfg.orphan_layer_refs();
        assert_eq!(orphans.len(), 1);
        assert_eq!(orphans[0].layer, "gone");
    }

    #[test]
    fn rename_layer_rewrites_composite_constituents() {
        let src = "[main]\nx = y\n[nav]\nh = left\n[sym]\nk = up\n[nav+sym]\nq = w\n";
        let mut cfg = EditConfig::parse(src);
        cfg.rename_layer("nav", "navi").unwrap();
        // The composite's `nav` part is rewritten so it doesn't dangle; `sym` is left.
        assert_eq!(
            cfg.serialize(),
            "[main]\nx = y\n[navi]\nh = left\n[sym]\nk = up\n[navi+sym]\nq = w\n"
        );
    }

    #[test]
    fn rename_rewrites_layer_refs_nested_in_an_action() {
        // `overloadi`'s arg 0 and arg 1 are actions, and `overload`'s arg 1 is an action —
        // a layer named inside any of them must be followed, or rename silently corrupts.
        let src = "[main]\n\
                   a = overloadi(esc, layer(nav), 200)\n\
                   b = overload(meta, oneshot(nav))\n\
                   c = overloadi(layer(nav), toggle(nav), 200)\n\
                   [nav]\nh = left\n";
        let mut cfg = EditConfig::parse(src);
        // 1 in a, 1 in b, 2 in c = 4 references rewritten.
        assert_eq!(cfg.rename_layer("nav", "sym").unwrap(), 4);
        assert_eq!(
            cfg.serialize(),
            "[main]\n\
             a = overloadi(esc, layer(sym), 200)\n\
             b = overload(meta, oneshot(sym))\n\
             c = overloadi(layer(sym), toggle(sym), 200)\n\
             [sym]\nh = left\n"
        );
        assert!(cfg.orphan_layer_refs().is_empty());
    }

    #[test]
    fn rename_handles_deep_nesting_and_a_shrinking_name() {
        // Three refs to `nav` in one value, the deepest two levels down (an `overload`
        // inside an `overloadi` action slot), rewritten to a SHORTER name — the
        // right-to-left splice must keep earlier offsets valid as the string shrinks.
        let src = "[main]\na = overloadi(layer(nav), overload(nav, oneshot(nav)), 200)\n[nav]\nh = left\n";
        let mut cfg = EditConfig::parse(src);
        assert_eq!(cfg.rename_layer("nav", "x").unwrap(), 3);
        assert_eq!(
            cfg.serialize(),
            "[main]\na = overloadi(layer(x), overload(x, oneshot(x)), 200)\n[x]\nh = left\n"
        );
    }

    #[test]
    fn orphan_scan_sees_a_layer_ref_nested_in_an_action() {
        // The orphan net shares the recursive span finder, so a missing layer named only
        // inside an `overloadi` hold descriptor must still be flagged.
        let cfg = EditConfig::parse("[main]\na = overloadi(esc, layer(gone), 200)\n");
        let orphans = cfg.orphan_layer_refs();
        assert_eq!(orphans.len(), 1);
        assert_eq!(orphans[0].layer, "gone");
    }

    #[test]
    fn span_finder_does_not_descend_into_macro_or_command_text() {
        // A literal `layer(...)` inside macro/command text is NOT a layer activation — the
        // scanner must not mistake it for one (precision over recall). No `[oops]` section
        // exists, so a false positive would surface as a phantom orphan.
        let cfg = EditConfig::parse(
            "[main]\n\
             a = overload(nav, macro(layer(oops)))\n\
             b = command(echo layer(oops))\n\
             [nav]\nh = left\n",
        );
        // Only `nav` is a real ref (defined), so no orphans — `oops` is never seen.
        assert!(cfg.orphan_layer_refs().is_empty(), "{:?}", cfg.orphan_layer_refs());
    }

    #[test]
    fn dangling_composite_is_flagged_until_its_part_exists() {
        // keyd rejects `[nav+sym]` when `sym` isn't a real layer (exit 255).
        let mut cfg = EditConfig::parse("[main]\nx = y\n[nav]\nh = left\n[nav+sym]\nq = w\n");
        assert_eq!(cfg.dangling_composites(), vec![("nav+sym".to_string(), "sym".to_string())]);
        // Defining the missing part clears it; `main` is implicitly defined (no flag).
        cfg.add_layer("sym").unwrap();
        assert!(cfg.dangling_composites().is_empty());
        // A composite over the implicit base board is fine without an explicit [main].
        let cfg2 = EditConfig::parse("[ids]\n*\n[nav]\nh = left\n[main+nav]\nq = w\n");
        assert!(cfg2.dangling_composites().is_empty());
    }

    #[test]
    fn set_layer_binding_edits_in_place_across_a_merged_section() {
        // `a` lives only in the earlier `[main]` block; editing it must rewrite that line,
        // not append a shadowed duplicate to the later block.
        let mut cfg = EditConfig::parse("[main]\na = b\n[nav]\nh = left\n[main]\nc = d\n");
        assert!(cfg.set_layer_binding("main", "a", "z"));
        assert_eq!(cfg.serialize(), "[main]\na = z\n[nav]\nh = left\n[main]\nc = d\n");
        // A brand-new key still appends to the LAST (winning) block.
        assert!(cfg.set_layer_binding("main", "e", "f"));
        assert_eq!(cfg.serialize(), "[main]\na = z\n[nav]\nh = left\n[main]\nc = d\ne = f\n");
    }

    #[test]
    fn set_layer_binding_appends_in_the_files_crlf_style() {
        // A freshly-created `[main]` header carries Eol::None on a no-final-newline file;
        // the appended binding must take the file's CRLF, not default to LF.
        let mut cfg = EditConfig::parse("[ids]\r\n*\r\n[nav]\r\nh = left");
        assert!(cfg.set_layer_binding("main", "a", "b")); // creates [main], then appends
        assert_eq!(cfg.serialize(), "[ids]\r\n*\r\n[nav]\r\nh = left\r\n\r\n[main]\r\na = b");
    }

    #[test]
    fn add_layer_main_creates_the_base_board_kind() {
        let mut cfg = EditConfig::parse("[ids]\n*\n[nav]\nh = left\n");
        cfg.add_layer("main").unwrap();
        let main = cfg.sections.iter().find(|s| s.name == "main").unwrap();
        assert_eq!(main.kind, SectionKind::Main);
    }

    #[test]
    fn rename_layer_rejects_bad_names_and_collisions() {
        let mut cfg = EditConfig::parse("[main]\na = b\n[nav]\nh = left\n[sym]\nk = up\n");
        assert!(cfg.rename_layer("nav", "").unwrap_err().contains("empty"));
        assert!(cfg.rename_layer("nav", "a b").unwrap_err().contains("letters"));
        assert!(cfg.rename_layer("nav", "ids").unwrap_err().contains("reserved"));
        assert!(cfg.rename_layer("nav", "nav").unwrap_err().contains("unchanged"));
        assert!(cfg.rename_layer("nav", "sym").unwrap_err().contains("exists"));
        // Nothing changed on any rejection.
        assert!(!cfg.is_dirty());
        assert_eq!(cfg.serialize(), "[main]\na = b\n[nav]\nh = left\n[sym]\nk = up\n");
    }

    #[test]
    fn rename_layer_refuses_main_composite_and_missing() {
        let mut cfg = EditConfig::parse("[main]\na = b\n[nav+sym]\nx = y\n");
        // The base layer and composites aren't simple renames; a missing layer can't be.
        assert!(cfg.rename_layer("main", "base").unwrap_err().contains("renameable"));
        assert!(cfg.rename_layer("nav+sym", "combo").unwrap_err().contains("renameable"));
        assert!(cfg.rename_layer("ghost", "x").unwrap_err().contains("renameable"));
        assert!(!cfg.is_dirty());
    }

    #[test]
    fn rename_layer_qualified_only_and_blocks_existing_base() {
        // A layer that exists only as a qualified section is still renameable.
        let mut cfg = EditConfig::parse("[main]\na = layer(nav)\n[nav:C]\nj = down\n");
        assert_eq!(cfg.rename_layer("nav", "fn").unwrap(), 1);
        assert_eq!(cfg.serialize(), "[main]\na = layer(fn)\n[fn:C]\nj = down\n");
    }

    // -------------------------------------------------------------- starter config (§5.5)
    #[test]
    fn starter_config_is_minimal_valid_and_round_trips() {
        // A specific device id.
        let s = starter_config(&["04fe:0021"]);
        assert_eq!(s, "[ids]\n04fe:0021\n\n[main]\n");
        // Round-trips by construction (the §5.1 gate the create path runs).
        assert!(round_trips(&s));
        let cfg = EditConfig::parse(&s);
        // Has [ids] + [main], an empty main, no diagnostics, no orphans.
        assert!(cfg.diagnostics().is_empty());
        assert!(cfg.orphan_layer_refs().is_empty());
        assert_eq!(cfg.section("ids").unwrap().kind, SectionKind::Ids);
        let main = cfg.section("main").unwrap();
        assert_eq!(main.kind, SectionKind::Main);
        assert!(!main.entries.iter().any(|e| matches!(e.kind, EntryKind::Binding { .. })));
    }

    #[test]
    fn starter_config_wildcard_and_multi_id() {
        assert_eq!(starter_config(&["*"]), "[ids]\n*\n\n[main]\n");
        assert_eq!(
            starter_config(&["04fe:0021", "k:1234:5678"]),
            "[ids]\n04fe:0021\nk:1234:5678\n\n[main]\n"
        );
    }

    // -------------------------------------------------- mutation-gap regressions
    #[test]
    fn set_label_rewrites_the_in_board_label_not_a_later_foreign_one() {
        let mut cfg = EditConfig::parse(
            "[main]\n# keyd-viz: a = Old\na = b\n[nav]\n# keyd-viz: a = NavLabel\nh = left\n",
        );
        assert!(cfg.set_label("main", "a", "New"));
        assert_eq!(
            cfg.serialize(),
            "[main]\n# keyd-viz: a = New\na = b\n[nav]\n# keyd-viz: a = NavLabel\nh = left\n",
        );
    }

    #[test]
    fn set_layer_chord_appends_and_reports_success() {
        let mut cfg = EditConfig::parse("[main]\na = b\n");
        assert!(cfg.set_layer_chord("main", "j+k", "k+j", "down"));
        assert_eq!(cfg.serialize(), "[main]\na = b\nk+j = down\n");
        assert!(!cfg.set_layer_chord("ghost", "j+k", "k+j", "down"));
    }

    #[test]
    fn set_layer_chord_does_not_rewrite_a_differently_keyed_chord() {
        let mut cfg = EditConfig::parse("[main]\na+b = x\n");
        assert!(cfg.set_layer_chord("main", "j+k", "j+k", "down"));
        assert_eq!(cfg.serialize(), "[main]\na+b = x\nj+k = down\n");
    }

    #[test]
    fn set_layer_chord_rewrites_existing_chord_by_canonical_set() {
        let mut cfg = EditConfig::parse("[main]\nj+k = old\n");
        assert!(cfg.set_layer_chord("main", "j+k", "k+j", "down"));
        assert_eq!(cfg.serialize(), "[main]\nj+k = down\n");
    }

    #[test]
    fn set_global_option_writes_the_option() {
        let mut cfg = EditConfig::parse("[main]\na = b\n");
        cfg.set_global_option("macro_timeout", "600");
        let s = cfg.serialize();
        assert!(s.contains("[global]"), "{s}");
        assert!(s.contains("macro_timeout = 600"), "{s}");
    }

    #[test]
    fn add_layer_accepts_underscores_and_hyphens() {
        let mut cfg = EditConfig::parse("[main]\na = b\n");
        assert!(cfg.add_layer("nav_2").is_ok());
        assert!(cfg.add_layer("home-row").is_ok());
    }

    #[test]
    fn global_section_mut_creates_global_when_absent() {
        let mut cfg = EditConfig::parse("[main]\na = b\n");
        cfg.set_global_option("macro_timeout", "600");
        assert_eq!(cfg.serialize(), "[main]\na = b\n\n[global]\nmacro_timeout = 600\n");
    }

    #[test]
    fn rename_layer_accepts_underscores_and_hyphens() {
        let mut cfg = EditConfig::parse("[main]\na = layer(nav)\n[nav]\nh = left\n");
        assert!(cfg.rename_layer("nav", "nav_2").is_ok());
    }

    #[test]
    fn rename_layer_rewrites_a_pure_repeat_composite() {
        let mut cfg = EditConfig::parse("[main]\na = b\n[nav]\nh = left\n[nav+nav]\nx = y\n");
        cfg.rename_layer("nav", "sym").unwrap();
        assert_eq!(cfg.serialize(), "[main]\na = b\n[sym]\nh = left\n[sym+sym]\nx = y\n");
    }

    #[test]
    fn diagnostics_flags_missing_ids() {
        let cfg = EditConfig::parse("[main]\na = b\n");
        assert!(cfg.diagnostics().iter().any(|w| w.contains("no [ids]")));
    }

    #[test]
    fn diagnostics_clean_for_an_ids_only_config() {
        let cfg = EditConfig::parse("[ids]\n0123:4567\n");
        assert!(cfg.diagnostics().is_empty(), "{:?}", cfg.diagnostics());
    }

    #[test]
    fn rename_layer_handles_a_tab_padded_layer_ref() {
        let mut cfg = EditConfig::parse("[main]\na = layer(\tnav)\n[nav]\nh = left\n");
        assert_eq!(cfg.rename_layer("nav", "sym").unwrap(), 1);
        assert_eq!(cfg.serialize(), "[main]\na = layer(\tsym)\n[sym]\nh = left\n");
    }
