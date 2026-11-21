//! What the app shows and how it reacts, with no drawing and no networking.
//!
//! The interface calls the methods of [`State`] when the person does
//! something, the backend's [`Event`]s are fed to [`State::apply`], and the
//! work the backend has to do comes out of [`State::take_requests`].

use leto_core::{
    engine::{Choices, ScreenMode, WireGuardFile},
    input::InputAllowed,
    model::{AppSummary, HostId, HostSummary, Pin, Reachability, TunnelSummary},
};

/// Seconds between two searches while the list of PCs is on screen.
const SEARCH_EVERY: f64 = 15.0;
/// Seconds between two looks at the tunnel while the settings show it.
const TUNNEL_EVERY: f64 = 2.0;

/// Work for the backend, and its news. Covers arrive ready to draw.
pub use leto_core::engine::{AppsProblem, Request, StreamEnd};
pub type Event = leto_core::engine::Event<egui::ColorImage>;

/// Something that is loading, loaded, or failed to load.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Loadable<T, E> {
    Loading,
    Ready(T),
    Failed(E),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Screen {
    /// The list of PCs.
    Pcs,
    /// What the camera of this device sees and what its microphone hears,
    /// with no PC involved.
    Devices(Devices),
    Pairing(Pairing),
    Apps(AppsView),
    /// An app of a PC, streamed. Fills the window.
    Stream(StreamView),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Pairing {
    pub host: HostSummary,
    /// Tells this attempt from an earlier one that was cancelled.
    pub attempt: u64,
    pub step: PairingStep,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PairingStep {
    /// The backend is choosing a PIN.
    Preparing,
    /// The PIN is on screen since `since`, in seconds of app time.
    ShowingPin {
        pin: Pin,
        since: f64,
    },
    Failed(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct AppsView {
    pub host: HostSummary,
    pub apps: Loadable<Vec<AppSummary>, AppsProblem>,
    /// What just happened that the person should know, such as why a stream
    /// ended.
    pub notice: Option<Notice>,
    /// The backend is closing the app running on the PC.
    pub quitting: bool,
}

impl AppsView {
    /// The app running on the PC, when it is one of the apps on screen.
    pub fn running(&self) -> Option<&AppSummary> {
        let running = self.host.running_app?;
        match &self.apps {
            Loadable::Ready(apps) => apps.iter().find(|app| app.id == running),
            _ => None,
        }
    }
}

/// A line that tells the person what happened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    pub text: String,
    /// Something went wrong, as opposed to something merely happened.
    pub problem: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StreamView {
    pub host: HostSummary,
    pub app: AppSummary,
    /// Tells this stream from an earlier one that was left.
    pub attempt: u64,
    /// The stream is connected. Before that, the PC is starting the app.
    pub connected: bool,
    /// The stream lost its PC, and Leto is getting it back. It stays
    /// connected meanwhile: the last picture, the keys and the full screen
    /// stay as they were.
    pub reconnecting: bool,
    /// What the PC lets this device play with, known once connected.
    pub input: InputAllowed,
    pub microphone: Mic,
    pub camera: Cam,
}

/// The microphone of this device during a stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mic {
    /// The PC takes none, or is not connected yet.
    NotTaken,
    /// The PC would take it and hears nothing: nobody turned it on.
    Off,
    /// It is on: the PC hears this device as soon as the microphone has
    /// opened.
    On,
    /// It was turned on and cannot be used: why.
    Failed(String),
}

/// The camera of this device during a stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cam {
    /// Nobody said yet whether the PC takes one: there is nothing to show.
    Unknown,
    /// The PC takes none.
    NotTaken,
    /// The PC would take it and sees nothing: nobody turned it on.
    Off,
    /// It is on: the PC sees this device as soon as the camera has opened
    /// and the PC agreed.
    On,
    /// It was turned on and cannot be sent: why.
    Failed(String),
}

/// The camera and the microphone of this device, tried out on their own:
/// nothing of them leaves the device.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Devices {
    pub camera: Trial,
    pub microphone: Trial,
}

/// A camera or a microphone that can be tried out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Device {
    Camera,
    Microphone,
}

/// A camera or a microphone being tried out.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Trial {
    /// Nobody turned it on: nothing is captured.
    #[default]
    Off,
    /// It is on: it captures as soon as it has opened.
    On,
    /// It was turned on and cannot be used: why.
    Failed(String),
}

/// The form that adds a PC by address.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AddForm {
    pub open: bool,
    /// The form just opened: its field should take the keyboard.
    pub wants_keyboard: bool,
    pub address: String,
    /// The backend is trying the address.
    pub busy: bool,
    pub error: Option<String>,
}

/// Leto's own WireGuard tunnel, as the settings show it, and the form that
/// takes its file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TunnelForm {
    /// The tunnel as the backend last told it. None without one, and
    /// before the backend has said.
    pub summary: Option<TunnelSummary>,
    /// The backend has said at least once.
    pub known: bool,
    /// The form is open: the person is giving a file.
    pub open: bool,
    /// The file, as pasted or dropped so far. It holds a private key and
    /// never shows in a log.
    pub draft: WireGuardFile,
    /// The backend is trying the file, or removing the tunnel.
    pub busy: bool,
    /// Why the file last given was not used.
    pub refused: Option<String>,
}

#[derive(Debug)]
pub struct State {
    pub screen: Screen,
    pub hosts: Vec<HostSummary>,
    pub searching: bool,
    /// At least one search has ended since the app started.
    pub searched: bool,
    /// The last search could listen on the network.
    pub network_searched: bool,
    pub add: AddForm,
    pub notice: Option<String>,
    /// The PCs asked to wake up, until they answer or Leto gives up.
    pub waking: Vec<HostId>,
    pub tunnel: TunnelForm,
    tunnel_asked: f64,
    last_search: f64,
    attempts: u64,
    requests: Vec<Request>,
}

impl State {
    pub fn new() -> Self {
        Self {
            screen: Screen::Pcs,
            hosts: Vec::new(),
            searching: false,
            searched: false,
            network_searched: true,
            add: AddForm::default(),
            notice: None,
            waking: Vec::new(),
            tunnel: TunnelForm::default(),
            tunnel_asked: f64::NEG_INFINITY,
            last_search: f64::NEG_INFINITY,
            attempts: 0,
            requests: Vec::new(),
        }
    }

    /// The work that piled up since the last call.
    pub fn take_requests(&mut self) -> Vec<Request> {
        std::mem::take(&mut self.requests)
    }

    /// Called every frame: keeps the list of PCs fresh while it is on screen.
    pub fn tick(&mut self, now: f64) {
        if self.screen == Screen::Pcs && now - self.last_search >= SEARCH_EVERY {
            self.search(now);
        }
    }

    /// Starts a search unless one is running.
    pub fn search(&mut self, now: f64) {
        self.last_search = now;
        if !self.searching {
            self.searching = true;
            self.requests.push(Request::Search);
        }
    }

    /// The person chose a PC in the list.
    pub fn open(&mut self, id: &HostId, now: f64) {
        let Some(host) = self.hosts.iter().find(|host| host.id == *id).cloned() else { return };
        self.notice = None;
        if host.reachability == Reachability::Offline && host.wakeable {
            // It sleeps or is off: wake it, and open it once it answers.
            if !self.waking.contains(id) {
                self.waking.push(id.clone());
                self.requests.push(Request::Wake(id.clone()));
            }
        } else if host.reachability == Reachability::Offline {
            // It may just have been switched on.
            self.search(now);
        } else if host.paired {
            self.show_apps(host);
        } else {
            self.start_pairing(host);
        }
    }

    pub fn start_pairing(&mut self, host: HostSummary) {
        self.attempts += 1;
        self.requests.push(Request::Pair { host: host.id.clone(), attempt: self.attempts });
        self.screen =
            Screen::Pairing(Pairing { host, attempt: self.attempts, step: PairingStep::Preparing });
    }

    fn show_apps(&mut self, host: HostSummary) {
        self.show_apps_with(host, None);
    }

    fn show_apps_with(&mut self, host: HostSummary, notice: Option<Notice>) {
        self.requests.push(Request::Apps(host.id.clone()));
        self.screen =
            Screen::Apps(AppsView { host, apps: Loadable::Loading, notice, quitting: false });
    }

    /// The person chose an app: start it, or go back to it if it is the one
    /// running, for `screen` when the window knows the screen it should
    /// fill, and as the person chose.
    pub fn play(&mut self, app: &AppSummary, screen: Option<ScreenMode>, choices: Choices) {
        let Screen::Apps(view) = &mut self.screen else { return };
        if view.quitting {
            return;
        }
        // A PC runs one app at a time.
        if let Some(running) = view.host.running_app.filter(|running| *running != app.id) {
            let other = match &view.apps {
                Loadable::Ready(apps) => apps.iter().find(|other| other.id == running),
                _ => None,
            };
            let other = other.map_or("Another app", |other| other.title.as_str());
            view.notice = Some(Notice {
                text: format!("{other} is running on this PC. Close it to start {}.", app.title),
                problem: true,
            });
            return;
        }
        self.attempts += 1;
        self.requests.push(Request::Play {
            host: view.host.id.clone(),
            app: app.id,
            attempt: self.attempts,
            screen,
            choices,
        });
        self.screen = Screen::Stream(StreamView {
            host: view.host.clone(),
            app: app.clone(),
            attempt: self.attempts,
            connected: false,
            reconnecting: false,
            input: InputAllowed::ALL,
            // Off at the start of every stream: a PC hears and sees this
            // device only once the person says so.
            microphone: Mic::NotTaken,
            camera: Cam::Unknown,
        });
    }

    /// The person turns the microphone on, or off again. A microphone that
    /// failed is tried again.
    pub fn switch_microphone(&mut self) {
        if let Screen::Stream(view) = &mut self.screen
            && view.connected
        {
            view.microphone = match view.microphone {
                Mic::NotTaken => Mic::NotTaken,
                Mic::On => Mic::Off,
                Mic::Off | Mic::Failed(_) => Mic::On,
            };
        }
    }

    /// The microphone that was turned on cannot be used, for this reason.
    pub fn microphone_failed(&mut self, why: String) {
        if let Screen::Stream(view) = &mut self.screen
            && view.microphone == Mic::On
        {
            view.microphone = Mic::Failed(why);
        }
    }

    /// The person turns the camera on, or off again. A camera that failed
    /// is tried again.
    pub fn switch_camera(&mut self) {
        if let Screen::Stream(view) = &mut self.screen
            && view.connected
        {
            view.camera = match view.camera {
                Cam::Unknown => Cam::Unknown,
                Cam::NotTaken => Cam::NotTaken,
                Cam::On => Cam::Off,
                Cam::Off | Cam::Failed(_) => Cam::On,
            };
        }
    }

    /// The camera that was turned on cannot be sent, for this reason.
    pub fn camera_failed(&mut self, why: String) {
        if let Screen::Stream(view) = &mut self.screen
            && view.camera == Cam::On
        {
            view.camera = Cam::Failed(why);
        }
    }

    /// The person wants to see what this device's camera and microphone
    /// give. Both are off until turned on there.
    pub fn check_devices(&mut self) {
        if self.screen == Screen::Pcs {
            self.notice = None;
            self.screen = Screen::Devices(Devices::default());
        }
    }

    /// The person turns the camera or the microphone on, or off again, on
    /// the screen that tries them out. One that failed is tried again.
    pub fn switch_device(&mut self, device: Device) {
        if let Screen::Devices(devices) = &mut self.screen {
            let trial = match device {
                Device::Camera => &mut devices.camera,
                Device::Microphone => &mut devices.microphone,
            };
            *trial = match trial {
                Trial::On => Trial::Off,
                Trial::Off | Trial::Failed(_) => Trial::On,
            };
        }
    }

    /// The device that was turned on cannot be used, for this reason.
    pub fn device_failed(&mut self, device: Device, why: String) {
        if let Screen::Devices(devices) = &mut self.screen {
            let trial = match device {
                Device::Camera => &mut devices.camera,
                Device::Microphone => &mut devices.microphone,
            };
            if *trial == Trial::On {
                *trial = Trial::Failed(why);
            }
        }
    }

    /// The person stops watching. The app keeps running on the PC, unless
    /// the person chose to `quit` it too.
    pub fn leave_stream(&mut self, quit: bool) {
        if let Screen::Stream(view) = &self.screen {
            let host = view.host.clone();
            self.requests.push(Request::Leave);
            self.show_apps(host);
            if quit {
                self.quit_app();
            }
        }
    }

    /// The person closes the app running on the PC whose apps are on screen.
    pub fn quit_app(&mut self) {
        if let Screen::Apps(view) = &mut self.screen
            && !view.quitting
        {
            view.quitting = true;
            view.notice = None;
            self.requests.push(Request::Quit(view.host.id.clone()));
        }
    }

    /// Asks again for the apps on screen.
    pub fn reload_apps(&mut self) {
        if let Screen::Apps(view) = &self.screen {
            self.show_apps(view.host.clone());
        }
    }

    /// Back to the list of PCs, from any screen.
    pub fn back(&mut self, now: f64) {
        if matches!(self.screen, Screen::Stream(_)) {
            self.requests.push(Request::Leave);
        }
        if self.screen != Screen::Pcs {
            self.screen = Screen::Pcs;
            self.search(now);
        }
    }

    /// Shows the form that adds a PC by address.
    pub fn open_add_form(&mut self) {
        self.add.open = true;
        self.add.wants_keyboard = true;
        self.add.error = None;
    }

    /// The person confirmed the address in the form.
    pub fn submit_address(&mut self) {
        let address = self.add.address.trim().to_owned();
        if self.add.busy {
            return;
        }
        if address.is_empty() {
            self.add.error = Some("Type the address of your PC first.".to_owned());
            return;
        }
        self.add.busy = true;
        self.add.error = None;
        self.requests.push(Request::Add(address));
    }

    pub fn forget(&mut self, id: &HostId) {
        self.hosts.retain(|host| host.id != *id);
        self.requests.push(Request::Forget(id.clone()));
        let shown = match &self.screen {
            Screen::Pcs | Screen::Devices(_) => None,
            Screen::Pairing(pairing) => Some(&pairing.host.id),
            Screen::Apps(view) => Some(&view.host.id),
            Screen::Stream(view) => Some(&view.host.id),
        };
        if shown == Some(id) {
            if matches!(self.screen, Screen::Stream(_)) {
                self.requests.push(Request::Leave);
            }
            self.screen = Screen::Pcs;
        }
    }

    /// Called every frame while the settings show the tunnel: asks the
    /// backend how it is doing, now and then.
    pub fn watch_tunnel(&mut self, now: f64) {
        if now - self.tunnel_asked >= TUNNEL_EVERY {
            self.tunnel_asked = now;
            self.requests.push(Request::Tunnel);
        }
    }

    /// The person gave a WireGuard file to use from now on.
    pub fn use_tunnel(&mut self) {
        if self.tunnel.busy || self.tunnel.draft.0.trim().is_empty() {
            return;
        }
        self.tunnel.busy = true;
        self.tunnel.refused = None;
        self.requests.push(Request::UseTunnel(self.tunnel.draft.clone()));
    }

    /// The person wants no tunnel any more.
    pub fn remove_tunnel(&mut self) {
        if !self.tunnel.busy {
            self.tunnel.busy = true;
            self.tunnel.refused = None;
            self.requests.push(Request::RemoveTunnel);
        }
    }

    /// Takes in news from the backend. `now` is the app time in seconds.
    pub fn apply(&mut self, event: Event, now: f64) {
        match event {
            Event::Hosts(hosts) => {
                // The screens about one PC follow what is known about it.
                let shown = match &mut self.screen {
                    Screen::Pcs | Screen::Devices(_) => None,
                    Screen::Pairing(pairing) => Some(&mut pairing.host),
                    Screen::Apps(view) => Some(&mut view.host),
                    Screen::Stream(view) => Some(&mut view.host),
                };
                if let Some(shown) = shown
                    && let Some(fresh) = hosts.iter().find(|host| host.id == shown.id)
                {
                    *shown = fresh.clone();
                }
                self.hosts = hosts;
            }
            Event::SearchFinished { network_searched } => {
                self.searching = false;
                self.searched = true;
                self.network_searched = network_searched;
                self.last_search = now;
            }
            Event::Added(Ok(host)) => {
                self.add = AddForm::default();
                // Whoever adds a PC wants to use it: go on to pairing.
                if !host.paired && self.screen == Screen::Pcs {
                    self.start_pairing(host);
                }
            }
            Event::Added(Err(message)) => {
                self.add.busy = false;
                self.add.error = Some(message);
            }
            Event::PinChosen { attempt, pin } => {
                if let Screen::Pairing(pairing) = &mut self.screen
                    && pairing.attempt == attempt
                {
                    pairing.step = PairingStep::ShowingPin { pin, since: now };
                }
            }
            Event::PairingEnded { attempt, result } => {
                // An attempt the person cancelled changes no screen.
                let Screen::Pairing(pairing) = &mut self.screen else { return };
                if pairing.attempt != attempt {
                    return;
                }
                match result {
                    Ok(host) => self.show_apps(host),
                    Err(message) => pairing.step = PairingStep::Failed(message),
                }
            }
            Event::Apps { host, result } => {
                if let Screen::Apps(view) = &mut self.screen
                    && view.host.id == host
                {
                    view.apps = match result {
                        Ok(apps) => Loadable::Ready(apps),
                        Err(problem) => Loadable::Failed(problem),
                    };
                }
            }
            Event::Playing { attempt, input, microphone } => {
                if let Screen::Stream(view) = &mut self.screen
                    && view.attempt == attempt
                {
                    // A stream that comes back keeps the microphone the
                    // person turned on.
                    let back = std::mem::take(&mut view.reconnecting);
                    view.connected = true;
                    view.input = input;
                    view.microphone = match &view.microphone {
                        Mic::On if back && microphone => Mic::On,
                        _ if microphone => Mic::Off,
                        _ => Mic::NotTaken,
                    };
                }
            }
            Event::Reconnecting { attempt } => {
                if let Screen::Stream(view) = &mut self.screen
                    && view.attempt == attempt
                {
                    view.reconnecting = true;
                }
            }
            Event::Camera { attempt, offer } => {
                if let Screen::Stream(view) = &mut self.screen
                    && view.attempt == attempt
                {
                    // A stream that came back keeps the camera as the person
                    // left it, when the PC still takes one.
                    match (offer, &view.camera) {
                        (None, _) => view.camera = Cam::NotTaken,
                        (Some(_), Cam::Unknown | Cam::NotTaken) => view.camera = Cam::Off,
                        (Some(_), Cam::Off | Cam::On | Cam::Failed(_)) => {}
                    }
                }
            }
            Event::PlayFailed { attempt, message } => {
                // A stream the person already left changes no screen.
                let Screen::Stream(view) = &self.screen else { return };
                if view.attempt != attempt {
                    return;
                }
                let host = view.host.clone();
                self.show_apps_with(host, Some(Notice { text: message, problem: true }));
            }
            Event::StreamEnded { attempt, why } => {
                let Screen::Stream(view) = &self.screen else { return };
                if view.attempt != attempt {
                    return;
                }
                let (host, app) = (view.host.clone(), &view.app.title);
                let notice = match why {
                    StreamEnd::AppClosed => {
                        Notice { text: format!("{app} was closed on the PC."), problem: false }
                    }
                    StreamEnd::ConnectionLost => Notice {
                        text: format!("The connection to {} was lost.", host.name),
                        problem: true,
                    },
                    StreamEnd::HostError(code) => Notice {
                        text: format!("{} ended the stream (error {code:#x}).", host.name),
                        problem: true,
                    },
                };
                // The backend still holds what is left of the stream.
                self.requests.push(Request::Leave);
                self.show_apps_with(host, Some(notice));
            }
            Event::QuitEnded { host, result } => {
                if let Screen::Apps(view) = &mut self.screen
                    && view.host.id == host
                {
                    view.quitting = false;
                    if let Err(message) = result {
                        view.notice = Some(Notice { text: message, problem: true });
                    }
                }
            }
            Event::Tunnel { summary, refused } => {
                let form = &mut self.tunnel;
                form.summary = summary;
                form.known = true;
                // An answer to a look changes nothing of a form in use.
                if form.busy {
                    form.busy = false;
                    if refused.is_none() {
                        // Used, or removed: the key leaves the window.
                        form.open = false;
                        form.draft = WireGuardFile(String::new());
                    }
                    form.refused = refused;
                }
            }
            Event::WakeEnded { host, result } => {
                if !self.waking.contains(&host) {
                    return;
                }
                self.waking.retain(|waking| *waking != host);
                // Opened as the person asked, unless they went on to
                // something else meanwhile.
                match result {
                    Ok(woken) if self.screen == Screen::Pcs => {
                        if woken.paired {
                            self.show_apps(woken);
                        } else {
                            self.start_pairing(woken);
                        }
                    }
                    Ok(_) => {}
                    Err(message) => self.notice = Some(message),
                }
            }
            // Covers are pictures: the drawing side keeps them.
            Event::Cover { .. } => {}
            Event::Notice(message) => self.notice = Some(message),
        }
    }
}

#[cfg(test)]
mod tests {
    use leto_core::{engine::CameraOffer, model::HostKind};

    use super::*;

    fn host(name: &str, paired: bool, reachability: Reachability) -> HostSummary {
        HostSummary {
            id: HostId(format!("id-{name}")),
            name: name.to_owned(),
            address: "192.168.1.20".to_owned(),
            port: 47989,
            kind: HostKind::Apollo,
            paired,
            reachability,
            busy: false,
            running_app: None,
            wakeable: false,
            tunnel: false,
        }
    }

    fn state_with(hosts: Vec<HostSummary>) -> State {
        let mut state = State::new();
        state.tick(0.0);
        assert_eq!(state.take_requests(), [Request::Search]);
        state.apply(Event::Hosts(hosts), 0.5);
        state.apply(Event::SearchFinished { network_searched: true }, 0.5);
        state
    }

    fn pin() -> Pin {
        Pin::from_bytes([4, 8, 2, 1])
    }

    #[test]
    fn the_list_searches_at_start_and_again_later_but_never_twice_at_once() {
        let mut state = State::new();
        state.tick(0.0);
        state.tick(0.1);
        assert_eq!(state.take_requests(), [Request::Search]);
        assert!(state.searching && !state.searched);

        // A slow search is not doubled.
        state.tick(60.0);
        assert_eq!(state.take_requests(), []);

        state.apply(Event::SearchFinished { network_searched: true }, 61.0);
        assert!(!state.searching && state.searched);
        state.tick(70.0);
        assert_eq!(state.take_requests(), []);
        state.tick(61.0 + SEARCH_EVERY);
        assert_eq!(state.take_requests(), [Request::Search]);
    }

    #[test]
    fn choosing_an_unpaired_pc_pairs_then_shows_its_apps() {
        let salon = host("Salon", false, Reachability::Online);
        let mut state = state_with(vec![salon.clone()]);

        state.open(&salon.id, 1.0);
        assert_eq!(state.take_requests(), [Request::Pair { host: salon.id.clone(), attempt: 1 }]);
        let Screen::Pairing(pairing) = &state.screen else { panic!("{:?}", state.screen) };
        assert_eq!(pairing.step, PairingStep::Preparing);

        state.apply(Event::PinChosen { attempt: 1, pin: pin() }, 2.0);
        let Screen::Pairing(pairing) = &state.screen else { panic!("{:?}", state.screen) };
        assert_eq!(pairing.step, PairingStep::ShowingPin { pin: pin(), since: 2.0 });

        let paired = host("Salon", true, Reachability::Online);
        state.apply(Event::PairingEnded { attempt: 1, result: Ok(paired.clone()) }, 9.0);
        assert_eq!(state.take_requests(), [Request::Apps(salon.id.clone())]);
        assert_eq!(
            state.screen,
            Screen::Apps(AppsView {
                host: paired,
                apps: Loadable::Loading,
                notice: None,
                quitting: false
            })
        );

        let apps = vec![AppSummary { id: 1, title: "Desktop".into(), hdr: true }];
        state.apply(Event::Apps { host: salon.id, result: Ok(apps.clone()) }, 9.5);
        let Screen::Apps(view) = &state.screen else { panic!("{:?}", state.screen) };
        assert_eq!(view.apps, Loadable::Ready(apps));
    }

    #[test]
    fn a_wrong_pin_can_be_tried_again_with_a_new_attempt() {
        let salon = host("Salon", false, Reachability::Online);
        let mut state = state_with(vec![salon.clone()]);
        state.open(&salon.id, 1.0);
        state.apply(Event::PinChosen { attempt: 1, pin: pin() }, 2.0);
        state.apply(Event::PairingEnded { attempt: 1, result: Err("No match.".into()) }, 3.0);
        let Screen::Pairing(pairing) = &state.screen else { panic!("{:?}", state.screen) };
        assert_eq!(pairing.step, PairingStep::Failed("No match.".into()));
        state.take_requests();

        state.start_pairing(salon.clone());
        assert_eq!(state.take_requests(), [Request::Pair { host: salon.id, attempt: 2 }]);
    }

    #[test]
    fn a_cancelled_pairing_that_ends_later_changes_no_screen() {
        let salon = host("Salon", false, Reachability::Online);
        let mut state = state_with(vec![salon.clone()]);
        state.open(&salon.id, 1.0);
        state.back(2.0);
        assert_eq!(state.screen, Screen::Pcs);

        // The person typed the PIN on the PC after all.
        let paired = host("Salon", true, Reachability::Online);
        state.apply(Event::PairingEnded { attempt: 1, result: Ok(paired.clone()) }, 30.0);
        assert_eq!(state.screen, Screen::Pcs);

        // The same goes for a second attempt on screen: the first one's end
        // must not be taken for its own.
        state.start_pairing(salon);
        state.apply(Event::PairingEnded { attempt: 1, result: Err("Too late.".into()) }, 31.0);
        let Screen::Pairing(pairing) = &state.screen else { panic!("{:?}", state.screen) };
        assert_eq!((pairing.attempt, &pairing.step), (2, &PairingStep::Preparing));
    }

    #[test]
    fn choosing_a_paired_pc_goes_straight_to_its_apps() {
        let salon = host("Salon", true, Reachability::Online);
        let mut state = state_with(vec![salon.clone()]);
        state.open(&salon.id, 1.0);
        assert_eq!(state.take_requests(), [Request::Apps(salon.id.clone())]);

        // Apps of another PC, asked for earlier, do not land on this screen.
        let other = HostId("id-Bureau".into());
        state.apply(Event::Apps { host: other, result: Ok(vec![]) }, 2.0);
        let Screen::Apps(view) = &state.screen else { panic!("{:?}", state.screen) };
        assert_eq!(view.apps, Loadable::Loading);
    }

    #[test]
    fn choosing_a_pc_that_is_off_and_cannot_be_woken_searches_again() {
        let salon = host("Salon", true, Reachability::Offline);
        let mut state = state_with(vec![salon.clone()]);
        state.open(&salon.id, 1.0);
        assert_eq!(state.screen, Screen::Pcs);
        assert_eq!(state.take_requests(), [Request::Search]);
    }

    #[test]
    fn choosing_a_pc_that_is_off_wakes_it_and_opens_it_once_it_answers() {
        let salon = HostSummary { wakeable: true, ..host("Salon", true, Reachability::Offline) };
        let mut state = state_with(vec![salon.clone()]);
        state.open(&salon.id, 1.0);
        assert_eq!(state.take_requests(), [Request::Wake(salon.id.clone())]);
        assert_eq!(state.waking, std::slice::from_ref(&salon.id));
        // Asked once, however impatient the person is.
        state.open(&salon.id, 2.0);
        assert_eq!(state.take_requests(), []);

        let awake = HostSummary { reachability: Reachability::Online, ..salon.clone() };
        state.apply(Event::WakeEnded { host: salon.id.clone(), result: Ok(awake) }, 30.0);
        assert!(state.waking.is_empty());
        assert_eq!(state.take_requests(), [Request::Apps(salon.id.clone())]);
        assert!(matches!(&state.screen, Screen::Apps(view) if view.host.id == salon.id));
    }

    #[test]
    fn a_pc_that_does_not_wake_up_says_why_and_one_that_wakes_late_stays_put() {
        let salon = HostSummary { wakeable: true, ..host("Salon", true, Reachability::Offline) };
        let mut state = state_with(vec![salon.clone()]);
        state.open(&salon.id, 1.0);
        let why = "The PC did not wake up within 120 seconds.".to_owned();
        state.apply(Event::WakeEnded { host: salon.id.clone(), result: Err(why.clone()) }, 121.0);
        assert_eq!((&state.screen, state.notice.as_ref()), (&Screen::Pcs, Some(&why)));

        // The person went on to another screen meanwhile: it stays on screen.
        state.open(&salon.id, 130.0);
        state.check_devices();
        let awake = HostSummary { reachability: Reachability::Online, ..salon.clone() };
        state.apply(Event::WakeEnded { host: salon.id.clone(), result: Ok(awake) }, 140.0);
        assert!(matches!(state.screen, Screen::Devices(_)), "{:?}", state.screen);
    }

    #[test]
    fn adding_by_address_checks_the_form_then_goes_on_to_pairing() {
        let mut state = state_with(vec![]);
        state.open_add_form();
        assert!(state.add.open && state.add.wants_keyboard);

        state.add.address = "   ".into();
        state.submit_address();
        assert!(state.add.error.is_some() && !state.add.busy);
        assert_eq!(state.take_requests(), []);

        state.add.address = " 192.168.1.20 ".into();
        state.submit_address();
        state.submit_address();
        assert!(state.add.busy && state.add.error.is_none());
        assert_eq!(state.take_requests(), [Request::Add("192.168.1.20".into())]);

        state.apply(Event::Added(Err("The PC did not answer.".into())), 1.0);
        assert!(!state.add.busy && state.add.open);
        assert_eq!(state.add.error.as_deref(), Some("The PC did not answer."));

        state.submit_address();
        state.take_requests();
        let salon = host("Salon", false, Reachability::Online);
        state.apply(Event::Added(Ok(salon.clone())), 2.0);
        assert_eq!(state.add, AddForm::default());
        assert!(matches!(&state.screen, Screen::Pairing(pairing) if pairing.host == salon));
    }

    #[test]
    fn forgetting_the_pc_on_screen_returns_to_the_list() {
        let salon = host("Salon", true, Reachability::Online);
        let bureau = host("Bureau", true, Reachability::Online);
        let mut state = state_with(vec![salon.clone(), bureau.clone()]);
        state.open(&salon.id, 1.0);
        state.take_requests();

        state.forget(&salon.id);
        assert_eq!(state.screen, Screen::Pcs);
        assert_eq!(state.hosts, [bureau]);
        assert_eq!(state.take_requests(), [Request::Forget(salon.id)]);
    }

    #[test]
    fn the_screen_about_a_pc_follows_news_about_it() {
        let salon = host("Salon", true, Reachability::Online);
        let mut state = state_with(vec![salon.clone()]);
        state.open(&salon.id, 1.0);

        let mut busy = salon;
        busy.busy = true;
        state.apply(Event::Hosts(vec![busy.clone()]), 2.0);
        let Screen::Apps(view) = &state.screen else { panic!("{:?}", state.screen) };
        assert_eq!(view.host, busy);
    }

    fn app(id: u32, title: &str) -> AppSummary {
        AppSummary { id, title: title.to_owned(), hdr: false }
    }

    /// A state showing the apps of a paired PC, with nothing left to send.
    fn state_with_apps(salon: &HostSummary) -> State {
        let mut state = state_with(vec![salon.clone()]);
        state.open(&salon.id, 1.0);
        let apps = vec![app(1, "Desktop"), app(2, "Steam")];
        state.apply(Event::Apps { host: salon.id.clone(), result: Ok(apps) }, 1.5);
        state.take_requests();
        state
    }

    /// The news that stream `attempt` is connected, to a PC that takes a
    /// microphone or not.
    fn playing(attempt: u64, microphone: bool) -> Event {
        Event::Playing { attempt, input: InputAllowed::ALL, microphone }
    }

    fn microphone(state: &State) -> &Mic {
        let Screen::Stream(view) = &state.screen else { panic!("{:?}", state.screen) };
        &view.microphone
    }

    #[test]
    fn the_microphone_is_off_until_the_person_turns_it_on_in_every_stream() {
        let salon = host("Salon", true, Reachability::Online);
        let mut state = state_with_apps(&salon);
        state.play(&app(1, "Desktop"), None, Choices::default());

        // Nothing to switch before the PC said it takes one.
        state.switch_microphone();
        assert_eq!(microphone(&state), &Mic::NotTaken);

        state.apply(playing(1, true), 3.0);
        assert_eq!(microphone(&state), &Mic::Off);
        state.switch_microphone();
        assert_eq!(microphone(&state), &Mic::On);
        state.switch_microphone();
        assert_eq!(microphone(&state), &Mic::Off);

        // Left on, it is off again in the next stream.
        state.switch_microphone();
        state.leave_stream(false);
        state.play(&app(1, "Desktop"), None, Choices::default());
        state.apply(playing(2, true), 9.0);
        assert_eq!(microphone(&state), &Mic::Off);
    }

    #[test]
    fn a_pc_that_takes_no_microphone_has_nothing_to_switch() {
        let salon = host("Salon", true, Reachability::Online);
        let mut state = state_with_apps(&salon);
        state.play(&app(1, "Desktop"), None, Choices::default());
        state.apply(playing(1, false), 3.0);
        state.switch_microphone();
        assert_eq!(microphone(&state), &Mic::NotTaken);
    }

    #[test]
    fn a_microphone_that_cannot_be_used_says_why_and_can_be_tried_again() {
        let salon = host("Salon", true, Reachability::Online);
        let mut state = state_with_apps(&salon);
        state.play(&app(1, "Desktop"), None, Choices::default());
        state.apply(playing(1, true), 3.0);

        // A failure nobody asked for a microphone for changes nothing.
        state.microphone_failed("Too early.".into());
        assert_eq!(microphone(&state), &Mic::Off);

        state.switch_microphone();
        state.microphone_failed("This device has no microphone.".into());
        assert_eq!(microphone(&state), &Mic::Failed("This device has no microphone.".into()));

        state.switch_microphone();
        assert_eq!(microphone(&state), &Mic::On);
    }

    /// The news that the PC of stream `attempt` takes a camera, or none.
    fn camera_news(attempt: u64, takes: bool) -> Event {
        let offer = CameraOffer { max_width: 1920, max_height: 1080, max_fps: 30 };
        Event::Camera { attempt, offer: takes.then_some(offer) }
    }

    fn camera(state: &State) -> &Cam {
        let Screen::Stream(view) = &state.screen else { panic!("{:?}", state.screen) };
        &view.camera
    }

    #[test]
    fn the_camera_is_off_until_the_person_turns_it_on_in_every_stream() {
        let salon = host("Salon", true, Reachability::Online);
        let mut state = state_with_apps(&salon);
        state.play(&app(1, "Desktop"), None, Choices::default());
        state.apply(playing(1, true), 3.0);

        // Nothing to switch before the PC said whether it takes one, which
        // comes a moment after the stream.
        state.switch_camera();
        assert_eq!(camera(&state), &Cam::Unknown);

        state.apply(camera_news(1, true), 3.5);
        assert_eq!(camera(&state), &Cam::Off);
        state.switch_camera();
        assert_eq!(camera(&state), &Cam::On);
        // Said twice, it does not turn off what the person turned on.
        state.apply(camera_news(1, true), 3.6);
        assert_eq!(camera(&state), &Cam::On);
        state.switch_camera();
        assert_eq!(camera(&state), &Cam::Off);

        // Left on, it is off again in the next stream, and what the PC of
        // the stream before says comes too late to matter.
        state.switch_camera();
        state.leave_stream(false);
        state.play(&app(1, "Desktop"), None, Choices::default());
        state.apply(playing(2, true), 9.0);
        state.apply(camera_news(1, true), 9.1);
        assert_eq!(camera(&state), &Cam::Unknown);
        state.apply(camera_news(2, true), 9.5);
        assert_eq!(camera(&state), &Cam::Off);
    }

    #[test]
    fn a_pc_that_takes_no_camera_has_nothing_to_switch() {
        let salon = host("Salon", true, Reachability::Online);
        let mut state = state_with_apps(&salon);
        state.play(&app(1, "Desktop"), None, Choices::default());
        state.apply(playing(1, true), 3.0);
        state.apply(camera_news(1, false), 3.5);
        state.switch_camera();
        assert_eq!(camera(&state), &Cam::NotTaken);
    }

    #[test]
    fn a_camera_that_cannot_be_sent_says_why_and_can_be_tried_again() {
        let salon = host("Salon", true, Reachability::Online);
        let mut state = state_with_apps(&salon);
        state.play(&app(1, "Desktop"), None, Choices::default());
        state.apply(playing(1, true), 3.0);
        state.apply(camera_news(1, true), 3.5);

        // A failure nobody asked for a camera for changes nothing.
        state.camera_failed("Too early.".into());
        assert_eq!(camera(&state), &Cam::Off);

        state.switch_camera();
        state.camera_failed("This device has no camera.".into());
        assert_eq!(camera(&state), &Cam::Failed("This device has no camera.".into()));
        // The microphone is another matter.
        assert_eq!(microphone(&state), &Mic::Off);

        state.switch_camera();
        assert_eq!(camera(&state), &Cam::On);
    }

    #[test]
    fn choosing_an_app_streams_it_and_leaving_returns_to_the_apps() {
        let salon = host("Salon", true, Reachability::Online);
        let mut state = state_with_apps(&salon);

        state.play(&app(2, "Steam"), None, Choices::default());
        let play = Request::Play {
            host: salon.id.clone(),
            app: 2,
            attempt: 1,
            screen: None,
            choices: Choices::default(),
        };
        assert_eq!(state.take_requests(), [play]);
        let Screen::Stream(view) = &state.screen else { panic!("{:?}", state.screen) };
        assert!(!view.connected);

        state.apply(playing(1, false), 3.0);
        let Screen::Stream(view) = &state.screen else { panic!("{:?}", state.screen) };
        assert!(view.connected);

        state.leave_stream(false);
        assert_eq!(state.take_requests(), [Request::Leave, Request::Apps(salon.id)]);
        let Screen::Apps(view) = &state.screen else { panic!("{:?}", state.screen) };
        assert_eq!(view.notice, None);
    }

    #[test]
    fn a_stream_that_lost_its_pc_stays_on_screen_until_it_is_back() {
        let salon = host("Salon", true, Reachability::Online);
        let mut state = state_with_apps(&salon);
        state.play(&app(1, "Desktop"), None, Choices::default());
        state.apply(playing(1, true), 3.0);
        state.apply(camera_news(1, true), 3.1);
        state.switch_microphone();
        state.switch_camera();
        state.take_requests();

        state.apply(Event::Reconnecting { attempt: 1 }, 20.0);
        let Screen::Stream(view) = &state.screen else { panic!("{:?}", state.screen) };
        assert!(view.connected && view.reconnecting);
        assert_eq!(state.take_requests(), []);
        // News of a stream left earlier changes nothing.
        state.apply(Event::Reconnecting { attempt: 9 }, 21.0);

        // Back: the microphone and the camera the person turned on stay on.
        state.apply(playing(1, true), 30.0);
        state.apply(camera_news(1, true), 30.1);
        let Screen::Stream(view) = &state.screen else { panic!("{:?}", state.screen) };
        assert!(view.connected && !view.reconnecting);
        assert_eq!(view.microphone, Mic::On);
        assert_eq!(view.camera, Cam::On);

        // Unless the PC that is back takes none.
        state.apply(Event::Reconnecting { attempt: 1 }, 40.0);
        state.apply(playing(1, false), 41.0);
        state.apply(camera_news(1, false), 41.1);
        assert_eq!(microphone(&state), &Mic::NotTaken);
        assert_eq!(camera(&state), &Cam::NotTaken);
    }

    #[test]
    fn a_stream_that_does_not_start_says_why_on_the_apps_screen() {
        let salon = host("Salon", true, Reachability::Online);
        let mut state = state_with_apps(&salon);
        state.play(&app(1, "Desktop"), None, Choices::default());
        state.take_requests();

        state
            .apply(Event::PlayFailed { attempt: 1, message: "The PC did not answer.".into() }, 4.0);
        assert_eq!(state.take_requests(), [Request::Apps(salon.id)]);
        let Screen::Apps(view) = &state.screen else { panic!("{:?}", state.screen) };
        let notice = Notice { text: "The PC did not answer.".into(), problem: true };
        assert_eq!(view.notice, Some(notice));
    }

    #[test]
    fn a_stream_that_ends_on_its_own_returns_to_the_apps_and_says_why() {
        let salon = host("Salon", true, Reachability::Online);
        for (why, text, problem) in [
            (StreamEnd::AppClosed, "Steam was closed on the PC.", false),
            (StreamEnd::ConnectionLost, "The connection to Salon was lost.", true),
            (StreamEnd::HostError(0x8003_0023), "Salon ended the stream (error 0x80030023).", true),
        ] {
            let mut state = state_with_apps(&salon);
            state.play(&app(2, "Steam"), None, Choices::default());
            state.apply(playing(1, false), 3.0);
            state.take_requests();

            state.apply(Event::StreamEnded { attempt: 1, why }, 9.0);
            assert_eq!(state.take_requests(), [Request::Leave, Request::Apps(salon.id.clone())]);
            let Screen::Apps(view) = &state.screen else { panic!("{:?}", state.screen) };
            assert_eq!(view.notice, Some(Notice { text: text.into(), problem }));
        }
    }

    #[test]
    fn a_stream_is_asked_for_as_the_person_chose_and_can_close_its_app_when_left() {
        let salon = host("Salon", true, Reachability::Online);
        let mut state = state_with_apps(&salon);
        let chosen = Choices { bitrate_kbps: Some(9000), sound_on_pc: true, ..Choices::default() };
        state.play(&app(1, "Desktop"), None, chosen);
        assert_eq!(
            state.take_requests(),
            [Request::Play {
                host: salon.id.clone(),
                app: 1,
                attempt: 1,
                screen: None,
                choices: chosen
            }]
        );
        state.apply(playing(1, true), 3.0);

        // Left as chosen: the app closes on the PC too, after the stream.
        state.leave_stream(true);
        assert_eq!(
            state.take_requests(),
            [Request::Leave, Request::Apps(salon.id.clone()), Request::Quit(salon.id.clone())]
        );
        let Screen::Apps(view) = &state.screen else { panic!("{:?}", state.screen) };
        assert!(view.quitting);
    }

    #[test]
    fn news_about_a_stream_the_person_left_changes_no_screen() {
        let salon = host("Salon", true, Reachability::Online);
        let mut state = state_with_apps(&salon);
        state.play(&app(1, "Desktop"), None, Choices::default());
        state.leave_stream(false);
        state.take_requests();
        let before = state.screen.clone();

        state.apply(playing(1, true), 5.0);
        state.apply(Event::PlayFailed { attempt: 1, message: "Too late.".into() }, 5.0);
        state.apply(Event::StreamEnded { attempt: 1, why: StreamEnd::ConnectionLost }, 5.0);
        assert_eq!(state.screen, before);
        assert_eq!(state.take_requests(), []);

        // The same goes for a second stream: the first one's end is not its own.
        state.play(&app(1, "Desktop"), None, Choices::default());
        state.take_requests();
        state.apply(Event::StreamEnded { attempt: 1, why: StreamEnd::AppClosed }, 6.0);
        assert!(matches!(&state.screen, Screen::Stream(view) if view.attempt == 2));
    }

    #[test]
    fn another_app_cannot_start_while_one_is_running_but_the_running_one_resumes() {
        let mut salon = host("Salon", true, Reachability::Online);
        salon.busy = true;
        salon.running_app = Some(1);
        let mut state = state_with_apps(&salon);

        state.play(&app(2, "Steam"), None, Choices::default());
        assert_eq!(state.take_requests(), []);
        let Screen::Apps(view) = &state.screen else { panic!("{:?}", state.screen) };
        let text = "Desktop is running on this PC. Close it to start Steam.";
        assert_eq!(view.notice, Some(Notice { text: text.into(), problem: true }));
        assert_eq!(view.running(), Some(&app(1, "Desktop")));

        // The stream is made for the screen the window is on.
        let screen = ScreenMode { width: 2560, height: 1440, refresh_hz: 144 };
        state.play(&app(1, "Desktop"), Some(screen), Choices::default());
        assert_eq!(
            state.take_requests(),
            [Request::Play {
                host: salon.id,
                app: 1,
                attempt: 1,
                screen: Some(screen),
                choices: Choices::default()
            }]
        );
    }

    #[test]
    fn closing_the_running_app_is_asked_once_and_a_failure_is_shown() {
        let mut salon = host("Salon", true, Reachability::Online);
        salon.running_app = Some(1);
        let mut state = state_with_apps(&salon);

        state.quit_app();
        state.quit_app();
        assert_eq!(state.take_requests(), [Request::Quit(salon.id.clone())]);
        // Nothing starts while the app is closing.
        state.play(&app(1, "Desktop"), None, Choices::default());
        assert_eq!(state.take_requests(), []);

        let result = Err("The PC did not answer.".to_owned());
        state.apply(Event::QuitEnded { host: salon.id.clone(), result }, 4.0);
        let Screen::Apps(view) = &state.screen else { panic!("{:?}", state.screen) };
        assert!(!view.quitting);
        assert_eq!(view.notice.as_ref().map(|notice| notice.problem), Some(true));

        // Closed: the PC says it runs nothing, and any app can start.
        state.quit_app();
        salon.running_app = None;
        state.apply(Event::Hosts(vec![salon.clone()]), 5.0);
        state.apply(Event::QuitEnded { host: salon.id.clone(), result: Ok(()) }, 5.0);
        state.take_requests();
        state.play(&app(2, "Steam"), None, Choices::default());
        assert_eq!(
            state.take_requests(),
            [Request::Play {
                host: salon.id,
                app: 2,
                attempt: 1,
                screen: None,
                choices: Choices::default()
            }]
        );
    }

    #[test]
    fn going_back_to_the_pcs_from_a_stream_leaves_it() {
        let salon = host("Salon", true, Reachability::Online);
        let mut state = state_with_apps(&salon);
        state.play(&app(1, "Desktop"), None, Choices::default());
        state.take_requests();

        state.back(8.0);
        assert_eq!(state.screen, Screen::Pcs);
        assert_eq!(state.take_requests(), [Request::Leave, Request::Search]);
    }

    fn devices(state: &State) -> &Devices {
        let Screen::Devices(devices) = &state.screen else { panic!("{:?}", state.screen) };
        devices
    }

    #[test]
    fn the_camera_and_the_microphone_are_tried_out_one_by_one_and_off_at_every_visit() {
        let mut state = state_with(vec![host("Salon", true, Reachability::Online)]);
        // Nothing to switch anywhere else.
        state.switch_device(Device::Camera);
        assert_eq!(state.screen, Screen::Pcs);

        state.check_devices();
        assert_eq!(devices(&state), &Devices { camera: Trial::Off, microphone: Trial::Off });
        state.switch_device(Device::Camera);
        assert_eq!(devices(&state), &Devices { camera: Trial::On, microphone: Trial::Off });
        state.switch_device(Device::Microphone);
        state.switch_device(Device::Camera);
        assert_eq!(devices(&state), &Devices { camera: Trial::Off, microphone: Trial::On });
        // Trying a device asks nothing of any PC.
        assert_eq!(state.take_requests(), []);

        state.back(9.0);
        assert_eq!(state.screen, Screen::Pcs);
        assert_eq!(state.take_requests(), [Request::Search]);
        state.check_devices();
        assert_eq!(devices(&state), &Devices::default());
    }

    #[test]
    fn a_device_that_cannot_be_tried_says_why_and_can_be_tried_again() {
        let mut state = state_with(Vec::new());
        state.check_devices();

        // One that is off has nothing to fail.
        state.device_failed(Device::Camera, "Late news.".into());
        assert_eq!(devices(&state).camera, Trial::Off);

        state.switch_device(Device::Camera);
        state.switch_device(Device::Microphone);
        state.device_failed(Device::Camera, "This device has no camera.".into());
        assert_eq!(devices(&state).camera, Trial::Failed("This device has no camera.".into()));
        // The other one is none the worse for it.
        assert_eq!(devices(&state).microphone, Trial::On);

        state.switch_device(Device::Camera);
        assert_eq!(devices(&state).camera, Trial::On);
    }

    #[test]
    fn the_list_of_pcs_stays_fresh_behind_the_screen_that_tries_devices() {
        let salon = host("Salon", true, Reachability::Online);
        let mut state = state_with(Vec::new());
        state.check_devices();
        state.switch_device(Device::Camera);

        // No search while the list is not on screen, and news of PCs
        // changes nothing here.
        state.tick(100.0);
        assert_eq!(state.take_requests(), []);
        state.apply(Event::Hosts(vec![salon.clone()]), 100.0);
        assert_eq!(devices(&state).camera, Trial::On);
        assert_eq!(state.hosts, [salon]);
    }

    fn tunnel() -> TunnelSummary {
        TunnelSummary {
            address: "10.8.0.4".to_owned(),
            endpoint: Some("home.example.org:51820".to_owned()),
            covers: vec!["10.8.0.0/24".to_owned()],
            public_key: "bGV0bw==".to_owned(),
            running: false,
            handshake: None,
            problem: None,
        }
    }

    #[test]
    fn the_tunnel_is_looked_at_now_and_then_while_the_settings_show_it() {
        let mut state = state_with(Vec::new());
        state.watch_tunnel(1.0);
        state.watch_tunnel(1.5);
        assert_eq!(state.take_requests(), [Request::Tunnel]);
        assert!(!state.tunnel.known);

        state.apply(Event::Tunnel { summary: Some(tunnel()), refused: None }, 1.6);
        assert!(state.tunnel.known);
        assert_eq!(state.tunnel.summary, Some(tunnel()));

        state.watch_tunnel(1.0 + TUNNEL_EVERY);
        assert_eq!(state.take_requests(), [Request::Tunnel]);
    }

    #[test]
    fn a_wireguard_file_is_given_once_and_leaves_the_window_when_it_is_used() {
        let mut state = state_with(Vec::new());
        state.tunnel.open = true;
        // Nothing to give yet.
        state.use_tunnel();
        assert_eq!(state.take_requests(), []);

        state.tunnel.draft = WireGuardFile("[Interface]".to_owned());
        state.use_tunnel();
        state.use_tunnel();
        assert_eq!(
            state.take_requests(),
            [Request::UseTunnel(WireGuardFile("[Interface]".to_owned()))]
        );
        assert!(state.tunnel.busy);

        // A look that was on its way answers first: the form stays as it is.
        state.tunnel.busy = false;
        state.apply(Event::Tunnel { summary: None, refused: None }, 2.0);
        assert!(state.tunnel.open);
        assert_eq!(state.tunnel.draft, WireGuardFile("[Interface]".to_owned()));

        state.tunnel.busy = true;
        state.apply(Event::Tunnel { summary: Some(tunnel()), refused: None }, 2.1);
        assert!(!state.tunnel.busy);
        assert!(!state.tunnel.open);
        assert_eq!(state.tunnel.draft, WireGuardFile::default());
        assert_eq!(state.tunnel.summary, Some(tunnel()));
    }

    #[test]
    fn a_file_that_is_refused_stays_in_the_form_with_the_reason() {
        let mut state = state_with(Vec::new());
        state.tunnel.open = true;
        state.tunnel.draft = WireGuardFile("hello".to_owned());
        state.use_tunnel();
        let why = "This is not a WireGuard file.".to_owned();
        state.apply(Event::Tunnel { summary: None, refused: Some(why.clone()) }, 2.0);

        assert!(state.tunnel.open);
        assert!(!state.tunnel.busy);
        assert_eq!(state.tunnel.refused, Some(why));
        assert_eq!(state.tunnel.draft, WireGuardFile("hello".to_owned()));
    }

    #[test]
    fn the_tunnel_is_removed_once_however_often_it_is_asked() {
        let mut state = state_with(Vec::new());
        state.apply(Event::Tunnel { summary: Some(tunnel()), refused: None }, 1.0);
        state.remove_tunnel();
        state.remove_tunnel();
        assert_eq!(state.take_requests(), [Request::RemoveTunnel]);

        state.apply(Event::Tunnel { summary: None, refused: None }, 2.0);
        assert!(!state.tunnel.busy);
        assert_eq!(state.tunnel.summary, None);
    }
}
