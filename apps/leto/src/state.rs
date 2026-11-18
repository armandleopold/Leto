//! What the app shows and how it reacts, with no drawing and no networking.
//!
//! The interface calls the methods of [`State`] when the person does
//! something, the backend's [`Event`]s are fed to [`State::apply`], and the
//! work the backend has to do comes out of [`State::take_requests`].

use leto_core::model::{AppSummary, HostId, HostSummary, Pin, Reachability};

/// Seconds between two searches while the list of PCs is on screen.
const SEARCH_EVERY: f64 = 15.0;

/// Work for the backend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    /// Check the saved PCs and look for others on the network.
    Search,
    /// Add the PC at this address.
    Add(String),
    Pair {
        host: HostId,
        attempt: u64,
    },
    /// Load the apps of a PC, then their covers.
    Apps(HostId),
    Forget(HostId),
}

/// News from the backend.
#[derive(Debug)]
pub enum Event {
    /// The list of PCs changed.
    Hosts(Vec<HostSummary>),
    /// A search ended. `network_searched` is false when this device could
    /// not listen for PCs announcing themselves.
    SearchFinished {
        network_searched: bool,
    },
    Added(Result<HostSummary, String>),
    /// The PIN the person has to type on the PC.
    PinChosen {
        attempt: u64,
        pin: Pin,
    },
    PairingEnded {
        attempt: u64,
        result: Result<HostSummary, String>,
    },
    Apps {
        host: HostId,
        result: Result<Vec<AppSummary>, AppsProblem>,
    },
    /// The cover of an app, or `None` when the PC has no usable picture.
    Cover {
        host: HostId,
        app: u32,
        image: Option<egui::ColorImage>,
    },
    /// Something the person should know that belongs to no screen.
    Notice(String),
}

/// Why the apps of a PC could not be listed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppsProblem {
    pub message: String,
    /// The PC no longer knows this device: pairing again fixes it.
    pub needs_pairing: bool,
}

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
    Pairing(Pairing),
    Apps(AppsView),
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
        if host.reachability == Reachability::Offline {
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
        self.requests.push(Request::Apps(host.id.clone()));
        self.screen = Screen::Apps(AppsView { host, apps: Loadable::Loading });
    }

    /// Asks again for the apps on screen.
    pub fn reload_apps(&mut self) {
        if let Screen::Apps(view) = &self.screen {
            self.show_apps(view.host.clone());
        }
    }

    /// Back to the list of PCs, from any screen.
    pub fn back(&mut self, now: f64) {
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
            Screen::Pcs => None,
            Screen::Pairing(pairing) => Some(&pairing.host.id),
            Screen::Apps(view) => Some(&view.host.id),
        };
        if shown == Some(id) {
            self.screen = Screen::Pcs;
        }
    }

    /// Takes in news from the backend. `now` is the app time in seconds.
    pub fn apply(&mut self, event: Event, now: f64) {
        match event {
            Event::Hosts(hosts) => {
                // The screens about one PC follow what is known about it.
                let shown = match &mut self.screen {
                    Screen::Pcs => None,
                    Screen::Pairing(pairing) => Some(&mut pairing.host),
                    Screen::Apps(view) => Some(&mut view.host),
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
            // Covers are pictures: the drawing side keeps them.
            Event::Cover { .. } => {}
            Event::Notice(message) => self.notice = Some(message),
        }
    }
}

#[cfg(test)]
mod tests {
    use leto_core::model::HostKind;

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
        assert_eq!(state.screen, Screen::Apps(AppsView { host: paired, apps: Loadable::Loading }));

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
    fn choosing_a_pc_that_is_off_searches_again() {
        let salon = host("Salon", true, Reachability::Offline);
        let mut state = state_with(vec![salon.clone()]);
        state.open(&salon.id, 1.0);
        assert_eq!(state.screen, Screen::Pcs);
        assert_eq!(state.take_requests(), [Request::Search]);
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
}
