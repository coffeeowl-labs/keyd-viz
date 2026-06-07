//! Byte-level safety scan of the exact bytes to be written (design doc §5.3).
//!
//! `keyd check` is syntax-only — a file full of `command(rm -rf …)` passes it clean —
//! so the *content* gate is ours. The scan runs on raw bytes (it never trusts the
//! GUI's edit model) and is a deliberate **superset** of what keyd would execute:
//!
//! - keyd runs a command only when a descriptor is literally `command(...)`
//!   (`parse_command`, config.c: `strstr(s, "command(") == s`), and macros via
//!   `macro(`/`macro2(`. Descriptors nest (`overload(nav, command(x))`), and keyd's
//!   arg splitter honors backslash escapes — but *substring presence per line* is
//!   immune to arg-splitting evasion: any line keyd could execute necessarily
//!   contains the literal token. Over-flagging (e.g. `mycommand(`, which keyd would
//!   reject) costs one extra confirmation; under-flagging is the failure mode we
//!   refuse.
//! - `include ` is detected exactly as keyd does (config.c `read_config_file`):
//!   the **raw, untrimmed** line starts with `include ` — include expansion happens
//!   *before* INI comment handling. Includes are advisory (keyd confines them to
//!   root-owned dirs; review #2), but an argument keyd would mangle (absolute or
//!   `..`) is flagged as suspect.
//! - Comment lines (first non-whitespace byte `#`, keyd's trim) can never execute
//!   and are skipped, so a commented-out `# command(...)` example doesn't demand
//!   confirmation. An *include* line is checked before the comment rule on purpose:
//!   include detection happens on the raw line in keyd, so byte-0 `include ` wins.

/// One finding from the scan, with the 1-based line it was found on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Finding {
    /// `command(` — executes shell as root on keypress. Requires explicit ack.
    Command { line: usize },
    /// `macro(`/`macro2(` — keystroke injection. Requires explicit ack.
    Macro { line: usize },
    /// An `include` directive (advisory; content is root-gated by keyd itself).
    Include { line: usize, arg: String },
    /// An include argument keyd would not resolve sanely (absolute or `..`).
    SuspectInclude { line: usize, arg: String },
}

impl Finding {
    /// Findings that require the caller's explicit acknowledgement before apply.
    pub fn needs_ack(&self) -> bool {
        matches!(self, Finding::Command { .. } | Finding::Macro { .. })
    }

    /// Machine-readable one-liner for the stdout protocol.
    pub fn describe(&self) -> String {
        match self {
            Finding::Command { line } => format!("command line {line}"),
            Finding::Macro { line } => format!("macro line {line}"),
            Finding::Include { line, arg } => format!("include line {line} {arg}"),
            Finding::SuspectInclude { line, arg } => {
                format!("suspect-include line {line} {arg}")
            }
        }
    }
}

/// Scan config bytes. Operates on `\n`-split raw lines (keyd's only line atom);
/// non-UTF-8 input scans fine — the tokens we look for are pure ASCII.
pub fn scan(bytes: &[u8]) -> Vec<Finding> {
    let mut findings = Vec::new();
    for (i, raw) in bytes.split(|&b| b == b'\n').enumerate() {
        let line = i + 1;

        // keyd's include check runs on the raw line, before any trim or comment
        // handling (read_config_file): literal `include ` at byte 0.
        if let Some(arg) = raw.strip_prefix(b"include ") {
            let arg = String::from_utf8_lossy(arg).into_owned();
            if arg.starts_with('/') || arg.split('/').any(|seg| seg == "..") {
                findings.push(Finding::SuspectInclude { line, arg });
            } else {
                findings.push(Finding::Include { line, arg });
            }
            continue;
        }

        // Comments can't execute (ini.c skips them post-include-expansion).
        let trimmed = trim_c_space(raw);
        if trimmed.first() == Some(&b'#') {
            continue;
        }

        if contains(raw, b"command(") {
            findings.push(Finding::Command { line });
        }
        if contains(raw, b"macro(") || contains(raw, b"macro2(") {
            findings.push(Finding::Macro { line });
        }
    }
    findings
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

fn trim_c_space(b: &[u8]) -> &[u8] {
    let is_space = |c: &&u8| matches!(**c, b' ' | b'\t' | b'\x0b' | b'\x0c' | b'\r');
    let start = b.iter().take_while(is_space).count();
    &b[start..]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(src: &str) -> Vec<String> {
        scan(src.as_bytes()).iter().map(Finding::describe).collect()
    }

    #[test]
    fn clean_config_has_no_findings() {
        assert!(scan(b"[ids]\n*\n[main]\ncapslock = esc\na = overload(nav, a)\n").is_empty());
    }

    #[test]
    fn command_and_macro_flagged_anywhere_in_a_line() {
        assert_eq!(kinds("[main]\na = command(rm -rf /)\n"), ["command line 2"]);
        // Nested descriptors execute too — substring presence catches them.
        assert_eq!(
            kinds("[main]\na = overload(nav, command(x))\n"),
            ["command line 2"]
        );
        assert_eq!(kinds("[main]\na = macro(C-a hello)\n"), ["macro line 2"]);
        assert_eq!(kinds("[main]\na = macro2(50, 50, ab)\n"), ["macro line 2"]);
    }

    #[test]
    fn comments_do_not_flag() {
        assert!(scan(b"# try a = command(date) for fun\n  # macro(x)\n").is_empty());
    }

    #[test]
    fn include_detection_matches_keyd_exactly() {
        // Raw byte-0 match, like read_config_file; an indented one is NOT an
        // include to keyd, and a missing trailing space isn't either.
        assert_eq!(kinds("include common\n"), ["include line 1 common"]);
        assert!(scan(b"  include common\n").is_empty());
        assert!(scan(b"includecommon\n").is_empty());
    }

    #[test]
    fn suspect_includes_flagged() {
        assert_eq!(
            kinds("include /etc/passwd\n"),
            ["suspect-include line 1 /etc/passwd"]
        );
        assert_eq!(
            kinds("include ../outside\n"),
            ["suspect-include line 1 ../outside"]
        );
    }

    #[test]
    fn needs_ack_split() {
        let f = scan(b"include common\na = command(x)\nb = macro(y)\n");
        let acks: Vec<bool> = f.iter().map(Finding::needs_ack).collect();
        assert_eq!(acks, [false, true, true]);
    }

    #[test]
    fn non_utf8_still_scans() {
        let mut bytes = b"[main]\na = command(".to_vec();
        bytes.push(0xff);
        bytes.extend_from_slice(b")\n");
        assert!(matches!(scan(&bytes)[0], Finding::Command { line: 2 }));
    }
}
