use gtk4::prelude::*;
fn test(view: &gtk4::TextView) {
    let buffer = view.buffer();
    let iter = buffer.start_iter();
    let rect = view.iter_location(&iter);
    let w = rect.width();
    let h = rect.height();
}
