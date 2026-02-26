use gtk4::prelude::*;
fn test(view: &gtk4::TextView) {
    let buffer = view.buffer();
    let tag_table = buffer.tag_table();
    let tag = gtk4::TextTag::builder()
        .name("fg_1")
        .foreground("red")
        .build();
    tag_table.add(&tag);
}
