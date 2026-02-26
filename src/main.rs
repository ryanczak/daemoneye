mod terminal;

use gtk4 as gtk;
use gtk::prelude::*;
use gtk::{Application, ApplicationWindow, TextView, Box, Orientation, ScrolledWindow, CssProvider};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use crate::terminal::{TerminalState, spawn_pty};
use portable_pty::PtySize;
use std::io::Write;

fn main() {
    let app = Application::builder()
        .application_id("com.github.t1000")
        .build();

    app.connect_activate(build_ui);
    app.run();
}

fn build_ui(app: &Application) {
    let window = ApplicationWindow::builder()
        .application(app)
        .title("T1000 - Basic Terminal")
        .default_width(800)
        .default_height(600)
        .build();

    let vbox = Box::new(Orientation::Vertical, 0);
    let view = TextView::builder()
        .editable(true) // Needs to be editable to easily accept focus, though we intercept keys
        .accepts_tab(false)
        .cursor_visible(false)
        .monospace(true)
        .top_margin(0)
        .bottom_margin(0)
        .left_margin(0)
        .right_margin(0)
        .build();

    let scrolled_window = ScrolledWindow::builder()
        .hexpand(true)
        .vexpand(true)
        .build();
    // Force horizontal scroll off AFTER construction — builder property may not stick
    scrolled_window.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    view.set_wrap_mode(gtk::WrapMode::None);
    scrolled_window.set_child(Some(&view));

    vbox.append(&scrolled_window);
    window.set_child(Some(&vbox));
    window.present();

    let state = Arc::new(Mutex::new(TerminalState::new(140, 40)));
    
    // Spawn PTY in background
    let state_clone = state.clone();
    let pty_handle = spawn_pty(state_clone, 140, 40).unwrap();
    let pty_writer = Arc::new(Mutex::new(pty_handle.master.take_writer().unwrap()));
    let master_pty = Arc::new(Mutex::new(pty_handle.master));
    let window_weak = window.downgrade();

    // Window dimension tracking state for the render loop
    let last_width = Arc::new(Mutex::new(0i32));
    let last_height = Arc::new(Mutex::new(0i32));
    let last_width_key = last_width.clone();

    // Setup dynamic font resizing CSS provider
    let font_size = Arc::new(Mutex::new(14));
    let css_provider = CssProvider::new();
    css_provider.load_from_data("
        textview { 
            font-family: monospace;
            padding: 0;
            border: 0;
            margin: 0;
            outline: none;
            box-shadow: none;
        }
        textview text {
            padding: 0;
            margin: 0;
        }
        scrolledwindow {
            padding: 0;
            border: 0;
        }
    ");
    #[allow(deprecated)]
    view.style_context().add_provider(&css_provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);

    // Setup input handling
    let key_controller = gtk::EventControllerKey::new();
    let pty_writer_clone = pty_writer.clone();
    let font_size_clone = font_size.clone();
    let css_provider_clone = css_provider.clone();
    let state_key = state.clone();
    key_controller.connect_key_pressed(move |_, keyval, _keycode, _state| {
        let mut writer = pty_writer_clone.lock().unwrap();
        let state_mask = _state;

        // 1. Intercept Global Shortcuts First
        if state_mask.contains(gtk::gdk::ModifierType::CONTROL_MASK) 
            && state_mask.contains(gtk::gdk::ModifierType::SHIFT_MASK) {
            
            if keyval == gtk::gdk::Key::E || keyval == gtk::gdk::Key::e {
                let _ = std::process::Command::new("tmux")
                    .args(["split-window", "-h", "-t", "t1000_main"])
                    .status();
                return gtk::glib::Propagation::Stop;
            } else if keyval == gtk::gdk::Key::O || keyval == gtk::gdk::Key::o {
                let _ = std::process::Command::new("tmux")
                    .args(["split-window", "-v", "-t", "t1000_main"])
                    .status();
                return gtk::glib::Propagation::Stop;
            }
        }

        // 1.5. Intercept Pane Switching (Alt + Arrows)
        if state_mask.contains(gtk::gdk::ModifierType::ALT_MASK) {
            match keyval {
                gtk::gdk::Key::Up => {
                    let _ = std::process::Command::new("tmux").args(["select-pane", "-U", "-t", "t1000_main"]).status();
                    return gtk::glib::Propagation::Stop;
                }
                gtk::gdk::Key::Down => {
                    let _ = std::process::Command::new("tmux").args(["select-pane", "-D", "-t", "t1000_main"]).status();
                    return gtk::glib::Propagation::Stop;
                }
                gtk::gdk::Key::Left => {
                    let _ = std::process::Command::new("tmux").args(["select-pane", "-L", "-t", "t1000_main"]).status();
                    return gtk::glib::Propagation::Stop;
                }
                gtk::gdk::Key::Right => {
                    let _ = std::process::Command::new("tmux").args(["select-pane", "-R", "-t", "t1000_main"]).status();
                    return gtk::glib::Propagation::Stop;
                }
                _ => {}
            }
        }

        // 2. Handle Ctrl+<Letter> and Scaling
        if state_mask.contains(gtk::gdk::ModifierType::CONTROL_MASK) {
            if keyval == gtk::gdk::Key::plus || keyval == gtk::gdk::Key::equal || keyval == gtk::gdk::Key::KP_Add {
                let mut fs = font_size_clone.lock().unwrap();
                *fs = (*fs + 1).min(72);
                css_provider_clone.load_from_data(&format!("
                    textview {{ font-size: {}px; padding: 0; border: 0; margin: 0; font-family: monospace; }}
                    textview text {{ padding: 0; margin: 0; }}
                ", *fs));
                *last_width_key.lock().unwrap() = 0; // Force geometry reflow
                return gtk::glib::Propagation::Stop;
            } else if keyval == gtk::gdk::Key::minus || keyval == gtk::gdk::Key::KP_Subtract {
                let mut fs = font_size_clone.lock().unwrap();
                *fs = (*fs - 1).max(6);
                css_provider_clone.load_from_data(&format!("
                    textview {{ font-size: {}px; padding: 0; border: 0; margin: 0; font-family: monospace; }}
                    textview text {{ padding: 0; margin: 0; }}
                ", *fs));
                *last_width_key.lock().unwrap() = 0; // Force geometry reflow
                return gtk::glib::Propagation::Stop;
            }

            if let Some(name) = keyval.name() {
                if name.len() == 1 {
                    let c = name.chars().next().unwrap().to_ascii_lowercase();
                    if c >= 'a' && c <= 'z' {
                        let ctrl_code = (c as u8) - b'a' + 1;
                        let _ = writer.write_all(&[ctrl_code]);
                        return gtk::glib::Propagation::Stop;
                    }
                }
            }
        }

        // 3. Handle Terminal Input
        let key_char = keyval.to_unicode();
        if let Some(c) = key_char {
            let mut buf = [0; 4];
            let s = c.encode_utf8(&mut buf);
            let _ = writer.write_all(s.as_bytes());
        } else {
            // Handle special keys like Enter, Backspace, Arrows
            match keyval {
                gtk::gdk::Key::Return | gtk::gdk::Key::KP_Enter => {
                    let _ = writer.write_all(b"\r");
                }
                gtk::gdk::Key::BackSpace => {
                    let _ = writer.write_all(b"\x08");
                }
                gtk::gdk::Key::Tab => {
                     let _ = writer.write_all(b"\t");
                }
                gtk::gdk::Key::Escape => {
                     let _ = writer.write_all(b"\x1b");
                }
                gtk::gdk::Key::Up => { let _ = writer.write_all(b"\x1b[A"); }
                gtk::gdk::Key::Down => { let _ = writer.write_all(b"\x1b[B"); }
                gtk::gdk::Key::Right => { let _ = writer.write_all(b"\x1b[C"); }
                gtk::gdk::Key::Left => { let _ = writer.write_all(b"\x1b[D"); }
                _ => {}
            }
            // Key press scrolls to bottom
            state_key.lock().unwrap().scroll_offset = 0;
        }
        gtk::glib::Propagation::Stop
    });
    view.add_controller(key_controller);

    // --- Mouse event forwarding ---
    // Helper: measure char cell size for coordinate conversion
    let char_dims = {
        let ctx = view.pango_context();
        let lay = gtk::pango::Layout::new(&ctx);
        lay.set_text(&"W".repeat(80));
        let (pw, ph) = lay.size();
        let scale = gtk::pango::SCALE as f64;
        Arc::new(Mutex::new((pw as f64 / scale / 80.0, ph as f64 / scale)))
    };

    // Mouse button press/release (GestureClick)
    let gesture = gtk::GestureClick::new();
    gesture.set_button(0); // capture all buttons
    let pty_writer_mouse = pty_writer.clone();
    let state_mouse = state.clone();
    let char_dims_click = char_dims.clone();
    gesture.connect_pressed(move |_gesture, _n_press, x, y| {
        let mode = state_mouse.lock().unwrap().mouse_mode;
        if mode == 0 { return; }
        let (cw, ch) = *char_dims_click.lock().unwrap();
        if cw <= 0.0 || ch <= 0.0 { return; }
        let col = (x / cw).floor() as u32 + 1;
        let row = (y / ch).floor() as u32 + 1;
        let button = _gesture.current_button();
        let btn_code = match button { 1 => 0, 2 => 1, 3 => 2, _ => return };
        // SGR encoding: CSI < button ; col ; row M
        let seq = format!("\x1b[<{};{};{}M", btn_code, col, row);
        if let Ok(mut w) = pty_writer_mouse.lock() {
            let _ = w.write_all(seq.as_bytes());
        }
    });
    let pty_writer_mouse2 = pty_writer.clone();
    let state_mouse2 = state.clone();
    let char_dims_release = char_dims.clone();
    gesture.connect_released(move |_gesture, _n_press, x, y| {
        let mode = state_mouse2.lock().unwrap().mouse_mode;
        if mode == 0 { return; }
        let (cw, ch) = *char_dims_release.lock().unwrap();
        if cw <= 0.0 || ch <= 0.0 { return; }
        let col = (x / cw).floor() as u32 + 1;
        let row = (y / ch).floor() as u32 + 1;
        let button = _gesture.current_button();
        let btn_code = match button { 1 => 0, 2 => 1, 3 => 2, _ => return };
        // SGR encoding: CSI < button ; col ; row m (lowercase m = release)
        let seq = format!("\x1b[<{};{};{}m", btn_code, col, row);
        if let Ok(mut w) = pty_writer_mouse2.lock() {
            let _ = w.write_all(seq.as_bytes());
        }
    });
    view.add_controller(gesture);

    // Mouse motion (button-event tracking, mode 1002)
    let motion_ctrl = gtk::EventControllerMotion::new();
    let pty_writer_motion = pty_writer.clone();
    let state_motion = state.clone();
    let char_dims_motion = char_dims.clone();
    motion_ctrl.connect_motion(move |_ctrl, x, y| {
        let mode = state_motion.lock().unwrap().mouse_mode;
        if mode < 1002 { return; } // only in button-event or any-event mode
        let (cw, ch) = *char_dims_motion.lock().unwrap();
        if cw <= 0.0 || ch <= 0.0 { return; }
        let col = (x / cw).floor() as u32 + 1;
        let row = (y / ch).floor() as u32 + 1;
        // SGR drag: button 32 (generic drag)
        let seq = format!("\x1b[<32;{};{}M", col, row);
        if let Ok(mut w) = pty_writer_motion.lock() {
            let _ = w.write_all(seq.as_bytes());
        }
    });
    view.add_controller(motion_ctrl);

    // Scroll wheel
    let scroll_ctrl = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
    let pty_writer_scroll = pty_writer.clone();
    let state_scroll = state.clone();
    let char_dims_scroll = char_dims.clone();
    scroll_ctrl.connect_scroll(move |_ctrl, _dx, dy| {
        let mut sl = state_scroll.lock().unwrap();
        let mode = sl.mouse_mode;
        if mode == 0 {
            // No mouse mode enabled -> use scroll wheel for internal history
            // dy > 0 is scroll down (towards bottom), dy < 0 is scroll up (into history)
            sl.scroll_by(-(dy.round() as i32));
            return gtk::glib::Propagation::Stop;
        }
        drop(sl);

        let (cw, ch) = *char_dims_scroll.lock().unwrap();
        if cw <= 0.0 || ch <= 0.0 { return gtk::glib::Propagation::Proceed; }
        // Use center of widget as approximate position
        let btn_code = if dy < 0.0 { 64 } else { 65 }; // 64=scroll up, 65=scroll down
        // We don't have precise coords in scroll, use col=1, row=1 as fallback
        let seq = format!("\x1b[<{};1;1M", btn_code);
        if let Ok(mut w) = pty_writer_scroll.lock() {
            let _ = w.write_all(seq.as_bytes());
        }
        gtk::glib::Propagation::Stop
    });
    view.add_controller(scroll_ctrl);

    // GTK UI Update loop
    let buffer = view.buffer();
    let scrolled_weak = scrolled_window.downgrade();
    let view_weak = view.downgrade();
    let pty_writer_resp = pty_writer.clone();
    gtk::glib::timeout_add_local(Duration::from_millis(50), move || {
        // 1. Check for resize
        if let Some(sw) = scrolled_weak.upgrade() {
            let width = sw.width();
            let height = sw.height();
            
            let mut lw = last_width.lock().unwrap();
            let mut lh = last_height.lock().unwrap();
            
            if width > 0 && height > 0 && (width != *lw || height != *lh) {
                let hadj = sw.hadjustment();
                let vadj = sw.vadjustment();
                let viewport_w = hadj.page_size();
                let viewport_h = vadj.page_size();

                if viewport_w < 20.0 || viewport_h < 20.0 {
                    *lw = 0;
                    *lh = 0;
                } else {
                    let (char_w_f, char_h_f) = {
                        let ctx = view.pango_context();
                        let lay = gtk::pango::Layout::new(&ctx);
                        const PROBE_LEN: usize = 200;
                        lay.set_text(&"W".repeat(PROBE_LEN));
                        let (pw, ph) = lay.size();
                        let scale = gtk::pango::SCALE as f64;
                        (pw as f64 / scale / PROBE_LEN as f64, ph as f64 / scale)
                    };

                    if char_w_f > 0.0 && char_h_f > 0.0 {
                        let cols = (viewport_w / char_w_f).floor() as u16;
                        let rows = (viewport_h / char_h_f).floor() as u16;
                        let cols = cols.max(10);
                        let rows = rows.max(5);



                        *lw = width;
                        *lh = height;

                        let size = PtySize { rows, cols,
                            pixel_width: viewport_w as u16, pixel_height: viewport_h as u16 };
                        if let Ok(master) = master_pty.lock() {
                            let _ = master.resize(size);
                        }
                        let mut state_lock = state.lock().unwrap();
                        state_lock.resize(cols as usize, rows as usize);
                    } else {
                        *lw = 0;
                        *lh = 0;
                    }
                }
            }
        }

        // 2. Render terminal
        {
            let mut state_lock = state.lock().unwrap();
            let cy = state_lock.cursor_y;
            if let Some(rendered_text) = state_lock.render_markup() {
                buffer.set_text("");
                let mut iter = buffer.start_iter();
                buffer.insert_markup(&mut iter, &rendered_text);

                // Scroll to active cursor
                if let Some(view) = view_weak.upgrade() {
                    let cy_safe = cy.min(state_lock.rows.saturating_sub(1));
                    if let Some(iter) = buffer.iter_at_line(cy_safe as i32) {
                        let mark = buffer.create_mark(None, &iter, false);
                        view.scroll_to_mark(&mark, 0.0, true, 0.0, 1.0);
                        buffer.delete_mark(&mark);
                    }
                }
            }
            // state_lock dropped here — MUST release before overflow check below
        }

        // Post-render overflow feedback: if content is wider than the viewport,
        // reduce state.cols immediately and resize. This converges in 1-2 frames.
        if let Some(sw) = scrolled_weak.upgrade() {
            let hadj = sw.hadjustment();
            let content_w = hadj.upper();
            let page_w = hadj.page_size();
            if page_w > 0.0 && content_w > page_w + 1.0 {
                let overflow_px = content_w - page_w;
                let (current_cols, new_rows) = {
                    let sl = state.lock().unwrap();
                    (sl.cols, sl.rows)
                };
                let char_w_est = content_w / current_cols as f64;
                let overflow_cols = (overflow_px / char_w_est).ceil() as usize + 1;
                let new_cols = current_cols.saturating_sub(overflow_cols).max(10);

                // Force recompute next tick
                *last_width.lock().unwrap() = 0;
                *last_height.lock().unwrap() = 0;
                let size = PtySize { rows: new_rows as u16, cols: new_cols as u16,
                    pixel_width: page_w as u16, pixel_height: 0 };
                if let Ok(master) = master_pty.lock() {
                    let _ = master.resize(size);
                }
                state.lock().unwrap().resize(new_cols, new_rows);
            }
        }

        // 3. Drain pending PTY responses (DSR/CPR/DA replies), window title, and clipboard
        {
            let mut state_lock = state.lock().unwrap();
            let responses: Vec<Vec<u8>> = std::mem::take(&mut state_lock.pending_responses);
            let new_title: Option<String> = state_lock.pending_title.take();
            let clipboard_data: Option<String> = state_lock.pending_clipboard.take();
            drop(state_lock);
            if !responses.is_empty() {
                if let Ok(mut writer) = pty_writer_resp.lock() {
                    for resp in responses {
                        let _ = writer.write_all(&resp);
                    }
                }
            }
            if let Some(title) = new_title {
                if let Some(win) = window_weak.upgrade() {
                    win.set_title(Some(&title));
                }
            }
            // OSC 52 clipboard: decode base64 and set system clipboard
            if let Some(b64) = clipboard_data {
                use gtk::glib;
                let decoded = glib::base64_decode(&b64);
                if let Ok(text) = String::from_utf8(decoded) {
                    if let Some(display) = gtk::gdk::Display::default() {
                        display.clipboard().set_text(&text);
                    }
                }
            }
        }
        gtk::glib::ControlFlow::Continue
    });
}
