//! The man page fixes `build.rs` applies around `mandown` (`build/manpage.rs`), on
//! roff shaped like `mandown` 1.1 output

#[path = "../build/manpage.rs"]
mod manpage;

use pretty_assertions::assert_eq;

#[test]
fn unicode_survives_as_groff_escapes_with_ascii_fallbacks() {
    let protected = manpage::protect_unicode("Sink: X \u{b7} Mic: Y, \u{2191}/\u{2193}");
    assert!(protected.is_ascii(), "{protected}");

    // `mandown` keeps the placeholders (alphanumeric) and escapes `.` and `-`
    let roff = format!(".TH \"pwsw\" 1\n.PP\n{protected}\\.\n");
    let out = manpage::postprocess(&roff, "pwsw", 1, "0.7.0");
    assert_eq!(
        out,
        ".TH \"pwsw\" 1 \"\" \"pwsw 0.7.0\"\n\
         .if \\n(.g .fchar \\[u2191] Up\n\
         .if \\n(.g .fchar \\[u2193] Down\n\
         .if \\n(.g .fchar \\[u00B7] \\-\n\
         .PP\n\
         Sink: X \\[u00B7] Mic: Y, \\[u2191]/\\[u2193]\\.\n"
    );
}

#[test]
fn characters_without_fallback_are_reported_once() {
    assert_eq!(
        manpage::unicode_without_fallback("\u{b7} \u{e9} \u{2713} \u{e9} \u{1f600}"),
        vec!['\u{e9}', '\u{1f600}']
    );
    assert_eq!(
        manpage::unicode_without_fallback("plain ASCII"),
        Vec::<char>::new()
    );
}

#[test]
fn text_that_only_looks_like_a_placeholder_is_kept() {
    let roff = ".PP\nPWSWU and PWSWU12 and PWSWUXYZ\n";
    assert_eq!(manpage::postprocess(roff, "pwsw", 1, "0.7.0"), roff);
}

#[test]
fn inline_code_loses_its_backticks() {
    let roff = ".PP\nPress `\\f[CR]?\\fP` or `\\f[CR]F1\\fP`\\.\n";
    assert_eq!(
        manpage::postprocess(roff, "pwsw", 1, "0.7.0"),
        ".PP\nPress \\f[CR]?\\fP or \\f[CR]F1\\fP\\.\n"
    );
}

#[test]
fn definition_terms_start_a_paragraph_and_end_in_roman() {
    // A paragraph, then a definition list: `mandown` glues the first term to the
    // paragraph, wraps it in an extra bold and opens the definition with `.PP`
    let roff = "\
.SH \"COMMANDS\"
.PP
Intro\\.
\\fB\\fBdaemon\\fP [\\fIOPTIONS\\fP]\\fP
.RS
.PP
Start it\\.
.PP
More\\.
.RE
\\fB\\fBtui\\fP\\fP
.RS
.PP
The TUI\\.
.RE
";
    assert_eq!(
        manpage::postprocess(roff, "pwsw", 1, "0.7.0"),
        "\
.SH \"COMMANDS\"
.PP
Intro\\.
.PP
\\fB\\fBdaemon\\fP [\\fIOPTIONS\\fP]\\fR
.RS
Start it\\.
.PP
More\\.
.RE
.PP
\\fB\\fBtui\\fP\\fR
.RS
The TUI\\.
.RE
"
    );
}

#[test]
fn bold_paragraph_before_a_list_is_not_a_term() {
    let roff = ".PP\n\\fBSupported:\\fP\n.RS\n.IP \\(bu 4\nA\n.RE\n";
    assert_eq!(manpage::postprocess(roff, "pwsw", 1, "0.7.0"), roff);
}

#[test]
fn only_this_pages_title_line_is_rewritten() {
    let out = manpage::postprocess(".TH \"pwsw\" 5\n.PP\nx\n", "pwsw", 5, "1.2.3");
    assert_eq!(out, ".TH \"pwsw\" 5 \"\" \"pwsw 1.2.3\"\n.PP\nx\n");
}
