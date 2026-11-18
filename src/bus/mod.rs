//! The in-process notice bus: producers [`publish`](bus::publish) one
//! [`Notice`] per thing they want known; the bus journals it, then fans it
//! out to inline and queued subscribers whose [`Filter`] matches.
//!
//! **Journal first.** `publish` masks `title`/`body`, appends the notice to
//! the event log (`event: "notice"`), then delivers. The journal is the
//! durable, ordered record — a full queue drops a notice for one subscriber
//! only, and the journal is the replay.
//!
//! **Two kinds of consumer.** *Inline* subscribers run inside `publish`
//! (after the journal) and must be cheap and non-blocking. *Queued*
//! subscribers get a bounded mpsc queue each; a full queue drops and counts
//! for that subscriber only. Publishes are serialized under one lock, so
//! the journal and every subscriber see the same order.

use std::sync::{Arc, Mutex, OnceLock};

use serde_json::json;

mod filter;
mod journal;
mod notice;
mod severity;

pub use filter::Filter;
pub use journal::{for_each_notice_between, notice_from_record};
pub use notice::{Notice, NoticeStatus, Source};
pub use severity::Severity;

use crate::ai::mask_sensitive;
use crate::daemon::utils::{UnpoisonExt, log_event};

/// One subscriber's counters and delivery kind, as `stats` reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubscriberStats {
    pub name: String,
    pub inline: bool,
    pub delivered: u64,
    pub dropped: u64,
}

/// What a subscriber is: an inline callback or a queued sender.
enum Subscriber {
    Inline {
        name: String,
        filter: Filter,
        f: Box<dyn Fn(&Notice) + Send + Sync>,
        delivered: u64,
    },
    Queued {
        name: String,
        filter: Filter,
        tx: tokio::sync::mpsc::Sender<Arc<Notice>>,
        delivered: u64,
        dropped: u64,
    },
}

impl Subscriber {
    fn matches(&self, n: &Notice) -> bool {
        match self {
            Subscriber::Inline { filter, .. } | Subscriber::Queued { filter, .. } => {
                filter.matches(n)
            }
        }
    }

    fn stats(&self) -> SubscriberStats {
        match self {
            Subscriber::Inline {
                name, delivered, ..
            } => SubscriberStats {
                name: name.clone(),
                inline: true,
                delivered: *delivered,
                dropped: 0,
            },
            Subscriber::Queued {
                name,
                delivered,
                dropped,
                ..
            } => SubscriberStats {
                name: name.clone(),
                inline: false,
                delivered: *delivered,
                dropped: *dropped,
            },
        }
    }
}

/// The bus. Create with [`Bus::new`] (journals through `log_event`) or
/// [`Bus::with_journal`] (tests inject a recording journal). Publishes are
/// serialized under one lock.
pub struct Bus {
    /// Serializes publishes so the journal and every subscriber see the same
    /// order. Re-entry is detected per thread by [`IN_PUBLISH`], not by lock
    /// contention.
    publish_lock: Mutex<()>,
    subscribers: Mutex<Vec<Subscriber>>,
    journal: Box<dyn Fn(&Notice) + Send + Sync>,
}

thread_local! {
    /// Whether this thread is inside a `publish` (an inline subscriber's
    /// callback is running). Detects re-entry per thread: a `try_lock` on
    /// `publish_lock` also fails when a *different* thread holds the lock,
    /// which would misreport concurrent publishes as re-entrant and silently
    /// skip their journal and delivery.
    static IN_PUBLISH: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Clears [`IN_PUBLISH`] on drop, so a panicking inline subscriber cannot
/// leave the flag set for the rest of the thread's life.
struct PublishGuard;

impl PublishGuard {
    fn enter() -> Self {
        IN_PUBLISH.with(|f| f.set(true));
        Self
    }
}

impl Drop for PublishGuard {
    fn drop(&mut self) {
        IN_PUBLISH.with(|f| f.set(false));
    }
}

impl Bus {
    /// A bus that journals through `log_event("notice", …)`.
    pub fn new() -> Self {
        Self::with_journal(|n| {
            let value = serde_json::to_value(n).unwrap_or_else(|_| json!({}));
            log_event("notice", value);
        })
    }

    /// A bus with an injected journal — tests record into a `Vec` instead of
    /// the event log.
    pub fn with_journal(f: impl Fn(&Notice) + Send + Sync + 'static) -> Self {
        Self {
            publish_lock: Mutex::new(()),
            subscribers: Mutex::new(Vec::new()),
            journal: Box::new(f),
        }
    }

    /// Register an inline subscriber: `f` runs inside `publish`, after the
    /// journal, in subscription order. Must be cheap and non-blocking.
    pub fn subscribe_inline(
        &self,
        name: impl Into<String>,
        filter: Filter,
        f: impl Fn(&Notice) + Send + Sync + 'static,
    ) {
        let mut subs = self.subscribers.lock().unwrap_or_log();
        subs.push(Subscriber::Inline {
            name: name.into(),
            filter,
            f: Box::new(f),
            delivered: 0,
        });
    }

    /// Register a queued subscriber and return its receiving end. Delivery
    /// uses `try_send`: a full queue drops the notice for this subscriber
    /// only (counted), a closed receiver removes the subscriber on the next
    /// publish.
    pub fn subscribe(
        &self,
        name: impl Into<String>,
        filter: Filter,
        capacity: usize,
    ) -> tokio::sync::mpsc::Receiver<Arc<Notice>> {
        let (tx, rx) = tokio::sync::mpsc::channel(capacity.max(1));
        let mut subs = self.subscribers.lock().unwrap_or_log();
        subs.push(Subscriber::Queued {
            name: name.into(),
            filter,
            tx,
            delivered: 0,
            dropped: 0,
        });
        rx
    }

    /// Mask, journal, then deliver to every matching subscriber — in that
    /// order, under one lock.
    ///
    /// A publish from inside an inline subscriber on the same thread must
    /// not deadlock: it logs at `error`, skips the journal and delivery, and
    /// returns the masked notice.
    pub fn publish(&self, notice: Notice) -> Arc<Notice> {
        let masked = Notice {
            title: mask_sensitive(&notice.title),
            body: mask_sensitive(&notice.body),
            ..notice
        };
        let masked = Arc::new(masked);

        if IN_PUBLISH.with(std::cell::Cell::get) {
            log::error!(
                "bus: re-entrant publish of notice '{}' from an inline subscriber; \
                 journal and delivery skipped",
                masked.name
            );
            return masked;
        }
        let _guard = PublishGuard::enter();

        let _lock = self.publish_lock.lock().unwrap_or_log();

        (self.journal)(&masked);

        let mut subs = self.subscribers.lock().unwrap_or_log();
        let mut removed: Vec<usize> = Vec::new();
        for (i, s) in subs.iter_mut().enumerate() {
            if !s.matches(&masked) {
                continue;
            }
            match s {
                Subscriber::Inline { f, delivered, .. } => {
                    *delivered += 1;
                    f(&masked);
                }
                Subscriber::Queued {
                    tx,
                    delivered,
                    dropped,
                    ..
                } => match tx.try_send(Arc::clone(&masked)) {
                    Ok(()) => *delivered += 1,
                    Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                        removed.push(i);
                    }
                    Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                        *dropped += 1;
                    }
                },
            }
        }
        // `remove`, not `swap_remove`: inline subscribers run in
        // subscription order, and a swap would move the last one forward.
        for i in removed.into_iter().rev() {
            subs.remove(i);
        }
        masked
    }

    /// A snapshot of every subscriber's counters, in subscription order.
    pub fn stats(&self) -> Vec<SubscriberStats> {
        self.subscribers
            .lock()
            .unwrap_or_log()
            .iter()
            .map(Subscriber::stats)
            .collect()
    }
}

impl Default for Bus {
    fn default() -> Self {
        Self::new()
    }
}

/// The process-wide bus.
pub fn global() -> &'static Bus {
    static BUS: OnceLock<Bus> = OnceLock::new();
    BUS.get_or_init(Bus::new)
}

/// Publish onto [`global`].
pub fn publish(notice: Notice) -> Arc<Notice> {
    global().publish(notice)
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};
    use std::thread;

    use super::{Bus, Filter, Notice, Severity, Source, SubscriberStats};
    use crate::bus::severity::Severity as Sev;

    fn notice() -> Notice {
        Notice::new(Severity::Warning, Source::Webhook, "HighCPU", "CPU high")
    }

    struct Recorded {
        ids: Mutex<Vec<String>>,
    }

    impl Recorded {
        fn new() -> Arc<Self> {
            Arc::new(Recorded {
                ids: Mutex::new(Vec::new()),
            })
        }

        fn journal(self: &Arc<Self>) -> impl Fn(&Notice) + Send + Sync + 'static {
            let rec = Arc::clone(self);
            move |n: &Notice| {
                let mut ids = rec.ids.lock().unwrap_or_log();
                ids.push(n.id.clone());
            }
        }

        fn ids(&self) -> Vec<String> {
            self.ids.lock().unwrap_or_log().clone()
        }
    }

    use crate::util::UnpoisonExt;

    fn take_ids(rx: &mut tokio::sync::mpsc::Receiver<Arc<Notice>>) -> Vec<String> {
        let mut ids = Vec::new();
        while let Ok(n) = rx.try_recv() {
            ids.push(n.id.clone());
        }
        ids
    }

    #[test]
    fn bus_journals_before_inline_delivery() {
        let rec = Recorded::new();
        let bus = Bus::with_journal(rec.journal());
        let rec_for_cb = Arc::clone(&rec);
        let seen: Arc<Mutex<Vec<(String, bool)>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&seen);
        bus.subscribe_inline("watch", Filter::default(), move |n| {
            // The journal must already hold this notice when the inline
            // subscriber runs.
            let journal = rec_for_cb.ids();
            sink.lock()
                .unwrap()
                .push((n.id.clone(), journal.contains(&n.id)));
        });
        let n = notice();
        bus.publish(n);
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 1, "one inline delivery");
        assert!(seen[0].1, "the journal held the notice before delivery");
        assert_eq!(rec.ids(), vec![seen[0].0.clone()]);
    }

    #[test]
    fn bus_delivers_only_to_matching_subscribers() {
        let bus = Bus::with_journal(|_| {});
        let hits: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&hits);
        bus.subscribe_inline(
            "critical-only",
            Filter {
                min_severity: Some(Severity::Critical),
                ..Filter::default()
            },
            move |n| sink.lock().unwrap().push(n.id.clone()),
        );
        bus.publish(notice());
        bus.publish(Notice::new(
            Severity::Critical,
            Source::Webhook,
            "Pager",
            "down",
        ));
        let hits = hits.lock().unwrap();
        assert_eq!(hits.len(), 1, "only the critical notice was delivered");
    }

    #[test]
    fn bus_masks_title_and_body() {
        const KEY: &str = "AKIAIOSFODNN7EXAMPLE";
        let journal: Arc<Mutex<Vec<Notice>>> = Arc::new(Mutex::new(Vec::new()));
        let j_sink = Arc::clone(&journal);
        let bus = Bus::with_journal(move |n| j_sink.lock().unwrap().push(n.clone()));
        let seen: Arc<Mutex<Vec<Notice>>> = Arc::new(Mutex::new(Vec::new()));
        let s_sink = Arc::clone(&seen);
        bus.subscribe_inline("s", Filter::default(), move |n| {
            s_sink.lock().unwrap().push(n.clone())
        });
        bus.publish(Notice {
            title: format!("key {KEY} leaked"),
            body: format!("body {KEY} leaked"),
            ..notice()
        });
        let journal = journal.lock().unwrap();
        let seen = seen.lock().unwrap();
        for (label, notices) in [("journal", &*journal), ("subscriber", &*seen)] {
            assert_eq!(notices.len(), 1, "{label} got the notice");
            let n = &notices[0];
            assert!(
                !n.title.contains(KEY) && !n.body.contains(KEY),
                "{label} never sees the raw key"
            );
            assert!(
                n.title.contains("<AWS_KEY>") && n.body.contains("<AWS_KEY>"),
                "{label} sees the masked key: title={:?} body={:?}",
                n.title,
                n.body
            );
        }
    }

    #[test]
    fn bus_full_queue_drops_and_counts() {
        let bus = Bus::with_journal(|_| {});
        let mut full = bus.subscribe("full", Filter::default(), 1);
        let mut roomy = bus.subscribe("roomy", Filter::default(), 10);
        for _ in 0..3 {
            bus.publish(notice());
        }
        assert_eq!(take_ids(&mut full).len(), 1, "capacity 1 holds one");
        assert_eq!(
            bus.stats(),
            vec![
                SubscriberStats {
                    name: "full".to_string(),
                    inline: false,
                    delivered: 1,
                    dropped: 2,
                },
                SubscriberStats {
                    name: "roomy".to_string(),
                    inline: false,
                    delivered: 3,
                    dropped: 0,
                },
            ]
        );
        assert_eq!(take_ids(&mut roomy).len(), 3, "roomy subscriber gets all 3");
    }

    #[test]
    fn bus_closed_subscriber_is_removed() {
        let bus = Bus::with_journal(|_| {});
        let rx = bus.subscribe("leaves", Filter::default(), 4);
        drop(rx);
        bus.publish(notice());
        bus.publish(notice());
        assert!(
            bus.stats().is_empty(),
            "a closed receiver is removed on the next publish"
        );
    }

    #[test]
    fn bus_concurrent_publishers_same_order_everywhere() {
        let rec = Recorded::new();
        let bus = Arc::new(Bus::with_journal(rec.journal()));
        let mut rx_a = bus.subscribe("a", Filter::default(), 1000);
        let mut rx_b = bus.subscribe("b", Filter::default(), 1000);
        let mut handles = Vec::new();
        for _ in 0..4 {
            let bus = Arc::clone(&bus);
            handles.push(thread::spawn(move || {
                for _ in 0..50 {
                    bus.publish(notice());
                }
            }));
        }
        for h in handles {
            h.join().expect("publisher thread");
        }
        let journal = rec.ids();
        let a = take_ids(&mut rx_a);
        let b = take_ids(&mut rx_b);
        assert_eq!(journal.len(), a.len(), "journal and subscriber a agree");
        assert_eq!(a.len(), b.len(), "subscribers a and b agree");
        assert_eq!(a, b, "subscribers a and b hold the same order");
        assert_eq!(journal, a, "the journal order matches subscriber a");
        let mut unique = a.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), 200, "200 distinct notices");
    }

    #[test]
    fn bus_reentrant_publish_does_not_deadlock() {
        let rec = Recorded::new();
        let journal = Arc::clone(&rec);
        let bus = Arc::new(Bus::with_journal(rec.journal()));
        let bus_for_cb = Arc::clone(&bus);
        let reentrant: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let re_sink = Arc::clone(&reentrant);
        bus.subscribe_inline("reentrant", Filter::default(), move |_| {
            let inner = bus_for_cb.publish(Notice::new(
                Severity::Error,
                Source::Daemon,
                "Reentrant",
                "from inside",
            ));
            *re_sink.lock().unwrap() = Some(inner.id.clone());
        });
        let outer = notice();
        let outer_id = outer.id.clone();
        let returned = bus.publish(outer);
        assert_eq!(
            returned.id, outer_id,
            "the outer call returns the outer notice"
        );
        let inner_id = reentrant
            .lock()
            .unwrap()
            .clone()
            .expect("the inline subscriber ran and published");
        assert_ne!(
            inner_id, outer_id,
            "the inner publish was a distinct notice"
        );
        let journal_ids = journal.ids();
        assert_eq!(
            journal_ids.len(),
            1,
            "the re-entrant publish skips the journal"
        );
        assert_eq!(journal_ids[0], outer_id);
    }

    #[test]
    fn bus_stats_reports_inline_and_dropped() {
        let bus = Bus::with_journal(|_| {});
        bus.subscribe_inline("inline", Filter::default(), |_| {});
        let mut rx = bus.subscribe("queued", Filter::default(), 1);
        for _ in 0..2 {
            bus.publish(notice());
        }
        let _ = take_ids(&mut rx);
        assert_eq!(
            bus.stats(),
            vec![
                SubscriberStats {
                    name: "inline".to_string(),
                    inline: true,
                    delivered: 2,
                    dropped: 0,
                },
                SubscriberStats {
                    name: "queued".to_string(),
                    inline: false,
                    delivered: 1,
                    dropped: 1,
                },
            ]
        );
    }

    #[test]
    fn bus_inline_subscribers_run_in_subscription_order() {
        let bus = Bus::with_journal(|_| {});
        let order: Arc<Mutex<Vec<&'static str>>> = Arc::new(Mutex::new(Vec::new()));
        for name in ["first", "second", "third"] {
            let sink = Arc::clone(&order);
            bus.subscribe_inline(name, Filter::default(), move |_| {
                sink.lock().unwrap().push(name)
            });
        }
        bus.publish(notice());
        assert_eq!(
            *order.lock().unwrap(),
            vec!["first", "second", "third"],
            "inline subscribers run in subscription order"
        );
    }

    #[test]
    fn bus_removing_a_closed_subscriber_keeps_inline_order() {
        let bus = Bus::with_journal(|_| {});
        let order: Arc<Mutex<Vec<&'static str>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&order);
        bus.subscribe_inline("first", Filter::default(), move |_| {
            sink.lock().unwrap().push("first")
        });
        drop(bus.subscribe("closed", Filter::default(), 4));
        for name in ["second", "third"] {
            let sink = Arc::clone(&order);
            bus.subscribe_inline(name, Filter::default(), move |_| {
                sink.lock().unwrap().push(name)
            });
        }
        bus.publish(notice());
        order.lock().unwrap().clear();
        bus.publish(notice());
        assert_eq!(
            *order.lock().unwrap(),
            vec!["first", "second", "third"],
            "removing a closed queued subscriber must not reorder the rest"
        );
    }

    #[test]
    fn severity_alias_smoke() {
        assert_eq!(Sev::parse("warn"), Some(Sev::Warning));
    }

    #[test]
    fn bus_new_journals_a_notice_event() {
        let (_guard, _home) = crate::test_home();
        let bus = Bus::new();
        let n = notice();
        bus.publish(n.clone());
        let seg = crate::config::current_event_segment_path();
        let content = std::fs::read_to_string(&seg).expect("the event segment exists");
        let mut found = false;
        for line in content.lines().rev() {
            let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
                continue;
            };
            if v["event"] == "notice"
                && v["id"] == n.id.as_str()
                && v["severity"] == n.severity.as_str()
            {
                found = true;
                break;
            }
        }
        assert!(
            found,
            "the segment holds one notice record with id+severity"
        );
    }
}
