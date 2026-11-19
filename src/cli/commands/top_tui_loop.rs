//! The live Ratatui loop for `daemoneye top`: raw-mode and alternate-screen
//! lifecycle, background key reader thread, and the 500 ms tick / event /
//! disk-read / attach-frame `tokio::select!` driver.

use super::*;

/// Wait for one frame from the open attach view's channel: `Some` applies
/// it, `None` marks the model's stream ended and keeps the view. Called
/// from `run_tui`'s select arm; the receive is cancel-safe, and the channel
/// closing (its task ended) is the stream ending too.
async fn attach_frame(model: &mut TopModel) -> Option<Response> {
    model.attach.as_mut()?.view.frames.recv().await
}

/// Open the attach view the last key asked for: the request, the first
/// frame, and the forwarding task. A failure names itself in the footer and
/// the panels stay.
async fn open_requested_attach(model: &mut TopModel) {
    let Some(id) = model
        .attach_requested
        .then(|| model.attach_candidate())
        .flatten()
    else {
        model.attach_requested = false;
        return;
    };
    model.attach_requested = false;
    match open_attach(&id).await {
        AttachOpen::Opened(view) => model.attach = Some(AttachBox { view: *view }),
        AttachOpen::Failed(msg) => model.attach_note = Some(msg),
    }
}

/// The live loop. Enters raw mode, the alternate screen and a hidden cursor;
/// the `AltScreenGuard` teardown leaves all three, so `?` cannot strand the
/// terminal. A thread reads keys into a channel; the tokio loop waits on it
/// and a 500 ms tick, re-reading only what [`plan_refresh`] asked for, and
/// redraws after every key and every tick.
pub(crate) async fn run_tui(
    config_cap: f64,
    filter: String,
    open_run: Option<crate::daemon::run::RunId>,
) -> Result<()> {
    enable_raw_mode()?;
    let teardown = || {
        let _ = crossterm::execute!(
            std::io::stdout(),
            LeaveAlternateScreen,
            crossterm::cursor::Show
        );
        let _ = disable_raw_mode();
    };
    crossterm::execute!(
        std::io::stdout(),
        EnterAlternateScreen,
        crossterm::cursor::Hide
    )?;
    let _guard = AltScreenGuard::new(teardown);

    let backend = CrosstermBackend::new(std::io::stdout());
    let mut terminal = Terminal::new(backend)?;

    let snapshot = crate::cli::commands::top::take_snapshot(config_cap).await;
    let mut model = TopModel::new(snapshot);
    model.filter = filter;
    // The snapshot's panels are the content the first frame sizes to, so the
    // key handling clamps against real heights from the first key.
    if let Ok(size) = terminal.size() {
        model.after_refresh(Some(ratatui::layout::Rect::new(
            0,
            0,
            size.width,
            size.height,
        )));
    }

    let running = Arc::new(AtomicBool::new(true));
    let (event_tx, mut event_rx) = tokio::sync::mpsc::unbounded_channel();
    let reader_running = Arc::clone(&running);
    let reader = std::thread::spawn(move || {
        while reader_running.load(Ordering::Relaxed) {
            if crossterm::event::poll(TICK).unwrap_or(false)
                && let Ok(event) = crossterm::event::read()
                && event_tx.send(event).is_err()
            {
                break;
            }
        }
    });

    let mut last_fp = None;
    let mut since_disk = Duration::ZERO;
    let mut since_shells = Duration::ZERO;
    let mut disk_in_flight = false;
    let (disk_tx, mut disk_rx) = tokio::sync::mpsc::unbounded_channel();
    let mut tick = tokio::time::interval(TICK);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    let mut last_size = None;
    if let Some(id) = open_run {
        model.open_detail(id);
        model.after_refresh(Some(ratatui::layout::Rect::new(
            0,
            0,
            terminal.size().map(|s| s.width).unwrap_or(80),
            terminal.size().map(|s| s.height).unwrap_or(24),
        )));
    }
    let result = loop {
        let size = terminal
            .size()
            .ok()
            .map(|s| ratatui::layout::Rect::new(0, 0, s.width, s.height));
        if size != last_size {
            last_size = size;
            model.after_refresh(size);
        }
        tokio::select! {
            maybe_event = event_rx.recv() => {
                match maybe_event {
                    Some(Event::Key(key)) => model.handle_key(key),
                    Some(_) => {}
                    None => break Ok(()),
                }
                if model.attach_requested && model.attach.is_none() {
                    open_requested_attach(&mut model).await;
                }
                if model.quit {
                    break Ok(());
                }
                terminal.draw(|f| draw(f, &model))?;
            }
            _ = tick.tick() => {
                since_disk += TICK;
                since_shells += TICK;
                let now_fp = fingerprint();
                let plan =
                    plan_refresh(last_fp, now_fp, since_disk, since_shells, disk_in_flight);
                if plan.disk && !disk_in_flight {
                    let tx = disk_tx.clone();
                    let cap = config_cap;
                    tokio::task::spawn_blocking(move || {
                        let disk = crate::cli::commands::top::disk_panels(cap);
                        let _ = tx.send(disk);
                    });
                    disk_in_flight = true;
                    last_fp = Some(now_fp);
                }
                if plan.shells {
                    shells_refresh(&mut model, &IpcFetcher).await;
                    model.after_refresh(size);
                    since_shells = Duration::ZERO;
                }
                terminal.draw(|f| draw(f, &model))?;
            }
            Some(disk) = disk_rx.recv() => {
                disk_in_flight = false;
                since_disk = Duration::ZERO;
                model.apply_disk(disk);
                model.after_refresh(size);
                terminal.draw(|f| draw(f, &model))?;
            }
            frame = attach_frame(&mut model), if model.attach.is_some() => {
                model.apply_attach_frame(frame);
                terminal.draw(|f| draw(f, &model))?;
            }
        }
    };

    running.store(false, Ordering::Relaxed);
    let _ = reader.join();
    result
}
