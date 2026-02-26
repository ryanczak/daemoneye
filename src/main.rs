mod ai;
mod config;
mod layout;
mod pane;
mod terminal;

use crate::ai::client::{make_client, AiEvent};
use crate::ai::filter::mask_sensitive;
use crate::config::{load_named_prompt, Config};
use crate::layout::{Direction, PaneManager};
use crate::pane::{AiPane, TerminalPane};

use gtk4 as gtk;
use gtk::glib;
use gtk::prelude::*;
use gtk::{Application, ApplicationWindow, Box as GtkBox, CssProvider, Orientation};

use portable_pty::PtySize;
use std::cell::RefCell;
use std::io::Write;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

// ---------------------------------------------------------------------------
// main
// ---------------------------------------------------------------------------

fn main() {
    let app = Application::builder()
        .application_id("com.github.t1000")
        .build();

    app.connect_activate(build_ui);
    app.run();
}

// ---------------------------------------------------------------------------
// build_ui
// ---------------------------------------------------------------------------

fn build_ui(app: &Application) {
    // Load config (ignore errors so the app still starts without a config)
    let _ = Config::ensure_dirs();
    let cfg = Config::load().unwrap_or_default();
    let ai_cfg = cfg.ai.clone();
    // Load the system prompt once at startup so it's ready when the AI pane opens.
    let default_system_prompt = load_named_prompt(&ai_cfg.prompt).system;

    let window = ApplicationWindow::builder()
        .application(app)
        .title("T1000")
        .default_width(800)
        .default_height(600)
        .build();

    // Root container — multi-pane content lives here
    let container = GtkBox::new(Orientation::Vertical, 0);
    container.set_hexpand(true);
    container.set_vexpand(true);
    window.set_child(Some(&container));
    window.present();

    // ----- Shared CSS provider (controls font size for all text views) -----
    let css_provider = CssProvider::new();
    apply_font_css(&css_provider, 14);
    #[allow(deprecated)]
    if let Some(display) = gtk::gdk::Display::default() {
        gtk::StyleContext::add_provider_for_display(
            &display,
            &css_provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }

    // ----- Initial terminal pane -----
    let initial_pane = TerminalPane::new(0, 80, 24, TerminalPane::tmux_cmd())
        .expect("failed to create initial terminal pane");

    let pm: Rc<RefCell<PaneManager>> = Rc::new(RefCell::new(PaneManager::new(initial_pane)));
    pm.borrow().rebuild_widget(&container);

    // Focus the initial pane's view
    pm.borrow().active_pane().widget().grab_focus();

    // ----- Tokio runtime (for AI requests) -----
    let tokio_rt = Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("failed to build tokio runtime"),
    );

    // ----- Font size state -----
    let font_size: Rc<RefCell<i32>> = Rc::new(RefCell::new(14));

    // =======================================================================
    // Keyboard handler
    // =======================================================================

    let key_controller = gtk::EventControllerKey::new();
    key_controller.set_propagation_phase(gtk::PropagationPhase::Capture);

    let pm_key = pm.clone();
    let container_key = container.clone();
    let css_key = css_provider.clone();
    let font_size_key = font_size.clone();
    let tokio_rt_key = tokio_rt.clone();
    let _window_weak = window.downgrade();
    let ai_cfg_key = ai_cfg.clone();
    let system_prompt_key = default_system_prompt.clone();

    key_controller.connect_key_pressed(move |_, keyval, _keycode, state_mask| {
        use gtk::gdk::ModifierType;
        let ctrl  = state_mask.contains(ModifierType::CONTROL_MASK);
        let shift = state_mask.contains(ModifierType::SHIFT_MASK);
        let alt   = state_mask.contains(ModifierType::ALT_MASK);

        // ------------------------------------------------------------------
        // 1. Global Ctrl+Shift shortcuts
        // ------------------------------------------------------------------
        if ctrl && shift {
            // Split horizontal
            if keyval == gtk::gdk::Key::E || keyval == gtk::gdk::Key::e {
                let new_id = pm_key.borrow_mut().next_id();
                match TerminalPane::new(new_id, 80, 24, TerminalPane::shell_cmd()) {
                    Ok(new_pane) => {
                        pm_key.borrow_mut().split_h(new_pane);
                        pm_key.borrow().rebuild_widget(&container_key);
                        pm_key.borrow().active_pane().widget().grab_focus();
                    }
                    Err(e) => eprintln!("h-split failed: {e}"),
                }
                return glib::Propagation::Stop;
            }
            // Split vertical
            if keyval == gtk::gdk::Key::O || keyval == gtk::gdk::Key::o {
                let new_id = pm_key.borrow_mut().next_id();
                match TerminalPane::new(new_id, 80, 24, TerminalPane::shell_cmd()) {
                    Ok(new_pane) => {
                        pm_key.borrow_mut().split_v(new_pane);
                        pm_key.borrow().rebuild_widget(&container_key);
                        pm_key.borrow().active_pane().widget().grab_focus();
                    }
                    Err(e) => eprintln!("v-split failed: {e}"),
                }
                return glib::Propagation::Stop;
            }
            // Close active pane
            if keyval == gtk::gdk::Key::W || keyval == gtk::gdk::Key::w {
                let closed = pm_key.borrow_mut().close_active();
                if closed.is_some() {
                    pm_key.borrow().rebuild_widget(&container_key);
                    pm_key.borrow().active_pane().widget().grab_focus();
                }
                return glib::Propagation::Stop;
            }
            // Open AI pane  (Ctrl+Shift+A)
            if keyval == gtk::gdk::Key::A || keyval == gtk::gdk::Key::a {
                let ai_id = pm_key.borrow_mut().next_id();
                let ai_pane = AiPane::new(ai_id);

                // Wire "Run in terminal" button — sends the last AI response
                // line to the active terminal pane's PTY writer.
                {
                    let pm_btn = pm_key.clone();
                    let resp_view = ai_pane.response_view.clone();
                    ai_pane.run_button.connect_clicked(move |_| {
                        let buf = resp_view.buffer();
                        let text = buf.text(&buf.start_iter(), &buf.end_iter(), false);
                        // Extract last non-empty line as the command
                        let cmd = text
                            .lines()
                            .filter(|l| !l.trim().is_empty())
                            .last()
                            .map(|l| l.trim().to_string());
                        if let Some(cmd) = cmd {
                            let pm = pm_btn.borrow();
                            if let Some(tp) = pm.active_terminal() {
                                let mut w = tp.pty_writer.lock().unwrap();
                                let _ = w.write_all(cmd.as_bytes());
                                let _ = w.write_all(b"\r");
                            }
                        }
                    });
                }

                // Wire "send on Enter" for the input entry
                {
                    let pm_ai = pm_key.clone();
                    let entry = ai_pane.input_entry.clone();
                    let resp_view = ai_pane.response_view.clone();
                    let ai_cfg_ai = ai_cfg_key.clone();
                    let rt_ai = tokio_rt_key.clone();
                    let ai_id_entry = ai_id;
                    let system_prompt_ai = system_prompt_key.clone();
                    entry.connect_activate(move |entry| {
                        let user_msg = entry.text().to_string();
                        if user_msg.is_empty() { return; }
                        entry.set_text("");

                        // Capture context from the active terminal pane
                        let context = {
                            let pm = pm_ai.borrow();
                            pm.active_terminal()
                                .map(|tp| {
                                    let state = tp.state.lock().unwrap();
                                    state.capture_context(100)
                                })
                                .unwrap_or_default()
                        };
                        // Build prompt using the loaded system prompt.
                        let system = &system_prompt_ai;
                        let user_prompt = format!(
                            "Terminal context (last 100 lines):\n```\n{}\n```\n\n{}",
                            mask_sensitive(&context),
                            user_msg
                        );

                        // Append user message to response view
                        {
                            let buf = resp_view.buffer();
                            let mut end = buf.end_iter();
                            buf.insert(&mut end, &format!("\n> {}\n", user_msg));
                        }

                        // Send to AI on tokio runtime; stream tokens back via
                        // an std::sync::mpsc channel that the render loop polls.
                        let (std_tx, std_rx) =
                            std::sync::mpsc::channel::<AiEvent>();

                        // Store the receiver in the AiPane so the render loop
                        // can drain it.
                        {
                            let mut pm = pm_ai.borrow_mut();
                            if let Some(ai) = pm.panes.get_mut(&ai_id_entry) {
                                if let Some(ap) = ai.as_ai_mut() {
                                    ap.event_rx = Some(std_rx);
                                }
                            }
                        }

                        let client = make_client(
                            &ai_cfg_ai.provider,
                            ai_cfg_ai.api_key.clone(),
                            ai_cfg_ai.model.clone(),
                        );
                        let system_str = system.clone();
                        rt_ai.spawn(async move {
                            let (tokio_tx, mut tokio_rx) =
                                tokio::sync::mpsc::unbounded_channel::<AiEvent>();
                            let std_tx2 = std_tx.clone();
                            tokio::spawn(async move {
                                while let Some(ev) = tokio_rx.recv().await {
                                    if std_tx2.send(ev).is_err() { break; }
                                }
                            });
                            if let Err(e) = client.chat(&system_str, &user_prompt, tokio_tx).await {
                                let _ = std_tx.send(AiEvent::Error(e.to_string()));
                            }
                        });
                    });
                }

                pm_key.borrow_mut().open_ai_pane(ai_pane);
                pm_key.borrow().rebuild_widget(&container_key);
                // Focus the AI input entry
                if let Some(ai) = pm_key.borrow().active_pane().as_ai() {
                    ai.input_entry.grab_focus();
                }
                return glib::Propagation::Stop;
            }
            // Clipboard paste (Ctrl+Shift+V)
            if keyval == gtk::gdk::Key::V || keyval == gtk::gdk::Key::v {
                if let Some(display) = gtk::gdk::Display::default() {
                    let clipboard = display.clipboard();
                    let pm_paste = pm_key.clone();
                    clipboard.read_text_async(None::<&gtk::gio::Cancellable>, move |result| {
                        if let Ok(Some(text)) = result {
                            let pm = pm_paste.borrow();
                            if let Some(tp) = pm.active_terminal() {
                                let bracketed = tp.state.lock().unwrap().bracketed_paste;
                                let mut w = tp.pty_writer.lock().unwrap();
                                if bracketed { let _ = w.write_all(b"\x1b[200~"); }
                                let _ = w.write_all(text.as_bytes());
                                if bracketed { let _ = w.write_all(b"\x1b[201~"); }
                            }
                        }
                    });
                }
                return glib::Propagation::Stop;
            }
        }

        // ------------------------------------------------------------------
        // 2. Alt+Arrow — focus adjacent pane
        // ------------------------------------------------------------------
        if alt && !ctrl {
            let dir = match keyval {
                gtk::gdk::Key::Up    => Some(Direction::Up),
                gtk::gdk::Key::Down  => Some(Direction::Down),
                gtk::gdk::Key::Left  => Some(Direction::Left),
                gtk::gdk::Key::Right => Some(Direction::Right),
                _ => None,
            };
            if let Some(d) = dir {
                pm_key.borrow_mut().focus_adjacent(d);
                pm_key.borrow().active_pane().widget().grab_focus();
                return glib::Propagation::Stop;
            }
        }

        // ------------------------------------------------------------------
        // 3. Ctrl+Plus/Minus — font scaling
        // ------------------------------------------------------------------
        if ctrl && !shift {
            if keyval == gtk::gdk::Key::plus
                || keyval == gtk::gdk::Key::equal
                || keyval == gtk::gdk::Key::KP_Add
            {
                let mut fs = font_size_key.borrow_mut();
                *fs = (*fs + 1).min(72);
                apply_font_css(&css_key, *fs);
                return glib::Propagation::Stop;
            }
            if keyval == gtk::gdk::Key::minus || keyval == gtk::gdk::Key::KP_Subtract {
                let mut fs = font_size_key.borrow_mut();
                *fs = (*fs - 1).max(6);
                apply_font_css(&css_key, *fs);
                return glib::Propagation::Stop;
            }
            // Ctrl+<letter> → control code
            if let Some(name) = keyval.name() {
                if name.len() == 1 {
                    let c = name.chars().next().unwrap().to_ascii_lowercase();
                    if ('a'..='z').contains(&c) {
                        let code = (c as u8) - b'a' + 1;
                        let pm = pm_key.borrow();
                        if let Some(tp) = pm.active_terminal() {
                            let _ = tp.pty_writer.lock().unwrap().write_all(&[code]);
                        }
                        return glib::Propagation::Stop;
                    }
                }
            }
        }

        // ------------------------------------------------------------------
        // 4. Normal terminal key input — only for terminal panes
        // ------------------------------------------------------------------
        let (pty_writer, state_arc) = {
            let pm = pm_key.borrow();
            match pm.active_terminal() {
                Some(tp) => (tp.pty_writer.clone(), tp.state.clone()),
                None => return glib::Propagation::Proceed,
            }
        };

        let app_cursor = state_arc.lock().unwrap().app_cursor_keys;
        let key_char = keyval.to_unicode();
        if let Some(c) = key_char {
            let mut buf = [0u8; 4];
            let s = c.encode_utf8(&mut buf);
            let _ = pty_writer.lock().unwrap().write_all(s.as_bytes());
            state_arc.lock().unwrap().scroll_offset = 0;
        } else {
            let mut w = pty_writer.lock().unwrap();
            match keyval {
                gtk::gdk::Key::Return | gtk::gdk::Key::KP_Enter => {
                    let _ = w.write_all(b"\r");
                }
                gtk::gdk::Key::BackSpace => { let _ = w.write_all(b"\x7f"); }
                gtk::gdk::Key::Tab       => { let _ = w.write_all(b"\t"); }
                gtk::gdk::Key::Escape    => { let _ = w.write_all(b"\x1b"); }
                gtk::gdk::Key::Up => {
                    if ctrl        { let _ = w.write_all(b"\x1b[1;5A"); }
                    else if app_cursor { let _ = w.write_all(b"\x1bOA"); }
                    else           { let _ = w.write_all(b"\x1b[A"); }
                }
                gtk::gdk::Key::Down => {
                    if ctrl        { let _ = w.write_all(b"\x1b[1;5B"); }
                    else if app_cursor { let _ = w.write_all(b"\x1bOB"); }
                    else           { let _ = w.write_all(b"\x1b[B"); }
                }
                gtk::gdk::Key::Right => {
                    if ctrl        { let _ = w.write_all(b"\x1b[1;5C"); }
                    else if app_cursor { let _ = w.write_all(b"\x1bOC"); }
                    else           { let _ = w.write_all(b"\x1b[C"); }
                }
                gtk::gdk::Key::Left => {
                    if ctrl        { let _ = w.write_all(b"\x1b[1;5D"); }
                    else if app_cursor { let _ = w.write_all(b"\x1bOD"); }
                    else           { let _ = w.write_all(b"\x1b[D"); }
                }
                gtk::gdk::Key::Delete   => { let _ = w.write_all(b"\x1b[3~"); }
                gtk::gdk::Key::Insert   => { let _ = w.write_all(b"\x1b[2~"); }
                gtk::gdk::Key::Home => {
                    if app_cursor { let _ = w.write_all(b"\x1bOH"); }
                    else          { let _ = w.write_all(b"\x1b[H"); }
                }
                gtk::gdk::Key::End => {
                    if app_cursor { let _ = w.write_all(b"\x1bOF"); }
                    else          { let _ = w.write_all(b"\x1b[F"); }
                }
                gtk::gdk::Key::Page_Up   => { let _ = w.write_all(b"\x1b[5~"); }
                gtk::gdk::Key::Page_Down => { let _ = w.write_all(b"\x1b[6~"); }
                gtk::gdk::Key::F1  => { let _ = w.write_all(b"\x1bOP"); }
                gtk::gdk::Key::F2  => { let _ = w.write_all(b"\x1bOQ"); }
                gtk::gdk::Key::F3  => { let _ = w.write_all(b"\x1bOR"); }
                gtk::gdk::Key::F4  => { let _ = w.write_all(b"\x1bOS"); }
                gtk::gdk::Key::F5  => { let _ = w.write_all(b"\x1b[15~"); }
                gtk::gdk::Key::F6  => { let _ = w.write_all(b"\x1b[17~"); }
                gtk::gdk::Key::F7  => { let _ = w.write_all(b"\x1b[18~"); }
                gtk::gdk::Key::F8  => { let _ = w.write_all(b"\x1b[19~"); }
                gtk::gdk::Key::F9  => { let _ = w.write_all(b"\x1b[20~"); }
                gtk::gdk::Key::F10 => { let _ = w.write_all(b"\x1b[21~"); }
                gtk::gdk::Key::F11 => { let _ = w.write_all(b"\x1b[23~"); }
                gtk::gdk::Key::F12 => { let _ = w.write_all(b"\x1b[24~"); }
                _ => {}
            }
            drop(w);
            state_arc.lock().unwrap().scroll_offset = 0;
        }

        glib::Propagation::Stop
    });
    window.add_controller(key_controller);

    // =======================================================================
    // Mouse forwarding
    // =======================================================================
    // Mouse events are handled per-pane via controllers attached to each
    // pane's view widget (see setup_mouse_controllers, called during rebuild).
    // For the initial pane, we set them up now.
    {
        let pm_borrow = pm.borrow();
        if let Some(tp) = pm_borrow.active_terminal() {
            setup_mouse_controllers(tp);
        }
    }

    // =======================================================================
    // Render & resize loop (50 ms)
    // =======================================================================

    let pm_render = pm.clone();
    let window_weak2 = window.downgrade();
    glib::timeout_add_local(Duration::from_millis(50), move || {
        let mut pm = pm_render.borrow_mut();

        for pane in pm.panes.values_mut() {
            // ---- Drain AI pane event queue ----
            if let crate::pane::Pane::Ai(ap) = pane {
                if let Some(rx) = &ap.event_rx {
                    loop {
                        match rx.try_recv() {
                            Ok(AiEvent::Token(t)) => {
                                let buf = ap.response_view.buffer();
                                let mut end = buf.end_iter();
                                buf.insert(&mut end, &t);
                            }
                            Ok(AiEvent::Done) => {
                                let buf = ap.response_view.buffer();
                                let mut end = buf.end_iter();
                                buf.insert(&mut end, "\n");
                                break;
                            }
                            Ok(AiEvent::Error(e)) => {
                                let buf = ap.response_view.buffer();
                                let mut end = buf.end_iter();
                                buf.insert(&mut end, &format!("\n[Error: {e}]\n"));
                                break;
                            }
                            Err(std::sync::mpsc::TryRecvError::Empty) => break,
                            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                                // drop the receiver
                                break;
                            }
                        }
                    }
                }
                continue;
            }

            let tp = match pane {
                crate::pane::Pane::Terminal(tp) => tp,
                crate::pane::Pane::Ai(_) => unreachable!(),
            };

            // ---- Resize ----
            let sw = &tp.scrolled;
            let width  = sw.width();
            let height = sw.height();

            if width > 0
                && height > 0
                && (width != tp.last_known_width || height != tp.last_known_height)
            {
                let (char_w, char_h) = measure_char_cell(&tp.view);
                let hadj = sw.hadjustment();
                let vadj = sw.vadjustment();
                let vp_w = hadj.page_size();
                let vp_h = vadj.page_size();

                if char_w > 0.0 && char_h > 0.0 && vp_w > 20.0 && vp_h > 20.0 {
                    let cols = ((vp_w / char_w).floor() as u16).max(10);
                    let rows = ((vp_h / char_h).floor() as u16).max(5);
                    tp.last_known_width  = width;
                    tp.last_known_height = height;
                    let size = PtySize {
                        rows,
                        cols,
                        pixel_width:  vp_w as u16,
                        pixel_height: vp_h as u16,
                    };
                    if let Ok(master) = tp.master_pty.lock() {
                        let _ = master.resize(size);
                    }
                    tp.state.lock().unwrap().resize(cols as usize, rows as usize);
                } else {
                    tp.last_known_width  = 0;
                    tp.last_known_height = 0;
                }
            }

            // ---- Render ----
            {
                let mut state = tp.state.lock().unwrap();
                let cy = state.cursor_y;
                if let Some(markup) = state.render_markup() {
                    let buf = tp.view.buffer();
                    buf.set_text("");
                    let mut iter = buf.start_iter();
                    buf.insert_markup(&mut iter, &markup);
                    // Scroll to cursor line
                    let cy_safe = cy.min(state.rows.saturating_sub(1));
                    if let Some(iter) = buf.iter_at_line(cy_safe as i32) {
                        let mark = buf.create_mark(None, &iter, false);
                        tp.view.scroll_to_mark(&mark, 0.0, true, 0.0, 1.0);
                        buf.delete_mark(&mark);
                    }
                }
            }

            // ---- Drain pending PTY responses ----
            let responses: Vec<Vec<u8>>;
            let new_title: Option<String>;
            let clipboard_data: Option<String>;
            {
                let mut state = tp.state.lock().unwrap();
                responses      = std::mem::take(&mut state.pending_responses);
                new_title      = state.pending_title.take();
                clipboard_data = state.pending_clipboard.take();
            }
            if !responses.is_empty() {
                if let Ok(mut w) = tp.pty_writer.lock() {
                    for resp in responses { let _ = w.write_all(&resp); }
                }
            }
            if let Some(title) = new_title {
                if let Some(win) = window_weak2.upgrade() {
                    win.set_title(Some(&title));
                }
            }
            if let Some(b64) = clipboard_data {
                let decoded = glib::base64_decode(&b64);
                if let Ok(text) = String::from_utf8(decoded) {
                    if let Some(display) = gtk::gdk::Display::default() {
                        display.clipboard().set_text(&text);
                    }
                }
            }
        }

        glib::ControlFlow::Continue
    });
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn apply_font_css(provider: &CssProvider, size: i32) {
    provider.load_from_data(&format!(
        "textview {{ font-size: {size}px; font-family: monospace; padding: 0; border: 0; margin: 0; }}
         textview text {{ padding: 0; margin: 0; }}
         scrolledwindow {{ padding: 0; border: 0; }}"
    ));
}

fn measure_char_cell(view: &gtk::TextView) -> (f64, f64) {
    let ctx = view.pango_context();
    let lay = gtk::pango::Layout::new(&ctx);
    const N: usize = 100;
    lay.set_text(&"W".repeat(N));
    let (pw, ph) = lay.size();
    let scale = gtk::pango::SCALE as f64;
    (pw as f64 / scale / N as f64, ph as f64 / scale)
}

/// Attach mouse event controllers to a terminal pane's view.
fn setup_mouse_controllers(tp: &TerminalPane) {
    let view = tp.view.clone();

    // Char-cell dimensions for coordinate conversion
    let char_dims = {
        let (cw, ch) = measure_char_cell(&view);
        std::rc::Rc::new(RefCell::new((cw, ch)))
    };

    // ---- Click (press + release) ----
    let gesture = gtk::GestureClick::new();
    gesture.set_button(0);
    let writer_press = tp.pty_writer.clone();
    let state_press  = tp.state.clone();
    let dims_press   = char_dims.clone();
    gesture.connect_pressed(move |g, _n, x, y| {
        let mode = state_press.lock().unwrap().mouse_mode;
        if mode == 0 { return; }
        let (cw, ch) = *dims_press.borrow();
        if cw <= 0.0 || ch <= 0.0 { return; }
        let col = (x / cw).floor() as u32 + 1;
        let row = (y / ch).floor() as u32 + 1;
        let btn = match g.current_button() { 1 => 0, 2 => 1, 3 => 2, _ => return };
        let seq = format!("\x1b[<{btn};{col};{row}M");
        if let Ok(mut w) = writer_press.lock() { let _ = w.write_all(seq.as_bytes()); }
    });
    let writer_rel  = tp.pty_writer.clone();
    let state_rel   = tp.state.clone();
    let dims_rel    = char_dims.clone();
    gesture.connect_released(move |g, _n, x, y| {
        let mode = state_rel.lock().unwrap().mouse_mode;
        if mode == 0 { return; }
        let (cw, ch) = *dims_rel.borrow();
        if cw <= 0.0 || ch <= 0.0 { return; }
        let col = (x / cw).floor() as u32 + 1;
        let row = (y / ch).floor() as u32 + 1;
        let btn = match g.current_button() { 1 => 0, 2 => 1, 3 => 2, _ => return };
        let seq = format!("\x1b[<{btn};{col};{row}m");
        if let Ok(mut w) = writer_rel.lock() { let _ = w.write_all(seq.as_bytes()); }
    });
    view.add_controller(gesture);

    // ---- Motion ----
    let motion = gtk::EventControllerMotion::new();
    let writer_motion = tp.pty_writer.clone();
    let state_motion  = tp.state.clone();
    let dims_motion   = char_dims.clone();
    motion.connect_motion(move |_, x, y| {
        let mode = state_motion.lock().unwrap().mouse_mode;
        if mode < 1002 { return; }
        let (cw, ch) = *dims_motion.borrow();
        if cw <= 0.0 || ch <= 0.0 { return; }
        let col = (x / cw).floor() as u32 + 1;
        let row = (y / ch).floor() as u32 + 1;
        let seq = format!("\x1b[<32;{col};{row}M");
        if let Ok(mut w) = writer_motion.lock() { let _ = w.write_all(seq.as_bytes()); }
    });
    view.add_controller(motion);

    // ---- Scroll ----
    let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
    let writer_scroll = tp.pty_writer.clone();
    let state_scroll  = tp.state.clone();
    scroll.connect_scroll(move |_, _dx, dy| {
        let mut sl = state_scroll.lock().unwrap();
        let mode = sl.mouse_mode;
        if mode == 0 {
            sl.scroll_by(-(dy.round() as i32));
            return glib::Propagation::Stop;
        }
        drop(sl);
        let btn = if dy < 0.0 { 64u32 } else { 65u32 };
        let seq = format!("\x1b[<{btn};1;1M");
        if let Ok(mut w) = writer_scroll.lock() { let _ = w.write_all(seq.as_bytes()); }
        glib::Propagation::Stop
    });
    view.add_controller(scroll);
}
