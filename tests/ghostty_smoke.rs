use ghostty_vt::Terminal;

#[test]
fn terminal_roundtrip() {
    let mut term = Terminal::new(20, 5, 1 << 20).expect("terminal");
    term.write(b"hi\r\nthere");
    let text = term.read_text_screen((0, 0), (19, 1), false).expect("text");
    assert!(text.contains("hi"), "{text:?}");
    assert!(text.contains("there"), "{text:?}");
}
