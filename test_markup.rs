use gtk4::prelude::*;
fn test(buffer: &gtk4::TextBuffer) {
    let mut iter = buffer.end_iter();
    buffer.insert_markup(&mut iter, "<span foreground=\"red\">Hello</span>");
}
