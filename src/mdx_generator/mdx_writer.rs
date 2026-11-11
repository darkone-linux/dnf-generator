//! Generic helpers for producing MDX source.

/// Encode `<`, `>`, `&` of Markdown text for MDX, where a bare `<->` or
/// `<name>` opens a JSX tag. Code spans and fenced blocks are literal: left as
/// written, an entity there would show as typed.
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_code = false;
    for ch in text.chars() {
        match ch {
            '`' => {
                in_code = !in_code;
                out.push(ch);
            }
            '&' if !in_code => out.push_str("&amp;"),
            '<' if !in_code => out.push_str("&lt;"),
            '>' if !in_code => out.push_str("&gt;"),
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_html_chars() {
        assert_eq!(escape("a<b & c>d"), "a&lt;b &amp; c&gt;d");
    }

    #[test]
    fn code_is_left_as_written() {
        assert_eq!(
            escape("LAN <-> WAN, see `backup-link/<name>/psk`"),
            "LAN &lt;-&gt; WAN, see `backup-link/<name>/psk`"
        );
        assert_eq!(
            escape("```sh\na && b > c\n```\nx > y"),
            "```sh\na && b > c\n```\nx &gt; y"
        );
    }
}
