//! Public formatter regressions for the local IDNA dependency repair.
use mdurl::{format_url_for_computers, format_url_for_humans};

#[test]
fn quality_idna_valid_unicode_computer_and_human_formats() {
    assert_eq!(
        format_url_for_computers("HTTPS://Bücher.example/你好?q=a b"),
        "https://xn--bcher-kva.example/%E4%BD%A0%E5%A5%BD?q=a%20b"
    );
    assert_eq!(
        format_url_for_humans("https://xn--bcher-kva.example/%E4%BD%A0%E5%A5%BD", 200),
        "bücher.example/你好"
    );
}

#[test]
fn quality_idna_masked_ascii_and_empty_ace_do_not_collapse() {
    for (masked, trusted) in [
        ("xn--example-.org", "example.org"),
        ("example.org.xn--", "example.org."),
    ] {
        let input = format!("https://{masked}/");
        let encoded = format_url_for_computers(&input);
        // The computer formatter preserves the original malformed label on
        // conversion failure rather than silently selecting the trusted host.
        assert_eq!(encoded, input);
        assert_ne!(
            encoded,
            format_url_for_computers(&format!("https://{trusted}/"))
        );
        let displayed = format_url_for_humans(&input, 200);
        assert_ne!(
            displayed,
            format_url_for_humans(&format!("https://{trusted}/"), 200)
        );
    }
}

#[test]
fn quality_idna_formatters_preserve_percent_escapes_and_non_host_schemes() {
    assert_eq!(
        format_url_for_computers("https://Bücher.example/a%2fb%FF?x=%25"),
        "https://xn--bcher-kva.example/a%2fb%FF?x=%25"
    );
    assert_eq!(
        format_url_for_humans("https://example.org/a%252Fb?x=%25", 200),
        "example.org/a%252Fb?x=%25"
    );
    assert_eq!(
        format_url_for_computers("matrix:u/alice:example.org"),
        "matrix:u/alice:example.org"
    );
    assert_eq!(format_url_for_computers("skype:名字"), "skype:名字");
}

#[test]
fn quality_idna_mailto_and_protocol_relative_valid_hosts() {
    assert_eq!(
        format_url_for_computers("mailto:person@Bücher.example"),
        "mailto:person@xn--bcher-kva.example"
    );
    assert_eq!(
        format_url_for_computers("//Bücher.example/a b"),
        "//xn--bcher-kva.example/a%20b"
    );
    assert_eq!(
        format_url_for_humans("mailto:person@xn--bcher-kva.example", 200),
        "person@bücher.example"
    );
}
