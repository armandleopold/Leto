//! This PC, shared: Leto as the host, in the same window as the client.
//!
//! For now it is the companion of `docs/return-channel.md`: beside an
//! Apollo or a Sunshine that streams the screen as before, Leto takes the
//! camera and the microphone of the devices they are paired with. See the
//! `leto-host` crate.
//!
//! While this PC is shared, closing the window puts it away and Leto goes
//! on beside the clock, on Windows, where an icon brings it back.

use std::{
    collections::HashMap,
    net::{IpAddr, UdpSocket},
    time::{Duration, Instant},
};

use egui::{Align, Color32, Layout, RichText, Stroke, TextureHandle, TextureOptions, Ui, vec2};
use leto_core::model::{HostId, HostSummary};
use leto_host::{
    Companion, Options,
    apollo::{Apollo, Device, Error},
};

use crate::{
    theme,
    ui::{pcs::THIS_PC, widgets},
};

/// How often the screen looks at what changed while nobody touches it.
const GLANCE: Duration = Duration::from_millis(250);

/// How often it looks for a new picture while a camera is being sent:
/// more often than a camera sends them.
const PICTURE: Duration = Duration::from_millis(16);

/// How often Apollo's list of devices is read again.
const DEVICES_EVERY: Duration = Duration::from_secs(2);

/// What Leto is doing for the devices that reach this PC.
enum State {
    /// Nothing: this PC is not shared.
    Off,
    /// Beside an Apollo, taking cameras, and handing them to the apps of
    /// this PC.
    Beside(Companion, apps::Handing, voice::Voicing),
    /// Asked to, and could not: why, in words for the person, and whether
    /// it is the system that keeps Apollo's settings from Leto.
    Stopped(String, bool),
}

/// This PC as a host, for as long as the app runs.
pub struct Sharing {
    state: State,
    devices: Vec<Device>,
    devices_read: Instant,
    /// What the camera that is being sent sees, and how many pictures had
    /// arrived when it was last drawn.
    camera: Option<(TextureHandle, u64)>,
    /// The person asked for this PC to be shared, running or not.
    wanted: bool,
    /// Whether an address of the list of PCs is one of this very PC, for
    /// each one asked about.
    own: HashMap<String, bool>,
    /// The icon beside the clock, for as long as Leto runs.
    #[cfg(windows)]
    tray: Option<tray_icon::TrayIcon>,
}

impl Default for Sharing {
    fn default() -> Self {
        Self {
            state: State::Off,
            devices: Vec::new(),
            devices_read: Instant::now(),
            camera: None,
            wanted: false,
            own: HashMap::new(),
            #[cfg(windows)]
            tray: None,
        }
    }
}

impl Sharing {
    /// Shares this PC or stops, as the setting says: called every frame,
    /// and does something when the setting changed. `window` is the handle
    /// of Leto's window, for the icon beside the clock to bring it back.
    pub fn follow(&mut self, wanted: bool, window: Option<isize>, ctx: &egui::Context) {
        self.wanted = wanted;
        match (&self.state, wanted) {
            (State::Off, true) => self.start(),
            (State::Beside(..) | State::Stopped(..), false) => {
                self.state = State::Off;
                self.camera = None;
            }
            _ => {}
        }
        #[cfg(windows)]
        if self.tray.is_none() {
            self.tray = window.and_then(|window| tray::start(window, ctx));
        }
        #[cfg(not(windows))]
        let _ = (window, ctx);
    }

    /// Whether Leto goes on once its window is closed: while the person
    /// wants this PC shared and an icon beside the clock can bring the
    /// window back.
    pub fn stays(&self) -> bool {
        #[cfg(windows)]
        return self.wanted && self.tray.is_some();
        #[cfg(not(windows))]
        false
    }

    /// This PC is shared at this moment.
    pub const fn shared(&self) -> bool {
        matches!(self.state, State::Beside(..))
    }

    /// The PCs of the list that are this very one: those at an address of
    /// this computer.
    pub fn own(&mut self, hosts: &[HostSummary]) -> Vec<HostId> {
        hosts
            .iter()
            .filter(|host| {
                *self.own.entry(host.address.clone()).or_insert_with(|| is_here(&host.address))
            })
            .map(|host| host.id.clone())
            .collect()
    }

    /// Reads what Apollo keeps and starts beside it.
    fn start(&mut self) {
        self.state = match Apollo::find() {
            Ok(apollo) => match Companion::start(apollo, &Options::default()) {
                Ok(companion) => {
                    let handing = apps::Handing::start(companion.watch());
                    let voicing = voice::Voicing::start(companion.watch());
                    State::Beside(companion, handing, voicing)
                }
                Err(error) => State::Stopped(
                    format!(
                        "The port for the camera could not be opened: {error}. Leto may already \
                         be running on this PC: look beside the clock."
                    ),
                    false,
                ),
            },
            Err(error) => {
                let kept = matches!(&error, Error::Unreadable(_, error)
                    if error.kind() == std::io::ErrorKind::PermissionDenied);
                State::Stopped(format!("{error}."), kept)
            }
        };
        if let State::Stopped(why, _) = &self.state {
            log::warn!("this PC is not shared: {why}");
        }
        self.read_devices();
    }

    fn read_devices(&mut self) {
        self.devices_read = Instant::now();
        if let State::Beside(companion, ..) = &self.state {
            // A file Apollo is writing at this moment keeps the last list.
            if let Ok(devices) = companion.apollo().devices() {
                self.devices = devices;
            }
        }
    }

    /// The screen. `share` is the setting. Returns whether the person asked
    /// to go back to `back_to`.
    pub fn show(&mut self, ui: &mut Ui, share: &mut bool, back_to: &str) -> bool {
        if self.devices_read.elapsed() >= DEVICES_EVERY {
            self.read_devices();
        }
        let back = widgets::back_button(ui, back_to).clicked();
        widgets::title(ui, THIS_PC);
        ui.add_space(8.0);

        card(ui, "Share this PC", |ui| {
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.set_width((ui.available_width() - 64.0).max(120.0));
                    ui.add(
                        egui::Label::new(widgets::dim(
                            "Your devices reach this PC through Leto. For now Leto works \
                             beside Apollo or Sunshine, which stream the screen, and takes \
                             what they do not: the camera and the microphone of your \
                             devices.",
                        ))
                        .wrap(),
                    );
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    widgets::switch(ui, share, "Share this PC");
                });
            });
        });

        let mut again = false;
        match &self.state {
            State::Off => {}
            State::Stopped(why, kept) => card(ui, "Not running", |ui| {
                ui.add(egui::Label::new(RichText::new(why).color(theme::problem())).wrap());
                // Windows keeps what is under `Program Files` from ordinary
                // programs: only an administrator reads Apollo's key.
                let elevates = *kept && cfg!(windows);
                let advice = if elevates {
                    "Windows only lets an administrator read the settings of Apollo. Leto can \
                     start again as one: Windows asks you first."
                } else if *kept {
                    "The system does not let Leto read these settings."
                } else {
                    "Apollo or Sunshine has to be installed on this PC."
                };
                ui.add(egui::Label::new(widgets::dim(advice)).wrap());
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if elevates {
                        if widgets::primary_button(ui, "Restart as administrator").clicked()
                            && leto_system::elevate::restart_as_administrator()
                        {
                            // The other Leto shares from here on: the choice
                            // is kept already.
                            std::process::exit(0);
                        }
                        again = widgets::quiet_button(ui, "Try again").clicked();
                    } else {
                        again = widgets::primary_button(ui, "Try again").clicked();
                    }
                });
            }),
            State::Beside(companion, handing, voicing) => {
                card(ui, "Running", |ui| {
                    ui.horizontal(|ui| {
                        dot(ui, theme::ready());
                        ui.label(format!("Beside Apollo, on port {}", companion.port()));
                    });
                    ui.add(
                        egui::Label::new(widgets::dim(format!(
                            "Devices are the ones Apollo is paired with. Its settings are read \
                             from {} and never changed.",
                            companion.apollo().folder().display()
                        )))
                        .wrap(),
                    );
                });
                let sender = companion.sender();
                let speaker = companion.speaker();
                devices(ui, &self.devices, sender.as_deref(), speaker.as_deref());
                camera(ui, companion, sender.as_deref(), handing.given(), &mut self.camera);
                microphone(ui, companion, voicing);
                if cfg!(windows) {
                    ui.add(
                        egui::Label::new(
                            widgets::dim(
                                "Closing the window keeps Leto running while this PC is \
                                 shared. Its icon beside the clock brings the window back, \
                                 and quits Leto.",
                            )
                            .size(theme::SMALL),
                        )
                        .wrap(),
                    );
                }
                // A camera that is being sent moves: its pictures are
                // shown as they come, not four times a second.
                // So does the meter of a microphone.
                let cameras = companion.cameras();
                let moving = cameras.report().open || cameras.voice_report().open;
                let pace = if moving { PICTURE } else { GLANCE };
                ui.ctx().request_repaint_after(pace);
            }
        }
        if again {
            self.start();
        }
        back
    }
}

/// Whether `address` is one of this computer: the system only lets a
/// socket be opened on an address it has. A name is not looked up, which
/// could take seconds: the search finds PCs by their numbers. Not the
/// loopback addresses either, which the search never finds: whoever adds
/// one by hand is trying Leto against a stand-in host on the same computer.
fn is_here(address: &str) -> bool {
    address
        .parse::<IpAddr>()
        .is_ok_and(|address| !address.is_loopback() && UdpSocket::bind((address, 0)).is_ok())
}

/// The devices Apollo is paired with, and which may send a camera and a
/// microphone.
fn devices(ui: &mut Ui, devices: &[Device], sender: Option<&str>, speaker: Option<&str>) {
    card(ui, "Devices", |ui| {
        if devices.is_empty() {
            ui.label(widgets::dim("Apollo is paired with no device yet. Pair one from Leto."));
        }
        for device in devices {
            let name = if device.name.is_empty() { "A device" } else { &device.name };
            ui.horizontal(|ui| {
                ui.label(name);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let name = Some(device.name.as_str());
                    let sending = match (sender == name, speaker == name) {
                        (true, true) => Some("Sending its camera and its microphone"),
                        (true, false) => Some("Sending its camera"),
                        (false, true) => Some("Sending its microphone"),
                        (false, false) => None,
                    };
                    if let Some(sending) = sending {
                        ui.label(RichText::new(sending).color(theme::sun()));
                    } else if device.trusted() {
                        ui.label(widgets::dim("May send its camera and its microphone"));
                    } else {
                        ui.label(widgets::dim("No camera, no microphone")).on_hover_text(
                            "Apollo does not let this device use the keyboard and the mouse.",
                        );
                    }
                });
            });
        }
        if devices.iter().any(|device| !device.trusted()) {
            ui.add_space(2.0);
            ui.add(
                egui::Label::new(
                    widgets::dim(
                        "A device may send its camera and its microphone once Apollo lets it \
                         use the keyboard and the mouse: tick both for it on Apollo's page, \
                         under PIN.",
                    )
                    .size(theme::SMALL),
                )
                .wrap(),
            );
        }
    });
}

/// What the camera that is being sent sees.
fn camera(
    ui: &mut Ui,
    companion: &Companion,
    sender: Option<&str>,
    given: apps::Given,
    shown: &mut Option<(TextureHandle, u64)>,
) {
    card(ui, "Camera", |ui| {
        let report = companion.cameras().report();
        if !report.open {
            *shown = None;
            ui.add(
                egui::Label::new(widgets::dim(
                    "No device is sending its camera. In Leto, on the device, turn the camera \
                     on during a stream.",
                ))
                .wrap(),
            );
            return;
        }
        let from = sender.filter(|name| !name.is_empty()).unwrap_or("a device");
        match report.size {
            Some((width, height)) => ui.label(format!("From {from}, {width} by {height}")),
            None => ui.label(format!("From {from}: waiting for its first picture")),
        };
        // Drawn again only when a picture arrived since.
        if shown.as_ref().is_none_or(|(_, pictures)| *pictures != report.pictures)
            && let Some(seen) = companion.cameras().seen()
        {
            let image =
                egui::ColorImage::from_rgba_unmultiplied([seen.width, seen.height], &seen.rgba);
            match shown {
                Some((texture, pictures)) => {
                    texture.set(image, TextureOptions::LINEAR);
                    *pictures = report.pictures;
                }
                None => {
                    let texture = ui.ctx().load_texture("camera", image, TextureOptions::LINEAR);
                    *shown = Some((texture, report.pictures));
                }
            }
        }
        if let Some((texture, _)) = shown {
            let size = texture.size_vec2();
            // Small enough to leave the rest of the screen in sight.
            let width = ui.available_width().min(size.x).min(420.0);
            let image = egui::Image::new(&*texture)
                .fit_to_exact_size(vec2(width, width * size.y / size.x))
                .corner_radius(theme::CONTROL_RADIUS);
            ui.add(image);
        }
        if report.lost > 0 {
            ui.label(
                widgets::dim(format!(
                    "{} pictures arrived, {} were lost",
                    report.pictures, report.lost
                ))
                .size(theme::SMALL),
            );
        }
        let said = match given {
            apps::Given::Yes => {
                "The apps of this PC see it as \"OBS Virtual Camera\", the camera that comes \
                 with OBS Studio: choose that one in Teams."
            }
            apps::Given::Taken => {
                "The apps of this PC do not see it: OBS is using its virtual camera itself. \
                 Stop the virtual camera in OBS and Leto takes it."
            }
            apps::Given::No => "The apps of this PC do not see it as a camera here.",
        };
        ui.add(egui::Label::new(widgets::dim(said).size(theme::SMALL)).wrap());
    });
}

/// The microphone that is being sent, and where the apps of this PC hear
/// it.
fn microphone(ui: &mut Ui, companion: &Companion, voicing: &voice::Voicing) {
    card(ui, "Microphone", |ui| {
        let Some(heard_as) = voicing.heard_as() else {
            ui.add(
                egui::Label::new(widgets::dim(
                    "Not taken yet: Windows needs a virtual cable for the apps of this PC to \
                     hear a device as a microphone. Install VB-CABLE, which is free, and Leto \
                     finds it by itself.",
                ))
                .wrap(),
            );
            ui.hyperlink_to("vb-audio.com/Cable", "https://vb-audio.com/Cable/");
            return;
        };
        let report = companion.cameras().voice_report();
        let talking = report.heard.is_some_and(|heard| heard.elapsed() < Duration::from_secs(1));
        if report.open && talking {
            let from = companion.speaker().filter(|name| !name.is_empty());
            ui.horizontal(|ui| {
                dot(ui, theme::sun());
                ui.label(format!("From {}", from.as_deref().unwrap_or("a device")));
                ui.add(
                    egui::ProgressBar::new(voicing.level())
                        .desired_width(120.0)
                        .desired_height(6.0),
                );
            });
        } else {
            ui.add(
                egui::Label::new(widgets::dim(
                    "No device is sending its microphone. In Leto, on the device, turn the \
                     microphone on during a stream.",
                ))
                .wrap(),
            );
        }
        ui.add(
            egui::Label::new(
                widgets::dim(format!(
                    "The apps of this PC hear it as \"{}\": choose that microphone in Teams.",
                    heard_as
                ))
                .size(theme::SMALL),
            )
            .wrap(),
        );
    });
}

/// The microphone that is sent, played where the apps of this PC hear a
/// microphone.
mod voice {
    use std::{
        sync::{
            Arc, Mutex, PoisonError,
            atomic::{AtomicBool, AtomicU32, Ordering},
            mpsc::{Receiver, RecvTimeoutError},
        },
        thread,
        time::{Duration, Instant},
    };

    use leto_host::camera::Cameras;
    use leto_media::cable::{self, Cable, Found};

    /// The name of a sound output to play the microphone on, for a cable
    /// Leto does not know and for trying this out where there is none.
    const CABLE: &str = "LETO_CABLE";

    /// How long without a frame before the meter falls back to nothing.
    const QUIET: Duration = Duration::from_millis(200);

    /// How often a virtual cable is looked for while this PC has none, or
    /// lost the one it had: the person may be installing one.
    const SEEK_EVERY: Duration = Duration::from_secs(5);

    /// What the thread tells whoever draws.
    #[derive(Default)]
    struct Told {
        /// The cable the microphone plays on, while there is one.
        cable: Mutex<Option<Found>>,
        /// How loud the last frame was, as the bits of an `f32`.
        level: AtomicU32,
        stop: AtomicBool,
    }

    /// A thread that plays what arrives of a microphone into a virtual
    /// cable, for as long as this lives. The PC takes a microphone while
    /// it has such a cable.
    pub struct Voicing {
        told: Arc<Told>,
    }

    impl Voicing {
        pub fn start(cameras: Arc<Cameras>) -> Self {
            let told = Arc::new(Told::default());
            let spawned = thread::Builder::new().name("microphone to apps".to_owned()).spawn({
                let told = Arc::clone(&told);
                move || run(&cameras, &told)
            });
            if let Err(error) = spawned {
                log::warn!("no thread for the microphone: {error}");
            }
            Self { told }
        }

        /// The name of the microphone the apps of this PC pick, while this
        /// PC has a cable.
        pub fn heard_as(&self) -> Option<String> {
            let cable = self.told.cable.lock().unwrap_or_else(PoisonError::into_inner);
            cable.as_ref().map(|cable| cable.microphone.clone())
        }

        /// How loud the microphone is at this moment, from 0 to 1.
        pub fn level(&self) -> f32 {
            f32::from_bits(self.told.level.load(Ordering::Relaxed))
        }
    }

    impl Drop for Voicing {
        fn drop(&mut self) {
            self.told.stop.store(true, Ordering::SeqCst);
        }
    }

    fn run(cameras: &Cameras, told: &Told) {
        let stopped = || told.stop.load(Ordering::SeqCst);
        let tell = |cable: Option<Found>| {
            *told.cable.lock().unwrap_or_else(PoisonError::into_inner) = cable;
        };
        while !stopped() {
            let found = match std::env::var(CABLE) {
                Ok(output) => cable::named(&output),
                Err(_) => cable::find(),
            };
            // Opened here: on some systems a sound output stays on the
            // thread that opened it.
            let opened = found.and_then(|found| match Cable::open(&found.output) {
                Ok(cable) => Some((found, cable)),
                Err(error) => {
                    log::warn!("the cable for the microphone did not open: {error}");
                    None
                }
            });
            if let Some((found, mut cable)) = opened {
                log::info!("a device's microphone plays on \"{}\"", found.output);
                tell(Some(found));
                // From here on the PC takes a microphone, until `frames`
                // goes.
                let frames = cameras.hear();
                play(&frames, &mut cable, told);
                tell(None);
                told.level.store(0, Ordering::Relaxed);
            }
            let sought = Instant::now();
            while !stopped() && sought.elapsed() < SEEK_EVERY {
                thread::sleep(QUIET);
            }
        }
    }

    /// Plays what arrives until the cable is gone or nobody wants it.
    fn play(frames: &Receiver<Vec<u8>>, cable: &mut Cable, told: &Told) {
        while !told.stop.load(Ordering::SeqCst) {
            let loud = match frames.recv_timeout(QUIET) {
                Ok(frame) => match cable.play(&frame) {
                    Some(loud) => loud,
                    None => {
                        log::warn!("the cable for the microphone is gone");
                        return;
                    }
                },
                Err(RecvTimeoutError::Timeout) => 0.0,
                Err(RecvTimeoutError::Disconnected) => return,
            };
            told.level.store(loud.to_bits(), Ordering::Relaxed);
        }
    }
}

/// The camera that is sent, handed to the apps of this PC.
mod apps {
    use std::{
        sync::{
            Arc, Mutex, PoisonError,
            atomic::{AtomicBool, Ordering},
        },
        thread,
        time::{Duration, Instant},
    };

    use leto_host::camera::{Cameras, Seen};
    use leto_system::vcam::{Feed, Refused};

    /// How often a new picture is looked for: far more often than a
    /// camera sends them, and next to no work when there is none.
    const LOOK: Duration = Duration::from_millis(4);

    /// How often the virtual camera is asked for again once it was refused.
    const ASK_AGAIN: Duration = Duration::from_secs(2);

    /// Whether the apps of this PC see the camera that is sent.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Given {
        /// As the virtual camera of OBS.
        Yes,
        /// OBS writes to its virtual camera itself.
        Taken,
        /// Not on this system, or Windows refused.
        No,
    }

    /// The thread that hands the pictures over, for as long as this lives.
    pub struct Handing {
        stop: Arc<AtomicBool>,
        given: Arc<Mutex<Given>>,
    }

    impl Handing {
        /// Starts handing what arrives in `cameras` to the virtual camera.
        /// On its own thread: the window draws nothing while it is put
        /// away, and the apps want their pictures all the same.
        pub fn start(cameras: Arc<Cameras>) -> Self {
            let stop = Arc::new(AtomicBool::new(false));
            let given = Arc::new(Mutex::new(Given::No));
            let handing = Self { stop: Arc::clone(&stop), given: Arc::clone(&given) };
            let spawned = thread::Builder::new()
                .name("camera to apps".to_owned())
                .spawn(move || hand(&cameras, &stop, &given));
            if let Err(error) = spawned {
                log::warn!("the camera is not handed to the apps of this PC: {error}");
            }
            handing
        }

        pub fn given(&self) -> Given {
            *self.given.lock().unwrap_or_else(PoisonError::into_inner)
        }
    }

    impl Drop for Handing {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Relaxed);
        }
    }

    fn hand(cameras: &Cameras, stop: &AtomicBool, given: &Mutex<Given>) {
        let started = Instant::now();
        let mut feed: Option<Feed> = None;
        let mut refused: Option<Instant> = None;
        let mut handed = 0;
        let say = |now: Given| {
            let mut given = given.lock().unwrap_or_else(PoisonError::into_inner);
            if *given != now {
                log::info!("the apps of this PC see the camera that is sent: {now:?}");
                *given = now;
            }
        };
        while !stop.load(Ordering::Relaxed) {
            thread::sleep(LOOK);
            let report = cameras.report();
            if !report.open {
                // The virtual camera shows its own card again.
                feed = None;
                continue;
            }
            if report.pictures == handed {
                continue;
            }
            let Some(seen) = cameras.seen() else { continue };
            handed = report.pictures;
            // Even on both sides, as NV12 needs.
            let (width, height) = (seen.width & !1, seen.height & !1);
            let size = (width as u32, height as u32);
            if feed.as_ref().is_some_and(|feed| feed.size() != size) {
                feed = None;
            }
            if feed.is_none() && refused.is_none_or(|at| at.elapsed() >= ASK_AGAIN) {
                let pace = report.asked.map_or(30, |(_, _, pace)| pace.max(1));
                match Feed::open(size.0, size.1, 10_000_000 / u64::from(pace)) {
                    Ok(opened) => {
                        feed = Some(opened);
                        refused = None;
                        say(Given::Yes);
                    }
                    Err(why) => {
                        refused = Some(Instant::now());
                        say(if why == Refused::Taken { Given::Taken } else { Given::No });
                    }
                }
            }
            if let Some(feed) = &mut feed {
                let (luma, chroma) = nv12(&seen, width, height);
                let time = u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX);
                feed.write(&luma, &chroma, time);
            }
        }
    }

    /// The top left `width` by `height` of a picture, both even, as NV12:
    /// how bright each pixel is, then the colour of every four, in the
    /// narrow range video uses. High definition has its own weights.
    pub fn nv12(seen: &Seen, width: usize, height: usize) -> (Vec<u8>, Vec<u8>) {
        // Weights of red, green and blue, in 1/65536: BT.709 from 720
        // lines up, BT.601 under. Narrow range: 16 to 235 and 16 to 240.
        let ([yr, yg, yb], [ur, ug, ub], [vr, vg, vb]) = if height >= 720 {
            ([11_966, 40_254, 4_064], [-6_596, -22_189, 28_784], [28_784, -26_145, -2_639])
        } else {
            ([16_829, 33_039, 6_416], [-9_714, -19_070, 28_784], [28_784, -24_103, -4_681])
        };
        let pixel = |x: usize, y: usize| {
            let at = (y * seen.width + x) * 4;
            let rgb = &seen.rgba[at..at + 3];
            [i32::from(rgb[0]), i32::from(rgb[1]), i32::from(rgb[2])]
        };
        // Within 0 to 255 once clamped.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let byte = |value: i32| value.clamp(0, 255) as u8;
        let mut luma = Vec::with_capacity(width * height);
        for y in 0..height {
            for x in 0..width {
                let [r, g, b] = pixel(x, y);
                luma.push(byte(16 + ((yr * r + yg * g + yb * b + 32_768) >> 16)));
            }
        }
        let mut chroma = Vec::with_capacity(width * height / 2);
        for y in (0..height).step_by(2) {
            for x in (0..width).step_by(2) {
                // The four pixels that share a colour, added up.
                let mut sum = [0; 3];
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let [r, g, b] = pixel(x + dx, y + dy);
                    sum = [sum[0] + r, sum[1] + g, sum[2] + b];
                }
                let [r, g, b] = sum;
                chroma.push(byte(128 + ((ur * r + ug * g + ub * b + 131_072) >> 18)));
                chroma.push(byte(128 + ((vr * r + vg * g + vb * b + 131_072) >> 18)));
            }
        }
        (luma, chroma)
    }
}

/// A group of things, under its name.
fn card(ui: &mut Ui, name: &str, add: impl FnOnce(&mut Ui)) {
    egui::Frame::new()
        .fill(theme::surface())
        .stroke(Stroke::new(1.0, theme::line()))
        .corner_radius(theme::CARD_RADIUS)
        .inner_margin(egui::Margin::same(18))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(name).size(18.0));
            ui.add_space(2.0);
            add(ui);
        });
    ui.add_space(12.0);
}

/// The dot that says something is going well.
fn dot(ui: &mut Ui, colour: Color32) {
    let (place, _) = ui.allocate_exact_size(vec2(10.0, 10.0), egui::Sense::hover());
    ui.painter().circle_filled(place.center(), 5.0, colour);
}

/// The icon beside the clock, on Windows.
#[cfg(windows)]
mod tray {
    use tray_icon::{
        Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent,
        menu::{Menu, MenuEvent, MenuItem},
    };

    /// Puts Leto's icon beside the clock. A click on it, or "Open Leto" in
    /// its menu, brings back the window with this handle; "Quit" ends Leto.
    ///
    /// The icon lives as long as what is returned. It has to be made by the
    /// thread that runs the window, which is where Windows sends its clicks.
    pub fn start(window: isize, ctx: &egui::Context) -> Option<TrayIcon> {
        let icon =
            image::load_from_memory(include_bytes!("../../../assets/brand/png/leto-icon-32.png"))
                .ok()?
                .into_rgba8();
        let (width, height) = icon.dimensions();
        let icon = Icon::from_rgba(icon.into_raw(), width, height).ok()?;

        let open = MenuItem::new("Open Leto", true, None);
        let quit = MenuItem::new("Quit", true, None);
        let menu = Menu::new();
        menu.append_items(&[&open, &quit]).ok()?;
        let tray = TrayIconBuilder::new()
            .with_icon(icon)
            .with_tooltip("Leto")
            .with_menu(Box::new(menu))
            // A click opens the window, and the other button the menu.
            .with_menu_on_left_click(false)
            .build()
            .ok()?;

        let bring_back = {
            let ctx = ctx.clone();
            move || {
                leto_system::shown::show(window);
                ctx.request_repaint();
            }
        };
        let (open, quit) = (open.id().clone(), quit.id().clone());
        MenuEvent::set_event_handler(Some({
            let bring_back = bring_back.clone();
            move |event: MenuEvent| {
                if event.id == open {
                    bring_back();
                } else if event.id == quit {
                    // The settings are kept as they change: nothing is left
                    // to put away.
                    std::process::exit(0);
                }
            }
        }));
        TrayIconEvent::set_event_handler(Some(move |event: TrayIconEvent| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                bring_back();
            }
        }));
        Some(tray)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_an_address_of_this_computer_is_here_and_not_the_loopback() {
        // An address of this computer, as the network knows it.
        let here = UdpSocket::bind("0.0.0.0:0")
            .and_then(|socket| socket.connect("192.0.2.1:9").and_then(|()| socket.local_addr()))
            .map(|address| address.ip());
        if let Ok(here) = here
            && !here.is_loopback()
        {
            assert!(is_here(&here.to_string()));
        }
        // An address kept for documentation, which no computer has.
        assert!(!is_here("192.0.2.1"));
        assert!(!is_here("127.0.0.1"));
        assert!(!is_here("armand-desktop"));
    }

    #[test]
    fn a_picture_becomes_nv12_in_the_narrow_range() {
        use leto_host::camera::Seen;

        // White, black, red and grey, two pixels wide and two high each.
        let colours = [[255, 255, 255], [0, 0, 0], [255, 0, 0], [128, 128, 128]];
        let mut rgba = Vec::new();
        for _row in 0..2 {
            for colour in colours {
                for _ in 0..2 {
                    rgba.extend_from_slice(&[colour[0], colour[1], colour[2], 255]);
                }
            }
        }
        let seen = Seen { width: 8, height: 2, rgba };
        let (luma, chroma) = apps::nv12(&seen, 8, 2);
        assert_eq!(luma.len(), 16);
        assert_eq!(chroma.len(), 8);
        // BT.601, as for every picture under 720 lines.
        assert_eq!(&luma[..8], &[235, 235, 16, 16, 81, 81, 126, 126]);
        assert_eq!(chroma, [128, 128, 128, 128, 90, 240, 128, 128]);
    }
}
