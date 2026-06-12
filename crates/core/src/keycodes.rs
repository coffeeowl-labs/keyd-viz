//! Linux input-event keycode → keyd key name.
//!
//! When we read keyd's virtual keyboard from evdev directly (instead of parsing
//! `keyd monitor`), each `EV_KEY` event carries a raw Linux keycode (`KEY_*` /
//! `struct input_event.code`). This maps it to the same primary name `keyd monitor`
//! would print, so the rest of the glow pipeline ([`crate::board`] canonicalisation,
//! `is_primary_keysym`) is unchanged.
//!
//! Transcribed verbatim from keyd v2.6.0 `src/keys.c` `keycode_table[code].name`
//! (primary name only — not alt/shifted). For codes < 256 keyd uses the evdev code
//! directly as the table index (`src/device.c:503-506`: "KEYD_* codes <256 correspond to
//! their evdev counterparts"), so this lookup is exact for real keyboard input. A handful
//! of entries (148, 149, 178, 195, 202, 203, 249-255) are keyd-internal synthetic codes
//! that never appear from real evdev; they're kept for fidelity but are dead in practice.

/// keyd v2.6.0 primary key name for a Linux input-event keycode (`keycode_table[code].name`).
/// `None` for unpopulated codes and for codes ≥ 256 (which keyd remaps internally).
pub fn keycode_name(code: u16) -> Option<&'static str> {
    match code {
        1 => Some("esc"),
        2 => Some("1"),
        3 => Some("2"),
        4 => Some("3"),
        5 => Some("4"),
        6 => Some("5"),
        7 => Some("6"),
        8 => Some("7"),
        9 => Some("8"),
        10 => Some("9"),
        11 => Some("0"),
        12 => Some("-"),
        13 => Some("="),
        14 => Some("backspace"),
        15 => Some("tab"),
        16 => Some("q"),
        17 => Some("w"),
        18 => Some("e"),
        19 => Some("r"),
        20 => Some("t"),
        21 => Some("y"),
        22 => Some("u"),
        23 => Some("i"),
        24 => Some("o"),
        25 => Some("p"),
        26 => Some("["),
        27 => Some("]"),
        28 => Some("enter"),
        29 => Some("leftcontrol"),
        30 => Some("a"),
        31 => Some("s"),
        32 => Some("d"),
        33 => Some("f"),
        34 => Some("g"),
        35 => Some("h"),
        36 => Some("j"),
        37 => Some("k"),
        38 => Some("l"),
        39 => Some(";"),
        40 => Some("'"),
        41 => Some("`"),
        42 => Some("leftshift"),
        43 => Some("\\"),
        44 => Some("z"),
        45 => Some("x"),
        46 => Some("c"),
        47 => Some("v"),
        48 => Some("b"),
        49 => Some("n"),
        50 => Some("m"),
        51 => Some(","),
        52 => Some("."),
        53 => Some("/"),
        54 => Some("rightshift"),
        55 => Some("kpasterisk"),
        56 => Some("leftalt"),
        57 => Some("space"),
        58 => Some("capslock"),
        59 => Some("f1"),
        60 => Some("f2"),
        61 => Some("f3"),
        62 => Some("f4"),
        63 => Some("f5"),
        64 => Some("f6"),
        65 => Some("f7"),
        66 => Some("f8"),
        67 => Some("f9"),
        68 => Some("f10"),
        69 => Some("numlock"),
        70 => Some("scrolllock"),
        71 => Some("kp7"),
        72 => Some("kp8"),
        73 => Some("kp9"),
        74 => Some("kpminus"),
        75 => Some("kp4"),
        76 => Some("kp5"),
        77 => Some("kp6"),
        78 => Some("kpplus"),
        79 => Some("kp1"),
        80 => Some("kp2"),
        81 => Some("kp3"),
        82 => Some("kp0"),
        83 => Some("kpdot"),
        84 => Some("iso-level3-shift"),
        85 => Some("zenkakuhankaku"),
        86 => Some("102nd"),
        87 => Some("f11"),
        88 => Some("f12"),
        89 => Some("ro"),
        90 => Some("katakana"),
        91 => Some("hiragana"),
        92 => Some("henkan"),
        93 => Some("katakanahiragana"),
        94 => Some("muhenkan"),
        95 => Some("kpjpcomma"),
        96 => Some("kpenter"),
        97 => Some("rightcontrol"),
        98 => Some("kpslash"),
        99 => Some("sysrq"),
        100 => Some("rightalt"),
        101 => Some("linefeed"),
        102 => Some("home"),
        103 => Some("up"),
        104 => Some("pageup"),
        105 => Some("left"),
        106 => Some("right"),
        107 => Some("end"),
        108 => Some("down"),
        109 => Some("pagedown"),
        110 => Some("insert"),
        111 => Some("delete"),
        112 => Some("macro"),
        113 => Some("mute"),
        114 => Some("volumedown"),
        115 => Some("volumeup"),
        116 => Some("power"),
        117 => Some("kpequal"),
        118 => Some("kpplusminus"),
        119 => Some("pause"),
        120 => Some("scale"),
        121 => Some("kpcomma"),
        122 => Some("hangeul"),
        123 => Some("hanja"),
        124 => Some("yen"),
        125 => Some("leftmeta"),
        126 => Some("rightmeta"),
        127 => Some("compose"),
        128 => Some("stop"),
        129 => Some("again"),
        130 => Some("props"),
        131 => Some("undo"),
        132 => Some("front"),
        133 => Some("copy"),
        134 => Some("open"),
        135 => Some("paste"),
        136 => Some("find"),
        137 => Some("cut"),
        138 => Some("help"),
        139 => Some("menu"),
        140 => Some("calc"),
        141 => Some("setup"),
        142 => Some("sleep"),
        143 => Some("wakeup"),
        144 => Some("file"),
        145 => Some("sendfile"),
        146 => Some("deletefile"),
        147 => Some("xfer"),
        148 => Some("scrolldown"), // keyd-internal, not a real evdev code
        149 => Some("scrollup"),   // keyd-internal, not a real evdev code
        150 => Some("www"),
        151 => Some("msdos"),
        152 => Some("coffee"),
        153 => Some("display"),
        154 => Some("cyclewindows"),
        155 => Some("mail"),
        156 => Some("favorites"),
        157 => Some("computer"),
        158 => Some("back"),
        159 => Some("forward"),
        160 => Some("closecd"),
        161 => Some("ejectcd"),
        162 => Some("ejectclosecd"),
        163 => Some("nextsong"),
        164 => Some("playpause"),
        165 => Some("previoussong"),
        166 => Some("stopcd"),
        167 => Some("record"),
        168 => Some("rewind"),
        169 => Some("phone"),
        170 => Some("iso"),
        171 => Some("config"),
        172 => Some("homepage"),
        173 => Some("refresh"),
        174 => Some("exit"),
        175 => Some("move"),
        176 => Some("edit"),
        177 => Some("zoom"),
        178 => Some("mouseback"), // keyd-internal, not a real evdev code
        179 => Some("kpleftparen"),
        180 => Some("kprightparen"),
        181 => Some("new"),
        182 => Some("redo"),
        183 => Some("f13"),
        184 => Some("f14"),
        185 => Some("f15"),
        186 => Some("f16"),
        187 => Some("f17"),
        188 => Some("f18"),
        189 => Some("f19"),
        190 => Some("f20"),
        191 => Some("f21"),
        192 => Some("f22"),
        193 => Some("f23"),
        194 => Some("f24"),
        195 => Some("noop"), // keyd-internal, not a real evdev code
        200 => Some("playcd"),
        201 => Some("pausecd"),
        202 => Some("scrollleft"),  // keyd-internal, not a real evdev code
        203 => Some("scrollright"), // keyd-internal, not a real evdev code
        204 => Some("dashboard"),
        205 => Some("suspend"),
        206 => Some("close"),
        207 => Some("play"),
        208 => Some("fastforward"),
        209 => Some("bassboost"),
        210 => Some("print"),
        211 => Some("hp"),
        212 => Some("camera"),
        213 => Some("sound"),
        214 => Some("question"),
        215 => Some("email"),
        216 => Some("chat"),
        217 => Some("search"),
        218 => Some("connect"),
        219 => Some("finance"),
        220 => Some("sport"),
        221 => Some("shop"),
        222 => Some("voicecommand"),
        223 => Some("cancel"),
        224 => Some("brightnessdown"),
        225 => Some("brightnessup"),
        226 => Some("media"),
        227 => Some("switchvideomode"),
        228 => Some("kbdillumtoggle"),
        229 => Some("kbdillumdown"),
        230 => Some("kbdillumup"),
        231 => Some("send"),
        232 => Some("reply"),
        233 => Some("forwardmail"),
        234 => Some("save"),
        235 => Some("documents"),
        236 => Some("battery"),
        237 => Some("bluetooth"),
        238 => Some("wlan"),
        239 => Some("uwb"),
        240 => Some("unknown"),
        241 => Some("next"),
        242 => Some("prev"),
        243 => Some("cycle"),
        244 => Some("auto"),
        245 => Some("off"),
        246 => Some("wwan"),
        247 => Some("rfkill"),
        248 => Some("micmute"),
        249 => Some("leftmouse"),    // keyd-internal, not a real evdev code
        250 => Some("middlemouse"),  // keyd-internal, not a real evdev code
        251 => Some("rightmouse"),   // keyd-internal, not a real evdev code
        252 => Some("mouse1"),       // keyd-internal, not a real evdev code
        253 => Some("mouse2"),       // keyd-internal, not a real evdev code
        254 => Some("fn"),           // keyd-internal, not a real evdev code
        255 => Some("mouseforward"), // keyd-internal, not a real evdev code
        _ => None,
    }
}

/// Every valid keyd key *name*, sorted (the 315 unique names `keyd list-keys`
/// prints, captured from keyd v2.6.0). Unlike [`keycode_name`] — which maps an
/// evdev code to its single *primary* name — this is the full name set keyd
/// accepts as a key token, including alt/shifted aliases (`escape`, `Q`, `!`,
/// `+`, `-`, `,`, `iso-level3-shift`) that `keycode_name` never returns. The macro
/// tokenizer needs exactly this set: keyd types a macro token as a key iff the
/// token is one of these names, otherwise it types the token literally — so
/// [`is_keycode`] must mirror keyd's decision or a valid key would serialize as
/// text. Sorted for binary search (ASCII-only, so byte order == `keyd list-keys`).
static KEY_NAMES: &[&str] = &[
    "!", "\"", "#", "$", "%", "&",
    "'", "(", ")", "*", "+", ",",
    "-", ".", "/", "0", "1", "102nd",
    "2", "3", "4", "5", "6", "7",
    "8", "9", ":", ";", "<", "=",
    ">", "?", "@", "A", "B", "C",
    "D", "E", "F", "G", "H", "I",
    "J", "K", "L", "M", "N", "O",
    "P", "Q", "R", "S", "T", "U",
    "V", "W", "X", "Y", "Z", "[",
    "\\", "]", "^", "_", "`", "a",
    "again", "apostrophe", "auto", "b", "back", "backslash",
    "backspace", "bassboost", "battery", "bluetooth", "bookmarks", "brightnessdown",
    "brightnessup", "c", "calc", "camera", "cancel", "capslock",
    "chat", "close", "closecd", "coffee", "comma", "compose",
    "computer", "config", "connect", "copy", "cut", "cycle",
    "cyclewindows", "d", "dashboard", "delete", "deletefile", "display",
    "documents", "dot", "down", "e", "edit", "ejectcd",
    "ejectclosecd", "email", "end", "enter", "equal", "esc",
    "escape", "exit", "f", "f1", "f10", "f11",
    "f12", "f13", "f14", "f15", "f16", "f17",
    "f18", "f19", "f2", "f20", "f21", "f22",
    "f23", "f24", "f3", "f4", "f5", "f6",
    "f7", "f8", "f9", "fastforward", "favorites", "file",
    "finance", "find", "fn", "forward", "forwardmail", "front",
    "g", "grave", "h", "hangeul", "hanja", "help",
    "henkan", "hiragana", "home", "homepage", "hp", "i",
    "insert", "iso", "iso-level3-shift", "j", "k", "katakana",
    "katakanahiragana", "kbdillumdown", "kbdillumtoggle", "kbdillumup", "kp0", "kp1",
    "kp2", "kp3", "kp4", "kp5", "kp6", "kp7",
    "kp8", "kp9", "kpasterisk", "kpcomma", "kpdot", "kpenter",
    "kpequal", "kpjpcomma", "kpleftparen", "kpminus", "kpplus", "kpplusminus",
    "kprightparen", "kpslash", "l", "left", "leftalt", "leftbrace",
    "leftcontrol", "leftmeta", "leftmouse", "leftshift", "linefeed", "m",
    "macro", "mail", "media", "menu", "micmute", "middlemouse",
    "minus", "mouse1", "mouse2", "mouseback", "mouseforward", "move",
    "msdos", "muhenkan", "mute", "n", "new", "next",
    "nextsong", "noop", "numlock", "o", "off", "open",
    "p", "pagedown", "pageup", "paste", "pause", "pausecd",
    "phone", "play", "playcd", "playpause", "power", "prev",
    "previoussong", "print", "prog1", "prog2", "prog3", "prog4",
    "props", "q", "question", "r", "record", "redo",
    "refresh", "reply", "rewind", "rfkill", "right", "rightalt",
    "rightbrace", "rightcontrol", "rightmeta", "rightmouse", "rightshift", "ro",
    "s", "save", "scale", "scrolldown", "scrollleft", "scrolllock",
    "scrollright", "scrollup", "search", "semicolon", "send", "sendfile",
    "setup", "shop", "slash", "sleep", "sound", "space",
    "sport", "stop", "stopcd", "suspend", "switchvideomode", "sysrq",
    "t", "tab", "u", "undo", "unknown", "up",
    "uwb", "v", "voicecommand", "volumedown", "volumeup", "w",
    "wakeup", "wlan", "wwan", "www", "x", "xfer",
    "y", "yen", "z", "zenkakuhankaku", "zoom", "{",
    "|", "}", "~",
];

/// True when `name` is a key keyd recognizes (a member of [`KEY_NAMES`]). This is
/// the macro tokenizer's Key-vs-Text decision: a token keyd would press as a key
/// vs. one it types literally. Mirrors keyd's own lookup exactly.
pub fn is_keycode(name: &str) -> bool {
    KEY_NAMES.binary_search(&name).is_ok()
}

#[cfg(test)]
mod tests {
    use super::{is_keycode, keycode_name, KEY_NAMES};

    #[test]
    fn key_names_is_sorted_for_binary_search() {
        let mut sorted = KEY_NAMES.to_vec();
        sorted.sort_unstable();
        assert_eq!(sorted, KEY_NAMES, "KEY_NAMES must be sorted");
    }

    #[test]
    fn is_keycode_matches_keyd_decision() {
        // Alt/shifted aliases keyd accepts but keycode_name never returns.
        for k in ["escape", "Q", "!", "+", "-", ",", "iso-level3-shift", "space", "enter", "\\"] {
            assert!(is_keycode(k), "{k} should be a keycode");
        }
        // Modifier aliases are NOT keys (they're shorthand, typed literally in a macro).
        for k in ["control", "shift", "alt", "meta", "altgr"] {
            assert!(!is_keycode(k), "{k} is a modifier alias, not a key");
        }
        // Plain text / unknown tokens.
        for k in ["Hello", "google.com", "C-a", "notakey123", ""] {
            assert!(!is_keycode(k), "{k:?} should not be a keycode");
        }
    }

    #[test]
    fn known_keys_map_to_keyd_primary_names() {
        assert_eq!(keycode_name(30), Some("a")); // KEY_A
        assert_eq!(keycode_name(33), Some("f")); // KEY_F
        assert_eq!(keycode_name(42), Some("leftshift"));
        assert_eq!(keycode_name(13), Some("=")); // primary, not shifted "+"
        assert_eq!(keycode_name(57), Some("space"));
        assert_eq!(keycode_name(105), Some("left"));
    }

    #[test]
    fn unpopulated_and_high_codes_are_none() {
        assert_eq!(keycode_name(0), None);
        assert_eq!(keycode_name(272), None); // BTN_LEFT — keyd remaps; not named here
        assert_eq!(keycode_name(600), None);
    }
}
