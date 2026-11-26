//! Hot-plug: lists the serial ports on a thread of its own and tells the UI when the list of
//! boards changes (#19, ADR-008 §3, ADR-011 §5).
//!
//! It only **enumerates**: asking the OS for the device list never opens a port, so plugging a
//! board in never resets it. Every [`WATCH_INTERVAL`] the ports are split by [`pre_group`]; the
//! [`Grouping`] is sent as [`Event::Boards`] only when it differs from the last one sent, so the
//! UI redraws on a change, not on a timer.
//!
//! The thread stops on its own when the [`Watcher`] is dropped or [`Watcher::stop`]ped (the stop
//! channel disconnects and wakes it immediately) or when the UI's receiver is gone.

use std::io;
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use super::group::{Grouping, pre_group};
use super::worker::{Event, wait_finished};
use crate::transport::{self, PortDesc};

/// How often the OS port list is read.
pub const WATCH_INTERVAL: Duration = Duration::from_millis(500);

/// The hot-plug thread. Dropping it asks the thread to stop without waiting for it.
#[derive(Debug)]
pub struct Watcher {
    stop: Sender<()>,
    handle: JoinHandle<()>,
}

impl Watcher {
    /// Starts the thread. `enumerate` lists the ports; `None` (an OS error) skips that round
    /// and keeps the last list, so a transient error does not make every board vanish.
    pub fn spawn<E>(
        mut enumerate: E,
        interval: Duration,
        events: Sender<Event>,
    ) -> io::Result<Watcher>
    where
        E: FnMut() -> Option<Vec<PortDesc>> + Send + 'static,
    {
        let (stop, stopped) = mpsc::channel::<()>();
        let handle = thread::Builder::new()
            .name("hot-plug".to_owned())
            .spawn(move || {
                let mut last: Option<Grouping> = None;
                loop {
                    if let Some(ports) = enumerate() {
                        let grouping = pre_group(&ports);
                        if last.as_ref() != Some(&grouping) {
                            if events.send(Event::Boards(grouping.clone())).is_err() {
                                return;
                            }
                            last = Some(grouping);
                        }
                    }
                    match stopped.recv_timeout(interval) {
                        Err(RecvTimeoutError::Timeout) => {}
                        Ok(()) | Err(RecvTimeoutError::Disconnected) => return,
                    }
                }
            })?;
        Ok(Watcher { stop, handle })
    }

    /// Starts the thread on the OS port list ([`transport::enumerate`]) every [`WATCH_INTERVAL`].
    pub fn spawn_system(events: Sender<Event>) -> io::Result<Watcher> {
        Watcher::spawn(|| transport::enumerate().ok(), WATCH_INTERVAL, events)
    }

    /// Asks the thread to stop and waits for it at most `timeout` (it may be in the middle of an
    /// enumeration). True if it ended; otherwise it is left to end on its own.
    pub fn stop(self, timeout: Duration) -> bool {
        let Watcher { stop, handle } = self;
        drop(stop);
        wait_finished(handle, timeout)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};
    use std::time::Instant;

    use super::*;
    use crate::transport::UsbIds;

    fn native(name: &str, serial: &str) -> PortDesc {
        PortDesc {
            name: name.to_owned(),
            usb: Some(UsbIds {
                vid: 0x303A,
                pid: 0x1001,
                serial: Some(serial.to_owned()),
                manufacturer: None,
                product: None,
                interface: Some(0),
            }),
        }
    }

    #[test]
    fn hotplug_sends_only_when_the_list_changes() {
        let c6 = native("COM11", "E8:F6:0A:F7:FF:D4");
        let c3 = native("COM6", "80:53:E0:0A:5D:70");
        // Same list twice, an OS error (kept), then a second board; then it stays the same.
        let script = Arc::new(Mutex::new(vec![
            Some(vec![c6.clone()]),
            Some(vec![c6.clone()]),
            None,
            Some(vec![c6.clone(), c3.clone()]),
        ]));
        let rounds = Arc::new(Mutex::new(0usize));
        let enumerate = {
            let script = Arc::clone(&script);
            let rounds = Arc::clone(&rounds);
            move || {
                *rounds.lock().unwrap() += 1;
                let mut script = script.lock().unwrap();
                if script.len() > 1 {
                    script.remove(0)
                } else {
                    script.first().cloned().flatten()
                }
            }
        };
        let (tx, rx) = mpsc::channel();
        let watcher = Watcher::spawn(enumerate, Duration::from_millis(1), tx).unwrap();

        let deadline = Instant::now() + Duration::from_secs(5);
        while *rounds.lock().unwrap() < 10 && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(5));
        }
        assert!(watcher.stop(Duration::from_secs(5)), "thread did not stop");

        let sent: Vec<Grouping> = rx
            .try_iter()
            .map(|event| match event {
                Event::Boards(grouping) => grouping,
                other => panic!("unexpected {other:?}"),
            })
            .collect();
        assert_eq!(sent.len(), 2, "{sent:?}");
        assert_eq!(sent[0], pre_group(std::slice::from_ref(&c6)));
        assert_eq!(sent[1], pre_group(&[c6, c3]));
    }

    #[test]
    fn dropping_the_watcher_stops_the_thread() {
        let (tx, _rx) = mpsc::channel();
        let watcher = Watcher::spawn(|| Some(Vec::new()), Duration::from_secs(60), tx).unwrap();
        let start = Instant::now();
        assert!(watcher.stop(Duration::from_secs(5)));
        assert!(start.elapsed() < Duration::from_secs(5));
    }
}
