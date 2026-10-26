//! Text from Linear is written by other people: strip what a terminal would
//! act on before any of it reaches the screen.

use serde_json::Value;

/// Remove control characters (except newline and tab), DEL, C1 controls and
/// bidirectional overrides and isolates. Escape sequences lose their ESC,
/// so they print as harmless text.
pub fn clean(s: &str) -> String {
    s.chars().filter(|&c| !unsafe_char(c)).collect()
}

fn unsafe_char(c: char) -> bool {
    matches!(c,
        '\u{0}'..='\u{8}' | '\u{b}'..='\u{1f}' | '\u{7f}'..='\u{9f}'
        | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
}

/// Clean every string in a JSON document, keys included.
pub fn clean_json(v: &mut Value) {
    match v {
        Value::String(s) if s.chars().any(unsafe_char) => *s = clean(s),
        Value::Array(a) => a.iter_mut().for_each(clean_json),
        Value::Object(m) => {
            let dirty: Vec<String> = m.keys().filter(|k| k.chars().any(unsafe_char)).cloned().collect();
            for k in dirty {
                if let Some(x) = m.remove(&k) {
                    m.insert(clean(&k), x);
                }
            }
            m.values_mut().for_each(clean_json);
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn keeps_text_newlines_tabs_and_unicode() {
        let s = "Coupon\tcode\nCafé 咖啡 ✓ 🎉";
        assert_eq!(clean(s), s);
    }

    #[test]
    fn strips_escape_sequences_and_controls() {
        assert_eq!(clean("\x1b]52;c;aGk=\x07title"), "]52;c;aGk=title");
        assert_eq!(clean("a\x1b[2Jb\rc\x00d\x7f"), "a[2Jbcd");
        assert_eq!(clean("x\u{9b}31my"), "x31my", "C1 CSI");
    }

    #[test]
    fn strips_bidi_overrides() {
        assert_eq!(clean("in\u{202e}voice.exe\u{2066}x\u{2069}"), "invoice.exex");
    }

    #[test]
    fn cleans_a_whole_document() {
        let mut v =
            json!({"title": "a\x1bb", "items": [{"body": "\u{202e}x"}], "n": 1, "k\x07": "v", "ok": "fine"});
        clean_json(&mut v);
        assert_eq!(v, json!({"title": "ab", "items": [{"body": "x"}], "n": 1, "k": "v", "ok": "fine"}));
    }
}
