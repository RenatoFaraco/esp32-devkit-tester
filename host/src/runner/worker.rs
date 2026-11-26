//! The runner's thread: one job at a time, events to the UI (#20, ADR-011 §5).
//!
//! espflash and `serialport` block until the board answers or a timeout expires (3 s per read,
//! about 10 s for a connect), so nothing that talks to a board may run on the UI thread. The UI
//! sends a [`Job`] through [`Worker::submit`] and keeps drawing; this thread runs it with the
//! sequential runner ([`identify`], [`run_board`]) and sends [`Event`]s back through a
//! `std::sync::mpsc` channel as things happen.
//!
//! # Cancelling
//! [`Worker::cancel`] raises an atomic flag. A blocking call is never interrupted in the middle
//! (the ceiling is that call's own timeout); the flag is checked **between steps**, by wrapping
//! what the runner is given instead of changing the runner:
//! - every [`Probe`] call except [`Probe::hard_reset`] fails fast with
//!   [`ProbeError::NotConnected`], so the suites stop at the next test ([`run_board`] then still
//!   performs the hard reset of the open session, which this wrapper lets through);
//! - waiting for a port to come back stops sleeping and stops looking;
//! - progress is no longer forwarded and the run ends with [`Event::RunCancelled`] instead of
//!   [`Event::RunFinished`].
//!
//! # Ending
//! The thread ends when the [`Worker`] is dropped (the job channel disconnects) once the job in
//! hand, if any, is over. [`Worker::shutdown`] cancels that job and waits for it for a bounded
//! time, so quitting never hangs on a board.

use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use super::group::{Board, Grouping, Identified, identify, run_board};
use super::{Clock, Progress, Sleeper, SystemClock};
use crate::chips::{Chip, ChipIdentity};
use crate::probe::{
    EmbeddedMem, EspflashProbe, FlashId, Probe, ProbeError, Revision, quiet_shielded_panics,
};
use crate::report::{Report, TestId, TestResult};
use crate::transport::{self, Mac, PortDesc};

/// What the UI asks the runner's thread to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Job {
    /// Open each port once to read its MAC (ADR-008 §4); only after the user confirmed.
    Identify(Vec<PortDesc>),
    /// Run the v0.1 catalog on one board, all its ports.
    Run(Board),
}

/// What the background threads tell the UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// The hot-plug thread found a different list of ports ([`super::watch`]).
    Boards(Grouping),
    /// One port of a [`Job::Identify`] was opened (or could not be).
    Identified(Identified),
    /// The [`Job::Identify`] is over (all ports done, or cancelled).
    IdentifyFinished,
    /// A test of the [`Job::Run`] is in flight.
    TestStarted(TestId),
    /// A test of the [`Job::Run`] has its final outcome.
    TestFinished(TestResult),
    /// The run waits for `port` to come back after a hard reset.
    Waiting {
        /// The port as enumerated before the reset.
        port: PortDesc,
    },
    /// The [`Job::Run`] is over: the complete report.
    RunFinished(Report),
    /// The [`Job::Run`] was cancelled; the board got its hard reset attempt.
    RunCancelled,
}

impl From<Progress> for Event {
    fn from(progress: Progress) -> Event {
        match progress {
            Progress::Started(id) => Event::TestStarted(id),
            Progress::Finished(test) => Event::TestFinished(test),
            Progress::Waiting { port } => Event::Waiting { port },
        }
    }
}

/// The runner's thread, seen from the UI.
#[derive(Debug)]
pub struct Worker {
    jobs: Sender<Job>,
    cancel: Arc<AtomicBool>,
    handle: JoinHandle<()>,
}

impl Worker {
    /// Starts the thread. `make_probe` builds a fresh [`Probe`] per session, `time` times the
    /// tests and paces the waits, `enumerate` lists the ports while a port is awaited; events go
    /// to `events`. All of them move to the new thread.
    pub fn spawn<P, T, E>(
        make_probe: P,
        time: T,
        enumerate: E,
        events: Sender<Event>,
    ) -> io::Result<Worker>
    where
        P: FnMut(&PortDesc) -> Box<dyn Probe> + Send + 'static,
        T: Clock + Sleeper + Send + 'static,
        E: FnMut() -> Vec<PortDesc> + Send + 'static,
    {
        let (jobs, inbox) = mpsc::channel::<Job>();
        let cancel = Arc::new(AtomicBool::new(false));
        let mut context = Context {
            make_probe,
            time,
            enumerate,
            events,
            cancel: Arc::clone(&cancel),
        };
        let handle = thread::Builder::new()
            .name("runner".to_owned())
            .spawn(move || {
                while let Ok(job) = inbox.recv() {
                    context.handle(job);
                }
            })?;
        Ok(Worker {
            jobs,
            cancel,
            handle,
        })
    }

    /// Starts the thread with the real board access: [`EspflashProbe`], the [`SystemClock`] and
    /// the OS port list (an enumeration error counts as an empty list while waiting for a port).
    pub fn spawn_system(events: Sender<Event>) -> io::Result<Worker> {
        Worker::spawn(
            |_: &PortDesc| -> Box<dyn Probe> { Box::new(EspflashProbe::new()) },
            SystemClock::new(),
            || transport::enumerate().unwrap_or_default(),
            events,
        )
    }

    /// Queues `job` and clears the cancel flag. False if the thread is gone.
    ///
    /// One job runs at a time; the UI only submits a job when the previous one has ended.
    pub fn submit(&self, job: Job) -> bool {
        self.cancel.store(false, Ordering::SeqCst);
        self.jobs.send(job).is_ok()
    }

    /// Asks the job in hand to stop at the next step (see the module docs).
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }

    /// True once the thread has ended (normally only after [`Worker::shutdown`]; earlier means it
    /// died, which is a bug).
    pub fn is_finished(&self) -> bool {
        self.handle.is_finished()
    }

    /// Cancels the job in hand, closes the job channel and waits at most `timeout` for the
    /// thread. True if it ended; otherwise it is left to end on its own (it holds no terminal).
    pub fn shutdown(self, timeout: Duration) -> bool {
        let Worker {
            jobs,
            cancel,
            handle,
        } = self;
        cancel.store(true, Ordering::SeqCst);
        drop(jobs);
        wait_finished(handle, timeout)
    }
}

/// Keeps the panic hook quiet about the panics espflash raises and the probe catches, so they do
/// not print over the TUI. Install it after the terminal's own hook (see
/// [`quiet_shielded_panics`]); the UI calls it through the runner because it never calls the
/// probe itself.
pub fn silence_shielded_panics() {
    quiet_shielded_panics();
}

/// Waits for `handle` to end, polling, for at most `timeout`; joins it if it did. Never blocks
/// longer than `timeout` (a plain `join` would wait for a board that does not answer).
pub(crate) fn wait_finished(handle: JoinHandle<()>, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while !handle.is_finished() {
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(Duration::from_millis(10));
    }
    handle.join().is_ok()
}

/// Everything the runner's thread owns.
struct Context<P, T, E> {
    make_probe: P,
    time: T,
    enumerate: E,
    events: Sender<Event>,
    cancel: Arc<AtomicBool>,
}

impl<P, T, E> Context<P, T, E>
where
    P: FnMut(&PortDesc) -> Box<dyn Probe>,
    T: Clock + Sleeper,
    E: FnMut() -> Vec<PortDesc>,
{
    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }

    /// Sends to the UI; a closed channel means the UI is gone, and the job ends on its own.
    fn send(&self, event: Event) {
        let _ = self.events.send(event);
    }

    fn handle(&mut self, job: Job) {
        match job {
            Job::Identify(ports) => self.identify(&ports),
            Job::Run(board) => self.run(&board),
        }
    }

    fn identify(&mut self, ports: &[PortDesc]) {
        let cancel = Arc::clone(&self.cancel);
        for port in ports {
            if self.cancelled() {
                break;
            }
            let make_probe = &mut self.make_probe;
            let mut factory = |port: &PortDesc| -> Box<dyn Probe> {
                Box::new(Cancellable {
                    inner: make_probe(port),
                    cancel: Arc::clone(&cancel),
                })
            };
            for result in identify(&mut factory, std::slice::from_ref(port)) {
                // A port whose identification was cut short by the cancel did not really fail:
                // it stays "not identified" instead of carrying a misleading reason.
                if self.cancelled() && matches!(result, Identified::Failed { .. }) {
                    continue;
                }
                self.send(Event::Identified(result));
            }
        }
        self.send(Event::IdentifyFinished);
    }

    fn run(&mut self, board: &Board) {
        let cancel = Arc::clone(&self.cancel);
        let Context {
            make_probe,
            time,
            enumerate,
            events,
            ..
        } = self;
        let mut factory = |port: &PortDesc| -> Box<dyn Probe> {
            Box::new(Cancellable {
                inner: make_probe(port),
                cancel: Arc::clone(&cancel),
            })
        };
        let sleeper = CancellableSleep {
            inner: &*time,
            cancel: &cancel,
        };
        let mut ports = || {
            if cancel.load(Ordering::SeqCst) {
                Vec::new()
            } else {
                enumerate()
            }
        };
        let mut observer = |progress: Progress| {
            if !cancel.load(Ordering::SeqCst) {
                let _ = events.send(Event::from(progress));
            }
        };
        let report = run_board(
            &mut factory,
            board,
            &*time,
            &sleeper,
            &mut ports,
            &mut observer,
        );
        if self.cancelled() {
            self.send(Event::RunCancelled);
        } else {
            self.send(Event::RunFinished(report));
        }
    }
}

/// A [`Probe`] that stops answering once the job is cancelled, except for the hard reset, which
/// always reaches the board (ADR-009 §4).
struct Cancellable {
    inner: Box<dyn Probe>,
    cancel: Arc<AtomicBool>,
}

impl Cancellable {
    fn go_on(&self) -> Result<(), ProbeError> {
        if self.cancel.load(Ordering::SeqCst) {
            Err(ProbeError::NotConnected)
        } else {
            Ok(())
        }
    }
}

impl Probe for Cancellable {
    fn connect(&mut self, port: &PortDesc) -> Result<(), ProbeError> {
        self.go_on()?;
        self.inner.connect(port)
    }
    fn chip(&mut self) -> Result<Chip, ProbeError> {
        self.go_on()?;
        self.inner.chip()
    }
    fn identity(&mut self) -> Result<ChipIdentity, ProbeError> {
        self.go_on()?;
        self.inner.identity()
    }
    fn revision(&mut self) -> Result<Revision, ProbeError> {
        self.go_on()?;
        self.inner.revision()
    }
    fn crystal_mhz(&mut self) -> Result<u32, ProbeError> {
        self.go_on()?;
        self.inner.crystal_mhz()
    }
    fn mac(&mut self) -> Result<Mac, ProbeError> {
        self.go_on()?;
        self.inner.mac()
    }
    fn read_reg(&mut self, addr: u32) -> Result<u32, ProbeError> {
        self.go_on()?;
        self.inner.read_reg(addr)
    }
    fn package_version(&mut self) -> Result<u32, ProbeError> {
        self.go_on()?;
        self.inner.package_version()
    }
    fn embedded_flash(&mut self) -> Result<Option<EmbeddedMem>, ProbeError> {
        self.go_on()?;
        self.inner.embedded_flash()
    }
    fn embedded_psram(&mut self) -> Result<Option<EmbeddedMem>, ProbeError> {
        self.go_on()?;
        self.inner.embedded_psram()
    }
    fn flash_id(&mut self) -> Result<FlashId, ProbeError> {
        self.go_on()?;
        self.inner.flash_id()
    }
    fn hard_reset(&mut self) -> Result<(), ProbeError> {
        self.inner.hard_reset()
    }
}

/// A [`Sleeper`] that does not sleep once the job is cancelled (the wait for a port then runs
/// out of polls at once).
struct CancellableSleep<'a, S: ?Sized> {
    inner: &'a S,
    cancel: &'a AtomicBool,
}

impl<S: Sleeper + ?Sized> Sleeper for CancellableSleep<'_, S> {
    fn sleep(&self, duration: Duration) {
        if !self.cancel.load(Ordering::SeqCst) {
            self.inner.sleep(duration);
        }
    }
}

/// A bench of fake boards for the worker and UI tests: the C6 of the bench (CH343 on `COM13`,
/// native port on `COM11`), a call log shared across threads and an optional gate that holds
/// the run at one step until the test releases it.
#[cfg(test)]
pub(crate) mod testing {
    use std::sync::atomic::AtomicU64;
    use std::sync::mpsc::Receiver;
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::probe::fake::{FakeProbe, Step};
    use crate::transport::UsbIds;

    pub(crate) const C6_MAC: &str = "e8:f6:0a:f7:ff:d4";

    pub(crate) fn usb(name: &str, vid: u16, pid: u16, serial: &str) -> PortDesc {
        PortDesc {
            name: name.to_owned(),
            usb: Some(UsbIds {
                vid,
                pid,
                serial: Some(serial.to_owned()),
                manufacturer: None,
                product: None,
                interface: None,
            }),
        }
    }

    pub(crate) fn native(name: &str, serial: &str) -> PortDesc {
        let mut port = usb(name, 0x303A, 0x1001, serial);
        if let Some(usb) = port.usb.as_mut() {
            usb.interface = Some(0);
        }
        port
    }

    pub(crate) fn ch343(name: &str) -> PortDesc {
        usb(name, 0x1A86, 0x55D3, "5D28088406")
    }

    pub(crate) fn c6_board() -> Board {
        let chip = Chip::from_target("esp32c6");
        Board::new(
            Mac::parse(C6_MAC).unwrap(),
            chip,
            vec![native("COM11", "E8:F6:0A:F7:FF:D4"), ch343("COM13")],
        )
        .unwrap()
    }

    /// The OS list during a C6 run: present, gone after the bridge's reset, back.
    pub(crate) fn c6_enumerate() -> impl FnMut() -> Vec<PortDesc> + Send + 'static {
        let mut lists = vec![
            vec![ch343("COM13")],
            vec![ch343("COM13"), native("COM11", "E8:F6:0A:F7:FF:D4")],
        ];
        move || {
            if lists.len() > 1 {
                lists.remove(0)
            } else {
                lists.first().cloned().unwrap_or_default()
            }
        }
    }

    /// Time stands still until someone sleeps; sleeping advances it exactly. Thread-safe.
    #[derive(Default)]
    pub(crate) struct TestTime {
        now: AtomicU64,
    }

    impl Clock for TestTime {
        fn now_ms(&self) -> u64 {
            self.now.load(Ordering::SeqCst)
        }
    }

    impl Sleeper for TestTime {
        fn sleep(&self, duration: Duration) {
            let ms = u64::try_from(duration.as_millis()).unwrap_or(u64::MAX);
            self.now.fetch_add(ms, Ordering::SeqCst);
        }
    }

    /// Calls made through the bench, with the name of the port each probe was made for.
    pub(crate) type Log = Arc<Mutex<Vec<(String, Step)>>>;

    /// Holds the probe before `step` until the test releases it.
    pub(crate) struct Gate {
        step: Step,
        reached: Mutex<Sender<Step>>,
        release: Mutex<Receiver<()>>,
    }

    /// The test's side of a [`Gate`].
    pub(crate) struct GateHandle {
        /// Receives the step once the probe is held at it.
        pub(crate) reached: Receiver<Step>,
        /// Send `()` to let the probe go on.
        pub(crate) release: Sender<()>,
    }

    pub(crate) fn gate(step: Step) -> (Arc<Gate>, GateHandle) {
        let (reached_tx, reached) = mpsc::channel();
        let (release, release_rx) = mpsc::channel();
        let gate = Gate {
            step,
            reached: Mutex::new(reached_tx),
            release: Mutex::new(release_rx),
        };
        (Arc::new(gate), GateHandle { reached, release })
    }

    struct Bench {
        name: String,
        inner: FakeProbe,
        log: Log,
        gate: Option<Arc<Gate>>,
    }

    impl Bench {
        fn note(&self, step: Step) {
            self.log.lock().unwrap().push((self.name.clone(), step));
            if let Some(gate) = &self.gate
                && gate.step == step
            {
                let _ = gate.reached.lock().unwrap().send(step);
                // Bounded: a test that forgets to release fails instead of hanging.
                let _ = gate
                    .release
                    .lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(10));
            }
        }
    }

    impl Probe for Bench {
        fn connect(&mut self, port: &PortDesc) -> Result<(), ProbeError> {
            self.note(Step::Connect);
            self.inner.connect(port)
        }
        fn chip(&mut self) -> Result<Chip, ProbeError> {
            self.note(Step::Chip);
            self.inner.chip()
        }
        fn identity(&mut self) -> Result<ChipIdentity, ProbeError> {
            self.note(Step::Identity);
            self.inner.identity()
        }
        fn revision(&mut self) -> Result<Revision, ProbeError> {
            self.note(Step::Revision);
            self.inner.revision()
        }
        fn crystal_mhz(&mut self) -> Result<u32, ProbeError> {
            self.note(Step::CrystalMhz);
            self.inner.crystal_mhz()
        }
        fn mac(&mut self) -> Result<Mac, ProbeError> {
            self.note(Step::Mac);
            self.inner.mac()
        }
        fn read_reg(&mut self, addr: u32) -> Result<u32, ProbeError> {
            self.note(Step::ReadReg);
            self.inner.read_reg(addr)
        }
        fn package_version(&mut self) -> Result<u32, ProbeError> {
            self.note(Step::PackageVersion);
            self.inner.package_version()
        }
        fn embedded_flash(&mut self) -> Result<Option<EmbeddedMem>, ProbeError> {
            self.note(Step::EmbeddedFlash);
            self.inner.embedded_flash()
        }
        fn embedded_psram(&mut self) -> Result<Option<EmbeddedMem>, ProbeError> {
            self.note(Step::EmbeddedPsram);
            self.inner.embedded_psram()
        }
        fn flash_id(&mut self) -> Result<FlashId, ProbeError> {
            self.note(Step::FlashId);
            self.inner.flash_id()
        }
        fn hard_reset(&mut self) -> Result<(), ProbeError> {
            self.note(Step::HardReset);
            self.inner.hard_reset()
        }
    }

    /// A probe factory where every port answers as the bench C6, logging to `log` and held by
    /// `gate`, if any.
    pub(crate) fn c6_probes(
        log: &Log,
        gate: Option<Arc<Gate>>,
    ) -> impl FnMut(&PortDesc) -> Box<dyn Probe> + Send + 'static {
        let log = Arc::clone(log);
        let inner = FakeProbe::from_fixture("esp32c6").unwrap();
        move |port: &PortDesc| -> Box<dyn Probe> {
            Box::new(Bench {
                name: port.name.clone(),
                inner: inner.clone(),
                log: Arc::clone(&log),
                gate: gate.clone(),
            })
        }
    }

    /// Receives events until `done` says so, failing after a few seconds instead of hanging.
    pub(crate) fn collect_until(
        events: &Receiver<Event>,
        done: impl Fn(&Event) -> bool,
    ) -> Vec<Event> {
        let mut seen = Vec::new();
        loop {
            match events.recv_timeout(Duration::from_secs(10)) {
                Ok(event) => {
                    let last = done(&event);
                    seen.push(event);
                    if last {
                        return seen;
                    }
                }
                Err(error) => panic!("no final event ({error}); got {seen:?}"),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::*;
    use super::*;
    use crate::probe::fake::Step;
    use crate::report::State;
    use crate::runner::{BOARD_TESTS, UART_BRIDGE, USB_SERIAL_JTAG};

    fn is_end(event: &Event) -> bool {
        matches!(event, Event::RunFinished(_) | Event::RunCancelled)
    }

    #[test]
    fn events_arrive_in_order() {
        let log = Log::default();
        let (tx, rx) = mpsc::channel();
        let worker = Worker::spawn(
            c6_probes(&log, None),
            TestTime::default(),
            c6_enumerate(),
            tx,
        )
        .unwrap();
        assert!(worker.submit(Job::Run(c6_board())));
        let events = collect_until(&rx, is_end);
        assert!(worker.shutdown(Duration::from_secs(5)));

        // Execution order: the bridge's port test, the board suites in catalog order, then the
        // native port (after the wait); every test exactly once, then the report.
        let mut expected = vec![
            Event::TestStarted(UART_BRIDGE),
            Event::TestFinished(finished_of(&events, UART_BRIDGE)),
        ];
        for id in BOARD_TESTS {
            expected.push(Event::TestStarted(id));
            expected.push(Event::TestFinished(finished_of(&events, id)));
        }
        expected.push(Event::TestStarted(USB_SERIAL_JTAG));
        expected.push(Event::Waiting {
            port: native("COM11", "E8:F6:0A:F7:FF:D4"),
        });
        expected.push(Event::TestFinished(finished_of(&events, USB_SERIAL_JTAG)));
        let Some(Event::RunFinished(report)) = events.last() else {
            panic!("no report: {events:?}");
        };
        assert_eq!(events[..events.len() - 1], expected[..]);

        // What was announced is what the report says.
        for test in report.tests() {
            assert_eq!(finished_of(&events, test.id), *test);
        }
        assert_eq!(
            report.summary().to_string(),
            "Tests: 0 failed, 11 passed, 0 skipped, 11 total"
        );
        assert!(
            report
                .tests()
                .iter()
                .all(|test| test.outcome.state() == State::Pass)
        );
    }

    fn finished_of(events: &[Event], id: TestId) -> TestResult {
        events
            .iter()
            .find_map(|event| match event {
                Event::TestFinished(test) if test.id == id => Some(test.clone()),
                _ => None,
            })
            .unwrap_or_else(|| panic!("{id} never finished: {events:?}"))
    }

    #[test]
    fn cancel_mid_run_resets_and_reports_cancelled() {
        let log = Log::default();
        let (held, gate) = gate(Step::Revision);
        let (tx, rx) = mpsc::channel();
        let worker = Worker::spawn(
            c6_probes(&log, Some(held)),
            TestTime::default(),
            c6_enumerate(),
            tx,
        )
        .unwrap();
        assert!(worker.submit(Job::Run(c6_board())));
        assert_eq!(
            gate.reached.recv_timeout(Duration::from_secs(10)),
            Ok(Step::Revision)
        );
        worker.cancel();
        gate.release.send(()).unwrap();
        let events = collect_until(&rx, is_end);

        assert_eq!(events.last(), Some(&Event::RunCancelled));
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, Event::RunFinished(_)))
        );
        // Nothing after the cancel reaches the UI but the end of the run.
        let after = events
            .iter()
            .position(|event| *event == Event::TestStarted(BOARD_TESTS[1]))
            .unwrap();
        assert_eq!(events[after + 1..], [Event::RunCancelled]);

        // The bridge session got its hard reset; no read after the cancelled one; the native
        // port was never opened.
        let calls = log.lock().unwrap().clone();
        assert_eq!(
            calls.iter().map(|(_, step)| *step).collect::<Vec<_>>(),
            [
                Step::Connect,
                Step::Mac,
                Step::Chip,
                Step::Identity,
                Step::Revision,
                Step::HardReset
            ]
        );
        assert!(calls.iter().all(|(name, _)| name == "COM13"));

        // The worker takes the next job with the flag cleared (the gate is opened in advance).
        gate.release.send(()).unwrap();
        assert!(worker.submit(Job::Run(c6_board())));
        let events = collect_until(&rx, is_end);
        assert!(matches!(events.last(), Some(Event::RunFinished(_))));
        assert!(worker.shutdown(Duration::from_secs(5)));
    }

    #[test]
    fn identify_sends_one_event_per_port_then_finishes() {
        let log = Log::default();
        let (tx, rx) = mpsc::channel();
        let worker =
            Worker::spawn(c6_probes(&log, None), TestTime::default(), Vec::new, tx).unwrap();
        assert!(worker.submit(Job::Identify(vec![ch343("COM13")])));
        let events = collect_until(&rx, |event| *event == Event::IdentifyFinished);
        assert!(worker.shutdown(Duration::from_secs(5)));
        assert_eq!(events.len(), 2);
        let Event::Identified(Identified::Found { port, mac, .. }) = &events[0] else {
            panic!("{events:?}");
        };
        assert_eq!(port.name, "COM13");
        assert_eq!(mac.to_string(), C6_MAC);
        assert_eq!(
            log.lock().unwrap().last().map(|(_, step)| *step),
            Some(Step::HardReset)
        );
    }

    #[test]
    fn shutdown_while_idle_is_immediate() {
        let (tx, _rx) = mpsc::channel();
        let worker = Worker::spawn(
            c6_probes(&Log::default(), None),
            TestTime::default(),
            Vec::new,
            tx,
        )
        .unwrap();
        let start = Instant::now();
        assert!(worker.shutdown(Duration::from_secs(5)));
        assert!(start.elapsed() < Duration::from_secs(2));
    }
}
