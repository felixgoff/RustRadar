//! KeysDialog (doc 06 §20; `KeysDialog.svelte`, `keys.ts`): the API keys and the Flightradar24
//! sign-in, and the keys' file (`core::keys_file`).
//!
//! A modal dialog ([`super::shortcuts::dialog`]: the backdrop, the centred card, the 180 ms
//! entry, Escape and a click on the backdrop closing it), shown while
//! `state.shortcuts.keys_open()` (the map controls' key button, the settings menu, a panel's
//! sign-in prompt). Its close (the close button, the backdrop, Escape) is
//! `Action::Open(Open::Keys, false)`, which asks the Trafikverket key again (`closeKeys`). While it
//! is open the page's keys are off (`Shortcuts::shortcuts_off`, which the window hands to the map's
//! `KeyGate`) and the map takes no pointer (the backdrop is over it).
//!
//! **Layout** (440 wide, no wider than the window less 32, at most the window's height less 32 of
//! content; padding `12px 16px 16px`, 12 between the parts):
//! 1. the header: `API keys` (14 px at 650) and the close button (`X` 16/1.75);
//! 2. the **Flightradar24** fieldset: its legend (13 px at 650, with the good badge `Signed in`
//!    8 px after it while the session says so) standing on the 1 px `--line` rule that runs from
//!    8 px after it to the right edge, 12 px under it the note (12 px `--text-2`), the fields
//!    `Email` and `Password` (label 12 px at 600 `--text-2`, 4 px over a 36 px [`TextField`]) and,
//!    signed in, the `Sign out` button; 8 apart;
//! 3. the **Trafikverket** fieldset: the note with `api.trafikinfo.trafikverket.se` as a link
//!    (`--amber`, underlined; opened in the system browser) and the field `API key`;
//! 4. the status line (12 px, `--live`, or `--danger` for an error; at least 14.4 px high);
//! 5. the actions: the primary button (`Save and sign in` when the form holds an email and a
//!    password and either differs from what is saved, else `Save`; the spinner in it while busy;
//!    disabled, at half opacity, while busy or with nothing changed), then at the right `Import…`
//!    and `Export` (disabled while busy);
//! 6. the checkbox `Include the Flightradar24 login in the export` (Chromium's dark box at 4, 3 in
//!    its 19 px row, the word 8 px after its margin; the whole row toggles it);
//! 7. the fine print (11 px `--text-3`).
//!
//! **Behaviour** ([`Dialog`], ported from the component's script in its order):
//! - opening clears the status and loads the keys into what is saved and the form; opened from a
//!   sign-in prompt ([`prompt_sign_in`]) the email field then takes the keyboard, and the prompt
//!   gets it back on closing;
//! - **Save** (the primary button, or Enter in a field while it is enabled: the form's implicit
//!   submission): saves the form; when the login changed and both its parts are set, signs in
//!   ([`Action::Session`] with the answer: the session, `refreshSoon()`, the selected flight's
//!   stream again) and says `Saved, signed in to Flightradar24`, else `Saved`; any error its
//!   message without a leading `Error: `, in red;
//! - **Sign out**: signs out ([`Action::Session`]), then saves the keys with an empty password (the
//!   next launch does not sign in again) and empties the form's password: `Signed out`;
//! - **Export**: `rustradar-keys.json` ([`keys_file`] of the form, with the login only when the
//!   box is ticked) into Downloads through the backend (`save_export`), then shown in the file
//!   manager: `Saved {file} to Downloads`, or `Couldn't export: {message}`;
//! - **Import…**: a `.json` file from the system's file dialog ([`read_keys_file`] over the form):
//!   saved at once with `Imported {n} key|keys` (and the sign-in when the login changed), or the
//!   file's error in red.
//!
//! Where the asks go ([`KeysFeeds`]): the backend (`keys`, `save_keys`, `sign_in`, `sign_out`,
//! `save_export`, on its runtime) with the desktop's file dialog (`rfd`); without a backend the keys
//! live in memory for the session and the rest answers [`NO_BACKEND`]; tests install their own
//! ([`install_feeds`]). The asks are polled every pass, the dialog open or not, as the component's
//! promises run on after it closes.
//!
//! **Android**: each field reports its text to the soft keyboard's bridge every pass it is shown
//! (`platform::android::text::report`).
//!
//! **Checked** against Chromium's layout of today's component (server-rendered by Svelte, opened
//! with `showModal()`, laid out with app.css at 1440 × 900 and 390 × 844, signed out and in, with a
//! status, busy; `tests/dialogs.rs`): signed out every edge (card, rules, fields, buttons, the
//! checkbox) lands on Chromium's pixels, signed in or with a status within one (line boxes of
//! 13 × 1.4 px here, floored to 1/64 px there); text, a paragraph's rows and the link's underline
//! stand on Chromium's baselines (`ui::text`'s hook, F1).
//!
//! Deviations:
//! - The dialog's initial keyboard focus is nowhere (Chromium: the close button), except the email
//!   field from a sign-in prompt (see [`super::shortcuts`]).
//! - Phones have no file dialog in the first native release (plan 1.3): `Import…` is not shown
//!   there.
//! - Revealing the exported file in the file manager is the desktop platform glue's (item U20):
//!   until `platform::desktop` offers it, the file is saved and announced but not shown.
//! - Without a backend (it did not start) the keys are kept in memory only (today's plain-browser
//!   page kept them in `localStorage`), and signing in, signing out, exporting and importing answer
//!   [`NO_BACKEND`].
//! - Disabled fields (while busy) look as they do enabled (app.css overrides Chromium's disabled
//!   look) and drop the keyboard; what is typed meanwhile is not taken.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use egui::{Color32, Context, CornerRadius, CursorIcon, Id, Key, Pos2, Rect, Sense, Shape, Stroke, StrokeKind, Ui, Vec2};
use rustradar_backend::Backend;
use rustradar_core::keys_file::{Keys, keys_file, read_keys_file};
use rustradar_core::model::SessionInfo;

use super::filters::aircraft::{CHECKBOX, CHECKBOX_BORDER, CHECKBOX_FILL, CHECKBOX_RADIUS, check_points};
use super::shortcuts::{self, Return};
use super::text::{self, Style};
use super::widgets::badge::{Badge, Tone};
use super::widgets::button::{self, Button, focus_ring};
use super::widgets::text_field::TextField;
use super::theme;
use crate::requests::Requests;
use crate::state::aircraft::Answer;
use crate::state::shortcuts::Open;
use crate::state::{Action, AppState};

/// The dialog's heading.
pub const TITLE: &str = "API keys";
/// The Flightradar24 fieldset: legend, badge, note, fields, button.
pub const FR24: &str = "Flightradar24";
pub const SIGNED_IN: &str = "Signed in";
pub const FR24_NOTE: &str = "Optional. A subscription unlocks filed flight plans, vertical speed, squawk and airspace, depending on your plan.";
pub const EMAIL: &str = "Email";
pub const PASSWORD: &str = "Password";
pub const SIGN_OUT: &str = "Sign out";
/// The Trafikverket fieldset: legend, note (its text, the link, the full stop), field.
pub const TRAFIKVERKET: &str = "Trafikverket";
pub const TRAFIKVERKET_NOTE: &str = "Every train in Sweden, SJ included. Free key from ";
pub const TRAFIKVERKET_LINK: &str = "api.trafikinfo.trafikverket.se";
pub const TRAFIKVERKET_URL: &str = "https://api.trafikinfo.trafikverket.se/";
pub const TRAFIKVERKET_NOTE_END: &str = ".";
pub const API_KEY: &str = "API key";
/// The actions.
pub const SAVE: &str = "Save";
pub const SAVE_AND_SIGN_IN: &str = "Save and sign in";
pub const IMPORT: &str = "Import…";
pub const EXPORT: &str = "Export";
/// The checkbox's word and the fine print.
pub const WITH_LOGIN: &str = "Include the Flightradar24 login in the export";
pub const FINE_PRINT: &str = "Kept on this device in plain text. An exported file holds the keys as they are: share it only with people you'd give the keys to.";
/// The exported file's name.
pub const FILE_NAME: &str = "rustradar-keys.json";
/// The statuses.
pub const SAVED: &str = "Saved";
pub const SIGNED_IN_SUFFIX: &str = ", signed in to Flightradar24";
pub const SIGNED_OUT: &str = "Signed out";
/// What the asks answer without a backend.
pub const NO_BACKEND: &str = "Not available: the app's backend didn't start.";

/// The content's width (`width: 440px`).
pub const WIDTH: f32 = 440.0;
/// What the window's height leaves the content (`max-height: calc(100dvh - 32px)`).
pub const MAX_HEIGHT_LESS: f32 = 32.0;
/// Between the content's parts (`.content { gap }`), and between a fieldset's (`fieldset { gap }`).
pub const GAP: f32 = theme::SPACE_3;
pub const FIELDSET_GAP: f32 = theme::SPACE_2;
/// Under the legend, before the fieldset's first part (`padding-top`).
pub const LEGEND_PAD: f32 = theme::SPACE_3;
/// After the legend's text (its `gap` to the badge) and after the legend (its `padding-right`,
/// where the rule starts).
pub const LEGEND_GAP: f32 = theme::SPACE_2;
/// Between a field's label and its input (`.field { gap }`).
pub const LABEL_GAP: f32 = theme::SPACE_1;
/// The status line's least height (`min-height: 1.2em` at 12 px).
pub const STATUS_MIN: f32 = 1.2 * theme::TEXT_SM;
/// Between the actions (`.actions { gap }`).
pub const ACTIONS_GAP: f32 = theme::SPACE_2;
/// The checkbox's row: the native box's margins (`3px 3px 3px 4px`), the gap to its word.
pub const CHECK_MARGIN: [f32; 4] = [3.0, 3.0, 3.0, 4.0];
pub const CHECK_GAP: f32 = theme::SPACE_2;
/// The link's underline: its top under the baseline, its thickness (Overpass's metrics at 12 px).
pub const UNDERLINE_OFFSET: f32 = 1.5;
pub const UNDERLINE_WIDTH: f32 = 1.0;
/// The primary button's opacity while disabled.
pub const PRIMARY_DISABLED_OPACITY: f32 = 0.5;

/// The legend (`--text-md` at 650).
pub const fn legend_style() -> Style {
    Style::ui(theme::TEXT_MD, theme::WEIGHT_BADGE, theme::TEXT_1)
}

/// A note (`.note`: 12 px `--text-2`).
pub const fn note_style() -> Style {
    Style::ui(theme::TEXT_SM, theme::WEIGHT_REGULAR, theme::TEXT_2)
}

/// A field's label (`.field`: 12 px at 600 `--text-2`).
pub const fn label_style() -> Style {
    Style::ui(theme::TEXT_SM, 600, theme::TEXT_2)
}

/// The status line (`.status`: 12 px, `--live` or `--danger`).
pub const fn status_style(error: bool) -> Style {
    Style::ui(theme::TEXT_SM, theme::WEIGHT_REGULAR, if error { theme::DANGER } else { theme::LIVE })
}

/// The checkbox's word (`.check`: 12 px `--text-2`).
pub const fn check_style() -> Style {
    Style::ui(theme::TEXT_SM, theme::WEIGHT_REGULAR, theme::TEXT_2)
}

/// The fine print (`.fine`: 11 px `--text-3`).
pub const fn fine_style() -> Style {
    Style::ui(theme::TEXT_XS, theme::WEIGHT_REGULAR, theme::TEXT_3)
}

/// `message(e)`: an error's text without a leading `Error:` and the white space after it.
pub fn message(e: &str) -> String {
    match e.strip_prefix("Error:") {
        Some(rest) => rest.trim_start_matches(js_space).to_owned(),
        None => e.to_owned(),
    }
}

/// JavaScript's `\s`: Unicode's white space and U+FEFF, without U+0085.
fn js_space(c: char) -> bool {
    c == '\u{feff}' || (c != '\u{85}' && c.is_whitespace())
}

/// `Imported {n} key|keys`.
pub fn imported(count: usize) -> String {
    format!("Imported {count} {}", if count == 1 { "key" } else { "keys" })
}

/// `Saved {file} to Downloads` for the file written at `path` (its last part, `\` or `/` apart).
pub fn saved_to(path: &Path) -> String {
    let path = path.to_string_lossy();
    format!("Saved {} to Downloads", path.rsplit(['/', '\\']).next().unwrap_or_default())
}

/// `Couldn't export: {message}`.
pub fn couldnt_export(e: &str) -> String {
    format!("Couldn't export: {}", message(e))
}

// ------------------------------------------------------------------------------------------------
// where the asks go

/// A file the file dialog gave: its text, or why it could not be read; `None` when cancelled.
pub type Picked = Option<Result<String, String>>;

/// Where the dialog's asks go: the backend, memory, or a test's.
pub trait KeysFeeds: Send {
    /// `loadKeys()` (`get_keys`).
    fn load(&mut self) -> Answer<Keys>;
    /// `saveKeys(keys)` (`save_keys`): the keys as stored (trimmed).
    fn save(&mut self, keys: Keys) -> Answer<Result<Keys, String>>;
    /// `signIn(email, password)`.
    fn sign_in(&mut self, email: String, password: String) -> Answer<Result<SessionInfo, String>>;
    /// `signOut()`.
    fn sign_out(&mut self) -> Answer<Result<SessionInfo, String>>;
    /// `save_export({fileName, contents})`: where the file went.
    fn export(&mut self, file_name: &str, contents: String) -> Answer<Result<PathBuf, String>>;
    /// `revealItemInDir(path)`, failures ignored.
    fn reveal(&mut self, path: &Path);
    /// The file input's picker (`accept=".json,application/json"`); `None` where there is none.
    fn pick(&mut self) -> Option<Answer<Picked>>;
}

/// Runs `job` on the runtime's blocking threads (file I/O).
async fn blocking<T: Send + 'static>(job: impl FnOnce() -> Result<T, String> + Send + 'static) -> Result<T, String> {
    tokio::task::spawn_blocking(job).await.unwrap_or_else(|e| Err(e.to_string()))
}

/// The backend's asks, on its runtime.
struct BackendFeeds {
    backend: Backend,
    requests: Requests,
}

impl KeysFeeds for BackendFeeds {
    fn load(&mut self) -> Answer<Keys> {
        // a lock read in the backend
        Answer::ready(self.backend.keys())
    }

    fn save(&mut self, keys: Keys) -> Answer<Result<Keys, String>> {
        let backend = self.backend.clone();
        self.requests.request(blocking(move || backend.save_keys(keys))).into()
    }

    fn sign_in(&mut self, email: String, password: String) -> Answer<Result<SessionInfo, String>> {
        let backend = self.backend.clone();
        self.requests.request(async move { backend.sign_in(&email, password).await }).into()
    }

    fn sign_out(&mut self) -> Answer<Result<SessionInfo, String>> {
        // swaps the client for a copy without the session: nothing to wait for
        Answer::ready(Ok(self.backend.sign_out()))
    }

    fn export(&mut self, file_name: &str, contents: String) -> Answer<Result<PathBuf, String>> {
        let (backend, file_name) = (self.backend.clone(), file_name.to_owned());
        self.requests.request(blocking(move || backend.save_export(&file_name, &contents))).into()
    }

    fn reveal(&mut self, path: &Path) {
        // the file manager, through the desktop glue (U20)
        log::info!("exported {}", path.display());
        crate::platform::reveal(path);
    }

    fn pick(&mut self) -> Option<Answer<Picked>> {
        pick_file(&self.requests)
    }
}

/// The desktop's file dialog for a `.json` file, its text read on the runtime.
#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
fn pick_file(requests: &Requests) -> Option<Answer<Picked>> {
    let chosen = rfd::AsyncFileDialog::new().add_filter("JSON", &["json"]).pick_file();
    Some(
        requests
            .request(async move {
                let file = chosen.await?;
                Some(Ok(String::from_utf8_lossy(&file.read().await).into_owned()))
            })
            .into(),
    )
}

/// Phones have no file dialog in the first native release.
#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
fn pick_file(_requests: &Requests) -> Option<Answer<Picked>> {
    None
}

/// Without a backend: the keys in memory for the session, the rest [`NO_BACKEND`].
#[derive(Default)]
struct MemoryFeeds {
    keys: Keys,
}

impl KeysFeeds for MemoryFeeds {
    fn load(&mut self) -> Answer<Keys> {
        Answer::ready(self.keys.clone())
    }

    fn save(&mut self, keys: Keys) -> Answer<Result<Keys, String>> {
        self.keys = keys.clone();
        Answer::ready(Ok(keys))
    }

    fn sign_in(&mut self, _email: String, _password: String) -> Answer<Result<SessionInfo, String>> {
        Answer::ready(Err(NO_BACKEND.into()))
    }

    fn sign_out(&mut self) -> Answer<Result<SessionInfo, String>> {
        Answer::ready(Err(NO_BACKEND.into()))
    }

    fn export(&mut self, _file_name: &str, _contents: String) -> Answer<Result<PathBuf, String>> {
        Answer::ready(Err(NO_BACKEND.into()))
    }

    fn reveal(&mut self, _path: &Path) {}

    fn pick(&mut self) -> Option<Answer<Picked>> {
        (!crate::platform::PHONE).then(|| Answer::ready(Some(Err(NO_BACKEND.into()))))
    }
}

/// The feeds for `state`: the backend's, else memory.
fn feeds_for(state: &AppState) -> Box<dyn KeysFeeds> {
    match (&state.backend, &state.requests) {
        (Some(backend), Some(requests)) => Box::new(BackendFeeds { backend: backend.clone(), requests: requests.clone() }),
        _ => Box::new(MemoryFeeds::default()),
    }
}

// ------------------------------------------------------------------------------------------------
// the dialog's state

/// The status line (`status`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    pub text: String,
    pub error: bool,
}

impl Status {
    fn ok(text: impl Into<String>) -> Option<Status> {
        Some(Status { text: text.into(), error: false })
    }

    fn error(text: impl Into<String>) -> Option<Status> {
        Some(Status { text: text.into(), error: true })
    }
}

/// An ask the dialog waits on, with what follows its answer.
enum Job {
    /// `save(next, done)`: the keys saved; then the sign-in when the login changed.
    Save { ask: Answer<Result<Keys, String>>, login_changed: bool, done: String },
    /// The sign-in after a save.
    SignIn { ask: Answer<Result<SessionInfo, String>>, done: String },
    /// `signOut()`: the sign-out; then the keys saved without the password.
    SignOut { ask: Answer<Result<SessionInfo, String>> },
    SignedOutSave { ask: Answer<Result<Keys, String>> },
}

/// The component's state (see the module documentation), kept by egui between passes.
#[derive(Default)]
pub struct Dialog {
    /// Chosen at the first pass ([`feeds_for`]) unless a test installed some.
    feeds: Option<Box<dyn KeysFeeds>>,
    /// As saved; the form is compared against it.
    pub saved: Keys,
    pub form: Keys,
    /// `withLogin`: the checkbox.
    pub with_login: bool,
    pub busy: bool,
    pub status: Option<Status>,
    load: Option<Answer<Keys>>,
    jobs: Vec<Job>,
    exports: Vec<Answer<Result<PathBuf, String>>>,
    picks: Vec<Answer<Picked>>,
    /// Opened from a sign-in prompt (`focusLogin`), and where the keyboard goes back on closing.
    prompt: Option<Prompt>,
    /// The email field takes the keyboard at the next pass (after the keys loaded).
    focus_email: bool,
}

/// A sign-in prompt that opened the dialog.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Prompt {
    /// The prompt's widget, which gets the keyboard back when the dialog closes.
    back: Option<Id>,
    /// Taken by the dialog when it opened.
    taken: bool,
}

impl std::fmt::Debug for Dialog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Dialog")
            .field("saved", &self.saved)
            .field("form", &self.form)
            .field("with_login", &self.with_login)
            .field("busy", &self.busy)
            .field("status", &self.status)
            .field("jobs", &self.jobs.len())
            .finish_non_exhaustive()
    }
}

impl Dialog {
    fn feeds(&mut self) -> &mut dyn KeysFeeds {
        self.feeds.get_or_insert_with(|| Box::new(MemoryFeeds::default())).as_mut()
    }

    /// `changed`: any field of the form differs from what is saved.
    pub fn changed(&self) -> bool {
        self.form != self.saved
    }

    /// Whether the form's login differs from the saved one.
    fn login_differs(&self, keys: &Keys) -> bool {
        keys.fr24_email != self.saved.fr24_email || keys.fr24_password != self.saved.fr24_password
    }

    /// The primary button's label: `Save and sign in` when the form has an email and a password
    /// (`hasLogin`) and either differs from what is saved, else `Save`.
    pub fn primary_label(&self) -> &'static str {
        let has_login = !self.form.fr24_email.is_empty() && !self.form.fr24_password.is_empty();
        if has_login && self.login_differs(&self.form) { SAVE_AND_SIGN_IN } else { SAVE }
    }

    /// Whether the primary button (and the form's submission) is enabled.
    pub fn can_save(&self) -> bool {
        !self.busy && self.changed()
    }

    /// The dialog opens (the `$effect`): the status cleared, the keys asked for.
    pub fn open(&mut self) {
        self.status = None;
        self.load = Some(self.feeds().load());
    }

    /// `save(next, done)`.
    pub fn save(&mut self, next: Keys, done: String) {
        self.busy = true;
        self.status = None;
        let login_changed = self.login_differs(&next);
        let ask = self.feeds().save(next);
        self.jobs.push(Job::Save { ask, login_changed, done });
    }

    /// The form's submission: `save()` with the form, while the primary button is enabled.
    pub fn submit(&mut self) {
        if self.can_save() {
            self.save(self.form.clone(), SAVED.to_owned());
        }
    }

    /// `signOut()`.
    pub fn sign_out(&mut self) {
        self.busy = true;
        let ask = self.feeds().sign_out();
        self.jobs.push(Job::SignOut { ask });
    }

    /// `exportKeys()`.
    pub fn export(&mut self) {
        let contents = keys_file(&self.form, self.with_login);
        let ask = self.feeds().export(FILE_NAME, contents);
        self.exports.push(ask);
    }

    /// The `Import…` button: the file dialog, unless one is open already.
    pub fn import(&mut self) {
        if self.picks.is_empty()
            && let Some(ask) = self.feeds().pick()
        {
            self.picks.push(ask);
        }
    }

    /// `importKeys` with the file's text: read over the form, then saved.
    fn imported(&mut self, picked: Picked) {
        match picked {
            None => {}
            Some(Err(e)) => self.status = Status::error(message(&e)),
            Some(Ok(text)) => match read_keys_file(&text, &self.form) {
                Ok(read) => self.save(read.keys, imported(read.count)),
                Err(e) => self.status = Status::error(message(&e.to_string())),
            },
        }
    }

    /// The answers that came since the last pass, in the order the asks were made; actions for the
    /// state go to `actions`. Answers whether anything changed.
    pub fn poll(&mut self, actions: &mut Vec<Action>) -> bool {
        let mut changed = false;
        if let Some(keys) = self.load.as_ref().and_then(Answer::poll) {
            self.load = None;
            self.form = keys.clone();
            self.saved = keys;
            if self.prompt.is_some_and(|p| p.taken) {
                self.focus_email = true;
            }
            changed = true;
        }
        // an ask that follows an answer is looked at at once (an answer already there is taken
        // in the same pass, as a resolved promise's `await` goes on before the next frame)
        let mut i = 0;
        while i < self.jobs.len() {
            let Some(next) = self.step(i, actions) else {
                i += 1;
                continue;
            };
            changed = true;
            match next {
                Some(job) => self.jobs[i] = job,
                None => {
                    self.jobs.remove(i);
                }
            }
        }
        let mut answered = Vec::new();
        self.exports.retain(|ask| match ask.poll() {
            Some(answer) => {
                answered.push(answer);
                false
            }
            None => true,
        });
        for answer in answered {
            changed = true;
            match answer {
                Ok(path) => {
                    self.feeds().reveal(&path);
                    self.status = Status::ok(saved_to(&path));
                }
                Err(e) => self.status = Status::error(couldnt_export(&e)),
            }
        }
        let mut picked = Vec::new();
        self.picks.retain(|ask| match ask.poll() {
            Some(answer) => {
                picked.push(answer);
                false
            }
            None => true,
        });
        for answer in picked {
            changed = true;
            self.imported(answer);
        }
        changed
    }

    /// Job `i`'s answer if it came: `Some(Some(next))` when another ask follows, `Some(None)` when
    /// it is done, `None` while it waits.
    fn step(&mut self, i: usize, actions: &mut Vec<Action>) -> Option<Option<Job>> {
        let next = match &self.jobs[i] {
            Job::Save { ask, login_changed, done } => {
                let answer = ask.poll()?;
                let (login_changed, done) = (*login_changed, done.clone());
                match answer {
                    Ok(keys) => {
                        self.saved = keys;
                        self.form = self.saved.clone();
                        let (email, password) = (self.saved.fr24_email.clone(), self.saved.fr24_password.clone());
                        if login_changed && !email.is_empty() && !password.is_empty() {
                            let ask = self.feeds().sign_in(email, password);
                            return Some(Some(Job::SignIn { ask, done }));
                        }
                        self.status = Status::ok(done);
                    }
                    Err(e) => self.status = Status::error(message(&e)),
                }
                None
            }
            Job::SignIn { ask, done } => {
                let answer = ask.poll()?;
                match answer {
                    Ok(session) => {
                        actions.push(Action::Session(session));
                        self.status = Status::ok(format!("{done}{SIGNED_IN_SUFFIX}"));
                    }
                    Err(e) => self.status = Status::error(message(&e)),
                }
                None
            }
            Job::SignOut { ask } => {
                let answer = ask.poll()?;
                match answer {
                    Ok(session) => {
                        actions.push(Action::Session(session));
                        // signing out forgets the saved password too, or the next launch signs in again
                        let keys = Keys { fr24_password: String::new(), ..self.saved.clone() };
                        let ask = self.feeds().save(keys);
                        return Some(Some(Job::SignedOutSave { ask }));
                    }
                    Err(e) => self.status = Status::error(message(&e)),
                }
                None
            }
            Job::SignedOutSave { ask } => {
                let answer = ask.poll()?;
                match answer {
                    Ok(keys) => {
                        self.saved = keys;
                        self.form.fr24_password.clear();
                        self.status = Status::ok(SIGNED_OUT);
                    }
                    Err(e) => self.status = Status::error(message(&e)),
                }
                None
            }
        };
        // `finally { busy = false }`
        self.busy = false;
        Some(next)
    }
}

fn dialog_id() -> Id {
    Id::unique("rustradar::keys_dialog")
}

/// The dialog's state as `ctx` keeps it between passes.
pub fn dialog(ctx: &Context) -> Arc<Mutex<Dialog>> {
    ctx.data_mut(|d| d.get_temp_mut_or_insert_with(dialog_id().with("state"), || Arc::new(Mutex::new(Dialog::default()))).clone())
}

/// Makes the dialog ask `feeds` instead of the backend (tests).
pub fn install_feeds(ctx: &Context, feeds: Box<dyn KeysFeeds>) {
    let dialog = dialog(ctx);
    dialog.lock().unwrap_or_else(PoisonError::into_inner).feeds = Some(feeds);
}

/// A sign-in prompt is opening the dialog (`promptSignIn`): call it with the action that opens
/// it (`Action::Open(Open::Keys, true)`). The email field takes the keyboard once the keys have
/// loaded, and `back` (the prompt's widget) gets it back when the dialog closes.
pub fn prompt_sign_in(ctx: &Context, back: Option<Id>) {
    let dialog = dialog(ctx);
    dialog.lock().unwrap_or_else(PoisonError::into_inner).prompt = Some(Prompt { back, taken: false });
}

// ------------------------------------------------------------------------------------------------
// drawing

/// What the content asked for in a pass.
#[derive(Debug, Default)]
struct Asked {
    close: bool,
    submit: bool,
    sign_out: bool,
    import: bool,
    export: bool,
}

/// What [`ui`] drew.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placed {
    /// The card, border included.
    pub card: Rect,
    /// The three fields' frames: email, password, API key.
    pub fields: [Rect; 3],
    /// The primary button, `Import…` (nothing on phones), `Export`, `Sign out` (nothing signed
    /// out), the checkbox's row, the link's first row, the close button.
    pub primary: Rect,
    pub import: Rect,
    pub export: Rect,
    pub sign_out: Rect,
    pub check: Rect,
    pub link: Rect,
    pub close: Rect,
}

impl Default for Placed {
    fn default() -> Self {
        let none = Rect::NOTHING;
        Placed { card: none, fields: [none; 3], primary: none, import: none, export: none, sign_out: none, check: none, link: none, close: none }
    }
}

/// The UI pass's keys dialog for `state` (see the module documentation): its asks are polled,
/// and while it is open it is drawn; its close and the sessions it signs in or out go to
/// `actions`. Answers what it placed while drawn.
pub fn ui(ctx: &Context, state: &AppState, actions: &mut Vec<Action>) -> Option<Placed> {
    let open = state.shortcuts.keys_open();
    let dialog = dialog(ctx);
    let mut d = dialog.lock().unwrap_or_else(PoisonError::into_inner);
    if d.feeds.is_none() {
        d.feeds = Some(feeds_for(state));
    }
    match Return::track(ctx, dialog_id(), open) {
        Some(true) => {
            if let Some(prompt) = &mut d.prompt {
                prompt.taken = true;
            }
            d.open();
        }
        Some(false) => {
            // `closeKeys`: the keyboard back to the prompt that opened it
            if let Some(Prompt { back: Some(back), taken: true }) = d.prompt.take() {
                ctx.memory_mut(|m| m.request_focus(back));
            }
            d.focus_email = false;
        }
        None => {}
    }
    if d.poll(actions) {
        ctx.request_repaint();
    }
    if !open {
        return None;
    }
    let signed_in = state.aircraft.session().is_some_and(|s| s.authenticated);
    let mut asked = Asked::default();
    let mut placed = Placed::default();
    let max_height = ctx.content_rect().height() - MAX_HEIGHT_LESS;
    let shown = shortcuts::dialog(ctx, dialog_id(), WIDTH, max_height, |ui| {
        shortcuts::padding().show(ui, |ui| content(ui, &mut d, signed_in, &mut asked, &mut placed));
    });
    placed.card = shown.rect;
    if asked.close || shown.close {
        actions.push(Action::Open(Open::Keys, false));
    }
    if asked.submit {
        d.submit();
    }
    if asked.sign_out {
        d.sign_out();
    }
    if asked.import {
        d.import();
    }
    if asked.export {
        d.export();
    }
    if d.busy || !d.jobs.is_empty() {
        ctx.request_repaint();
    }
    Some(placed)
}

/// The dialog's content across `ui`'s width (the padding's inside).
fn content(ui: &mut Ui, d: &mut Dialog, signed_in: bool, asked: &mut Asked, placed: &mut Placed) {
    let width = ui.available_width();
    let close = shortcuts::header(ui, TITLE);
    asked.close |= close.clicked();
    placed.close = close.rect;
    ui.add_space(GAP);

    // Flightradar24
    legend(ui, FR24, signed_in.then_some(SIGNED_IN));
    ui.add_space(LEGEND_PAD);
    paragraph(ui, FR24_NOTE, &note_style(), width);
    ui.add_space(FIELDSET_GAP);
    let busy = d.busy;
    let email = field(ui, "email", EMAIL, &mut d.form.fr24_email, false, busy, asked);
    placed.fields[0] = email.0;
    if std::mem::take(&mut d.focus_email) {
        ui.memory_mut(|m| m.request_focus(email.1));
    }
    ui.add_space(FIELDSET_GAP);
    placed.fields[1] = field(ui, "password", PASSWORD, &mut d.form.fr24_password, true, busy, asked).0;
    if signed_in {
        ui.add_space(FIELDSET_GAP);
        let response = Button::text_only(SIGN_OUT).enabled(!busy).show(ui);
        placed.sign_out = response.rect;
        asked.sign_out |= response.clicked();
    }
    ui.add_space(GAP);

    // Trafikverket
    legend(ui, TRAFIKVERKET, None);
    ui.add_space(LEGEND_PAD);
    placed.link = note_with_link(ui, width);
    ui.add_space(FIELDSET_GAP);
    placed.fields[2] = field(ui, "trafikverket", API_KEY, &mut d.form.trafikverket, false, busy, asked).0;
    ui.add_space(GAP);

    // the status line
    match &d.status {
        Some(status) => paragraph(ui, &status.text, &status_style(status.error), width),
        None => {
            ui.allocate_exact_size(Vec2::new(width, STATUS_MIN), Sense::hover());
        }
    }
    ui.add_space(GAP);

    actions_row(ui, d, asked, placed);
    ui.add_space(GAP);
    placed.check = check_row(ui, &mut d.with_login);
    ui.add_space(GAP);
    paragraph(ui, FINE_PRINT, &fine_style(), width);
}

/// A wrapped paragraph `width` wide (its rows' line boxes high).
fn paragraph(ui: &mut Ui, text: &str, style: &Style, width: f32) {
    let galley = ui.painter().layout_job(text::wrapped(text, style, width));
    let rows = galley.rows.len().max(1) as f32;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, rows * style.line_px()), Sense::hover());
    if ui.is_rect_visible(rect) {
        ui.painter().galley(rect.min, galley, Color32::PLACEHOLDER);
    }
}

/// A fieldset's legend standing on the fieldset's top rule (see the module documentation).
fn legend(ui: &mut Ui, title: &str, badge: Option<&str>) {
    let width = ui.available_width();
    let style = legend_style();
    let galley = ui.painter().layout_job(text::job(title, &style));
    let badge = badge.map(|b| Badge::new(b, Tone::Good));
    let badge_size = badge.as_ref().map_or(Vec2::ZERO, |b| b.measure(ui.ctx()));
    // `display: flex; align-items: center`
    let height = style.line_px().max(badge_size.y);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
    let painter = ui.painter();
    let mut right = rect.left() + galley.size().x;
    painter.galley(Pos2::new(rect.left(), rect.top() + (height - style.line_px()) / 2.0), galley, Color32::PLACEHOLDER);
    if let Some(badge) = &badge {
        badge.paint(painter, Pos2::new(right + LEGEND_GAP, rect.top() + (height - badge_size.y) / 2.0));
        right += LEGEND_GAP + badge_size.x;
    }
    right += LEGEND_GAP;
    // the fieldset's border-top runs through the legend's middle
    let y = (rect.top() + (height - theme::BORDER) / 2.0).round();
    if right < rect.right() {
        painter.rect_filled(Rect::from_x_y_ranges(right..=rect.right(), y..=y + theme::BORDER), CornerRadius::ZERO, theme::LINE);
    }
}

/// A labelled field over `text` (a password's characters hidden); while `busy` it takes no
/// keyboard and no change. Enter in it submits the form. Answers its frame and its text edit's id.
fn field(ui: &mut Ui, salt: &str, label: &str, text: &mut String, password: bool, busy: bool, asked: &mut Asked) -> (Rect, Id) {
    let width = ui.available_width();
    let style = label_style();
    let galley = ui.painter().layout_job(text::job(label, &style));
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, style.line_px()), Sense::hover());
    ui.painter().galley(rect.min, galley, Color32::PLACEHOLDER);
    ui.add_space(LABEL_GAP);
    let mut edited = text.clone();
    let shown = TextField::new(shortcuts::id(("keys", salt)), &mut edited).password(password).width(width).show(ui);
    let id = shown.response.id;
    if busy {
        if shown.focused {
            ui.memory_mut(|m| m.surrender_focus(id));
        }
    } else {
        *text = edited;
        if shown.focused && ui.input(|i| i.key_pressed(Key::Enter)) {
            asked.submit = true;
        }
    }
    crate::platform::android::text::report(ui.ctx(), id, text);
    (shown.frame, id)
}

/// The Trafikverket note with its link (the link's rows clickable, underlined); answers the link's
/// first row.
fn note_with_link(ui: &mut Ui, width: f32) -> Rect {
    let style = note_style();
    let mut job = text::wrapped(TRAFIKVERKET_NOTE, &style, width);
    text::append(&mut job, TRAFIKVERKET_LINK, &style.color(theme::AMBER));
    text::append(&mut job, TRAFIKVERKET_NOTE_END, &style);
    // the link's characters (a glyph per character, a row's newline counting one)
    let start = TRAFIKVERKET_NOTE.chars().count();
    let chars = start..start + TRAFIKVERKET_LINK.chars().count();
    let galley = ui.painter().layout_job(job);
    let rows = galley.rows.len().max(1) as f32;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, rows * style.line_px()), Sense::hover());
    let mut link = Rect::NOTHING;
    let mut first = Rect::NOTHING;
    let mut offset = 0;
    for (i, row) in galley.rows.iter().enumerate() {
        let row_start = offset;
        offset += row.char_count_including_newline().0;
        let glyphs: Vec<_> = row.glyphs.iter().enumerate().filter(|(j, _)| chars.contains(&(row_start + j))).map(|(_, g)| g).collect();
        let (Some(a), Some(b)) = (glyphs.first(), glyphs.last()) else { continue };
        let left = rect.left() + row.pos.x + a.pos.x;
        let right = rect.left() + row.pos.x + b.pos.x + b.advance_width;
        let baseline = rect.top() + text::row_baseline(&galley, i);
        let top = (baseline + UNDERLINE_OFFSET).round();
        ui.painter().rect_filled(Rect::from_x_y_ranges(left..=right, top..=top + UNDERLINE_WIDTH), CornerRadius::ZERO, theme::AMBER);
        let row_rect = Rect::from_x_y_ranges(left..=right, rect.top() + row.pos.y..=rect.top() + row.pos.y + row.size.y);
        if first == Rect::NOTHING {
            first = row_rect;
        }
        link = link.union(row_rect);
    }
    ui.painter().galley(rect.min, galley, Color32::PLACEHOLDER);
    if link != Rect::NOTHING {
        let response = ui.interact(link, shortcuts::id("keys link"), Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
        if response.clicked() {
            ui.ctx().open_url(egui::OpenUrl::new_tab(TRAFIKVERKET_URL));
        }
        if response.has_focus() {
            focus_ring(ui.painter(), link, 0);
        }
    }
    first
}

/// The actions' row: the primary button at the left, `Import…` and `Export` at the right (on a
/// row of their own when the three do not fit, as `flex-wrap` wraps them).
fn actions_row(ui: &mut Ui, d: &Dialog, asked: &mut Asked, placed: &mut Placed) {
    let width = ui.available_width();
    let mut primary = Button::new();
    if d.busy {
        primary = primary.spinner(super::widgets::spinner::SIZE);
    }
    let primary = primary.text(d.primary_label()).primary().enabled(d.can_save()).dim_when_disabled(PRIMARY_DISABLED_OPACITY);
    let import = (!crate::platform::PHONE).then(|| Button::text_only(IMPORT).enabled(!d.busy));
    let export = Button::text_only(EXPORT).enabled(!d.busy);
    let ctx = ui.ctx().clone();
    let primary_w = primary.size(&ctx).x;
    let import_w = import.as_ref().map_or(0.0, |b| b.size(&ctx).x + ACTIONS_GAP);
    let export_w = export.size(&ctx).x;
    // the spacer's two gaps count while the items are laid out on one line
    let one_line = primary_w + 2.0 * ACTIONS_GAP + import_w + export_w <= width;
    let height = if one_line { button::HEIGHT } else { 2.0 * button::HEIGHT + ACTIONS_GAP };
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
    let row = Rect::from_min_size(rect.min, Vec2::new(width, button::HEIGHT));
    placed.primary = Rect::from_min_size(row.min, Vec2::new(primary_w, button::HEIGHT));
    let response = ui.put(placed.primary, primary);
    asked.submit |= response.clicked();
    let right = if one_line { row } else { row.translate(Vec2::new(0.0, button::HEIGHT + ACTIONS_GAP)) };
    placed.export = Rect::from_min_max(Pos2::new(right.right() - export_w, right.top()), right.max);
    asked.export |= ui.put(placed.export, export).clicked();
    if let Some(import) = import {
        placed.import = Rect::from_min_size(Pos2::new(placed.export.left() - import_w, right.top()), Vec2::new(import_w - ACTIONS_GAP, button::HEIGHT));
        asked.import |= ui.put(placed.import, import).clicked();
    }
}

/// The checkbox's row (Chromium's native box in its margins, its word 8 px after); the whole row
/// toggles it. Answers the row.
fn check_row(ui: &mut Ui, value: &mut bool) -> Rect {
    let width = ui.available_width();
    let [top, right, bottom, left] = CHECK_MARGIN;
    let height = top + CHECKBOX + bottom;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
    let response = ui.interact(rect, shortcuts::id("keys with login"), Sense::click()).on_hover_cursor(CursorIcon::Default);
    if response.clicked() {
        *value = !*value;
    }
    let painter = ui.painter();
    let check = Rect::from_min_size(Pos2::new(rect.left() + left, rect.top() + top), Vec2::splat(CHECKBOX));
    if *value {
        painter.rect_filled(check, CornerRadius::same(CHECKBOX_RADIUS), theme::AMBER);
        painter.add(Shape::line(check_points(check).to_vec(), Stroke::new(CHECKBOX * 0.16, CHECKBOX_FILL)));
    } else {
        painter.rect(check, CornerRadius::same(CHECKBOX_RADIUS), CHECKBOX_FILL, Stroke::new(theme::BORDER, CHECKBOX_BORDER), StrokeKind::Inside);
    }
    let style = check_style();
    let galley = painter.layout_job(text::job(WITH_LOGIN, &style));
    painter.galley(Pos2::new(check.right() + right + CHECK_GAP, rect.center().y - style.line_px() / 2.0), galley, Color32::PLACEHOLDER);
    if response.has_focus() {
        focus_ring(painter, check, CHECKBOX_RADIUS);
    }
    rect
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::{self, Receiver, Sender};

    use egui::{Event, PointerButton, RawInput};
    use rustradar_scene::camera::ViewState;

    use super::*;
    use crate::state::Startup;

    const SIZE: Vec2 = Vec2::new(1440.0, 900.0);

    fn keys(email: &str, password: &str, trafikverket: &str) -> Keys {
        Keys { fr24_email: email.into(), fr24_password: password.into(), trafikverket: trafikverket.into(), ..Default::default() }
    }

    /// What a fake was asked, and the senders that answer the asks it holds open.
    #[derive(Default)]
    struct Script {
        asked: Vec<String>,
        saves: Vec<Sender<Result<Keys, String>>>,
        sign_ins: Vec<Sender<Result<SessionInfo, String>>>,
        exports: Vec<Sender<Result<PathBuf, String>>>,
        picks: Vec<Sender<Picked>>,
        stored: Keys,
    }

    #[derive(Clone, Default)]
    struct Fake(Arc<Mutex<Script>>);

    impl Fake {
        fn asked(&self) -> Vec<String> {
            self.0.lock().unwrap().asked.clone()
        }

        fn script(&self) -> std::sync::MutexGuard<'_, Script> {
            self.0.lock().unwrap()
        }
    }

    fn channel<T>() -> (Sender<T>, Answer<T>) {
        let (tx, rx): (Sender<T>, Receiver<T>) = mpsc::channel();
        (tx, Answer::Channel(rx))
    }

    impl KeysFeeds for Fake {
        fn load(&mut self) -> Answer<Keys> {
            let mut s = self.script();
            s.asked.push("load".into());
            Answer::ready(s.stored.clone())
        }

        fn save(&mut self, keys: Keys) -> Answer<Result<Keys, String>> {
            let mut s = self.script();
            s.asked.push(format!("save {} {} {}", keys.fr24_email, keys.fr24_password, keys.trafikverket));
            let (tx, rx) = channel();
            s.saves.push(tx);
            rx
        }

        fn sign_in(&mut self, email: String, password: String) -> Answer<Result<SessionInfo, String>> {
            let mut s = self.script();
            s.asked.push(format!("sign in {email} {password}"));
            let (tx, rx) = channel();
            s.sign_ins.push(tx);
            rx
        }

        fn sign_out(&mut self) -> Answer<Result<SessionInfo, String>> {
            self.script().asked.push("sign out".into());
            Answer::ready(Ok(SessionInfo { authenticated: false, expires: None }))
        }

        fn export(&mut self, file_name: &str, contents: String) -> Answer<Result<PathBuf, String>> {
            let mut s = self.script();
            s.asked.push(format!("export {file_name} {contents}"));
            let (tx, rx) = channel();
            s.exports.push(tx);
            rx
        }

        fn reveal(&mut self, path: &Path) {
            self.script().asked.push(format!("reveal {}", path.display()));
        }

        fn pick(&mut self) -> Option<Answer<Picked>> {
            let mut s = self.script();
            s.asked.push("pick".into());
            let (tx, rx) = channel();
            s.picks.push(tx);
            Some(rx)
        }
    }

    fn with_fake() -> (Dialog, Fake) {
        let fake = Fake::default();
        let d = Dialog { feeds: Some(Box::new(fake.clone())), ..Default::default() };
        (d, fake)
    }

    fn poll(d: &mut Dialog) -> Vec<Action> {
        let mut actions = Vec::new();
        d.poll(&mut actions);
        actions
    }

    #[test]
    fn messages_lose_a_leading_error() {
        assert_eq!(message("Error: no route"), "no route");
        assert_eq!(message("Error:\u{a0}\tnope"), "nope");
        assert_eq!(message("Error:x"), "x");
        assert_eq!(message("An Error: kept"), "An Error: kept");
        assert_eq!(message("Error:\u{85}x"), "\u{85}x", "U+0085 is not white space in JavaScript");
        assert_eq!(imported(1), "Imported 1 key");
        assert_eq!(imported(3), "Imported 3 keys");
        assert_eq!(saved_to(Path::new("/home/me/Downloads/rustradar-keys (2).json")), "Saved rustradar-keys (2).json to Downloads");
        assert_eq!(saved_to(Path::new("C:\\Users\\me\\Downloads\\rustradar-keys.json")), "Saved rustradar-keys.json to Downloads");
        assert_eq!(couldnt_export("Error: No Downloads folder to save to"), "Couldn't export: No Downloads folder to save to");
    }

    #[test]
    fn opening_loads_the_keys_and_the_labels_follow_the_form() {
        let (mut d, fake) = with_fake();
        fake.script().stored = keys("me@example.com", "", "abc");
        d.status = Status::ok("old");
        d.open();
        assert_eq!(d.status, None, "the status is cleared");
        assert!(poll(&mut d).is_empty());
        assert_eq!(d.saved, keys("me@example.com", "", "abc"));
        assert_eq!(d.form, d.saved);
        assert!(!d.changed() && !d.can_save());
        assert_eq!(d.primary_label(), SAVE);
        d.form.fr24_password = "pw".into();
        assert!(d.can_save());
        assert_eq!(d.primary_label(), SAVE_AND_SIGN_IN, "an email and a password, the password changed");
        d.form.fr24_email.clear();
        assert_eq!(d.primary_label(), SAVE, "no email: no login");
        d.form = keys("me@example.com", "", "xyz");
        assert_eq!(d.primary_label(), SAVE, "only the Trafikverket key changed");
        assert!(d.can_save());
        d.busy = true;
        assert!(!d.can_save());
        assert_eq!(fake.asked(), ["load"]);
    }

    #[test]
    fn save_with_a_new_login_signs_in_and_hands_the_session_over() {
        let (mut d, fake) = with_fake();
        d.open();
        poll(&mut d);
        d.form = keys(" me@example.com", "pw", "");
        d.submit();
        assert!(d.busy);
        assert_eq!(fake.asked(), ["load", "save  me@example.com pw "]);
        // the backend trims what it stores
        fake.script().saves[0].send(Ok(keys("me@example.com", "pw", ""))).unwrap();
        assert!(poll(&mut d).is_empty());
        assert!(d.busy, "still signing in");
        assert_eq!(d.form, keys("me@example.com", "pw", ""));
        assert_eq!(fake.asked()[2], "sign in me@example.com pw");
        let session = SessionInfo { authenticated: true, expires: Some(1_800_000_000) };
        fake.script().sign_ins[0].send(Ok(session)).unwrap();
        assert_eq!(poll(&mut d), [Action::Session(session)]);
        assert!(!d.busy);
        assert_eq!(d.status, Status::ok("Saved, signed in to Flightradar24"));
    }

    #[test]
    fn errors_show_in_red_and_a_failed_sign_in_keeps_the_saved_keys() {
        let (mut d, fake) = with_fake();
        d.open();
        poll(&mut d);
        d.form = keys("me@example.com", "bad", "");
        d.submit();
        fake.script().saves[0].send(Ok(d.form.clone())).unwrap();
        poll(&mut d);
        fake.script().sign_ins[0].send(Err("Error: Sign-in failed: 401".into())).unwrap();
        assert!(poll(&mut d).is_empty(), "no session handed over");
        assert_eq!(d.status, Status::error("Sign-in failed: 401"));
        assert_eq!(d.saved, keys("me@example.com", "bad", ""), "saved all the same");
        assert!(!d.busy);
        // a save that fails
        d.form.trafikverket = "k".into();
        d.submit();
        assert_eq!(d.status, None, "cleared by the save");
        fake.script().saves[1].send(Err("Couldn't save /x/keys.json: denied".into())).unwrap();
        poll(&mut d);
        assert_eq!(d.status, Status::error("Couldn't save /x/keys.json: denied"));
        assert!(!d.busy);
        // nothing changed: no submission
        d.form = d.saved.clone();
        d.submit();
        assert_eq!(fake.script().saves.len(), 2);
    }

    #[test]
    fn a_save_without_a_login_change_says_saved() {
        let (mut d, fake) = with_fake();
        fake.script().stored = keys("me@example.com", "pw", "");
        d.open();
        poll(&mut d);
        d.form.trafikverket = "abc".into();
        d.submit();
        fake.script().saves[0].send(Ok(d.form.clone())).unwrap();
        assert!(poll(&mut d).is_empty());
        assert_eq!(d.status, Status::ok(SAVED));
        assert_eq!(fake.asked().len(), 2, "no sign-in");
    }

    #[test]
    fn sign_out_forgets_the_password() {
        let (mut d, fake) = with_fake();
        fake.script().stored = keys("me@example.com", "pw", "abc");
        d.open();
        poll(&mut d);
        d.form.trafikverket = "typed".into();
        d.status = Status::ok("kept");
        d.sign_out();
        assert!(d.busy);
        assert_eq!(d.status, Status::ok("kept"), "signing out does not clear the status");
        assert_eq!(poll(&mut d), [Action::Session(SessionInfo { authenticated: false, expires: None })]);
        assert_eq!(fake.asked()[1..], ["sign out", "save me@example.com  abc"]);
        fake.script().saves[0].send(Ok(keys("me@example.com", "", "abc"))).unwrap();
        poll(&mut d);
        assert_eq!(d.saved, keys("me@example.com", "", "abc"));
        assert_eq!(d.form, keys("me@example.com", "", "typed"), "the form keeps what was typed, less the password");
        assert_eq!(d.status, Status::ok(SIGNED_OUT));
        assert!(!d.busy);
    }

    #[test]
    fn export_writes_the_form_with_or_without_the_login() {
        let (mut d, fake) = with_fake();
        d.form = keys("me@example.com", "pw", "abc");
        d.export();
        let file = "{\n  \"format\": \"rustradar-keys\",\n  \"version\": 1,\n  \"keys\": {\n    \"trafikverket\": \"abc\"\n  }\n}\n";
        assert_eq!(fake.asked(), [format!("export rustradar-keys.json {file}")]);
        fake.script().exports[0].send(Ok(PathBuf::from("/home/me/Downloads/rustradar-keys.json"))).unwrap();
        poll(&mut d);
        assert_eq!(fake.asked()[1], "reveal /home/me/Downloads/rustradar-keys.json");
        assert_eq!(d.status, Status::ok("Saved rustradar-keys.json to Downloads"));
        d.with_login = true;
        d.export();
        assert!(fake.asked()[2].contains("\"fr24Email\": \"me@example.com\"") && fake.asked()[2].contains("\"fr24Password\": \"pw\""));
        fake.script().exports[1].send(Err("No Downloads folder to save to".into())).unwrap();
        poll(&mut d);
        assert_eq!(d.status, Status::error("Couldn't export: No Downloads folder to save to"));
        assert!(!d.busy, "exporting is never busy");
    }

    #[test]
    fn import_reads_the_file_over_the_form_and_saves_it() {
        let (mut d, fake) = with_fake();
        fake.script().stored = keys("me@example.com", "pw", "");
        d.open();
        poll(&mut d);
        d.import();
        d.import();
        assert_eq!(fake.asked(), ["load", "pick"], "one file dialog at a time");
        fake.script().picks[0].send(Some(Ok(r#"{"format":"rustradar-keys","version":1,"keys":{"trafikverket":"tv","trafiklabOxyfi":"ox"}}"#.into()))).unwrap();
        poll(&mut d);
        assert!(d.busy);
        assert_eq!(fake.asked()[2], "save me@example.com pw tv");
        fake.script().saves[0].send(Ok(Keys { trafiklab_oxyfi: "ox".into(), ..keys("me@example.com", "pw", "tv") })).unwrap();
        assert!(poll(&mut d).is_empty(), "the login did not change: no sign-in");
        assert_eq!(d.status, Status::ok("Imported 2 keys"));
        // a file that is not one, one without keys, a cancelled dialog
        for (text, status) in [("not json", "That file isn't a RustRadar keys file."), (r#"{"format":"rustradar-keys","keys":{}}"#, "That file has no keys in it.")] {
            d.import();
            let n = fake.script().picks.len();
            fake.script().picks[n - 1].send(Some(Ok(text.into()))).unwrap();
            poll(&mut d);
            assert_eq!(d.status, Status::error(status));
        }
        d.import();
        let n = fake.script().picks.len();
        fake.script().picks[n - 1].send(None).unwrap();
        poll(&mut d);
        assert_eq!(d.status, Status::error("That file has no keys in it."), "cancelling changes nothing");
        assert_eq!(fake.script().saves.len(), 1);
    }

    #[test]
    fn an_imported_login_signs_in() {
        let (mut d, fake) = with_fake();
        d.open();
        poll(&mut d);
        d.import();
        fake.script().picks[0].send(Some(Ok(r#"{"format":"rustradar-keys","keys":{"fr24Email":"a@b.c","fr24Password":"pw"}}"#.into()))).unwrap();
        poll(&mut d);
        fake.script().saves[0].send(Ok(keys("a@b.c", "pw", ""))).unwrap();
        poll(&mut d);
        let session = SessionInfo { authenticated: true, expires: None };
        fake.script().sign_ins[0].send(Ok(session)).unwrap();
        assert_eq!(poll(&mut d), [Action::Session(session)]);
        assert_eq!(d.status, Status::ok("Imported 2 keys, signed in to Flightradar24"));
    }

    #[test]
    fn without_a_backend_the_keys_live_in_memory() {
        let mut d = Dialog::default();
        d.open();
        poll(&mut d);
        assert_eq!(d.saved, Keys::default());
        d.form.trafikverket = "abc".into();
        d.submit();
        poll(&mut d);
        assert_eq!(d.status, Status::ok(SAVED));
        d.open();
        poll(&mut d);
        assert_eq!(d.saved.trafikverket, "abc", "kept for the session");
        d.export();
        poll(&mut d);
        assert_eq!(d.status, Status::error(format!("Couldn't export: {NO_BACKEND}")));
        d.form = keys("a@b.c", "pw", "abc");
        d.submit();
        assert!(poll(&mut d).is_empty());
        assert_eq!(d.status, Status::error(NO_BACKEND));
    }

    // ---------------------------------------------------------------------------------------------
    // through egui

    fn state() -> AppState {
        AppState::new(Startup::default(), None, None, None, ViewState::INITIAL, [1440.0, 900.0])
    }

    fn context(fake: &Fake) -> Context {
        let ctx = Context::default();
        theme::apply(&ctx);
        install_feeds(&ctx, Box::new(fake.clone()));
        ctx
    }

    fn pass(ctx: &Context, state: &AppState, time: f64, events: Vec<Event>) -> (Vec<Action>, Option<Placed>) {
        let mut actions = Vec::new();
        let mut placed = None;
        let raw = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, SIZE)), time: Some(time), events, ..Default::default() };
        ctx.run_ui(raw, |ui| placed = super::ui(ui.ctx(), state, &mut actions)).textures_delta.clear();
        (actions, placed)
    }

    fn click(at: Pos2) -> Vec<Event> {
        vec![
            Event::PointerMoved(at),
            Event::PointerButton { pos: at, button: PointerButton::Primary, pressed: true, modifiers: Default::default() },
            Event::PointerButton { pos: at, button: PointerButton::Primary, pressed: false, modifiers: Default::default() },
        ]
    }

    fn key(key: Key) -> Event {
        Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: Default::default() }
    }

    #[test]
    fn the_dialog_is_laid_out_where_chromium_lays_out_todays() {
        let fake = Fake::default();
        let ctx = context(&fake);
        let mut s = state();
        s.set_open(Open::Keys, true);
        pass(&ctx, &s, 0.0, vec![]);
        let placed = pass(&ctx, &s, 1.0, vec![]).1.unwrap();
        // Chromium at 1440 x 900, signed out: the card 442 x 552.13 at 499, 173.94
        let near = |a: f32, b: f32, what: &str| assert!((a - b).abs() <= 0.5, "{what}: {a} against Chromium's {b}");
        near(placed.card.left(), 499.0, "card x");
        near(placed.card.top(), 173.94, "card y");
        near(placed.card.width(), 442.0, "card width");
        near(placed.card.height(), 552.13, "card height");
        near(placed.close.left(), 892.0, "close x");
        near(placed.close.top(), 186.94, "close y");
        for (field, y) in placed.fields.iter().zip([323.52, 388.31, 528.89]) {
            near(field.left(), 516.0, "field x");
            near(field.top(), y, "field y");
            near(field.width(), 408.0, "field width");
            near(field.height(), 36.0, "field height");
        }
        near(placed.primary.left(), 516.0, "Save x");
        near(placed.primary.top(), 603.28, "Save y");
        near(placed.primary.width(), 54.83, "Save width");
        near(placed.import.left(), 770.5, "Import x");
        near(placed.import.width(), 79.22, "Import width");
        near(placed.export.left(), 857.72, "Export x");
        near(placed.export.width(), 66.28, "Export width");
        near(placed.check.top(), 647.28, "checkbox row y");
        near(placed.check.height(), 19.0, "checkbox row height");
        near(placed.link.left(), 516.0, "link x");
        near(placed.link.width(), 146.56, "link width");
        assert_eq!(placed.sign_out, Rect::NOTHING);
        // signed in: the badge's legend is 18.5 high and Sign out comes after the password
        s.aircraft.set_session(SessionInfo { authenticated: true, expires: None });
        pass(&ctx, &s, 2.0, vec![]);
        let placed = pass(&ctx, &s, 3.0, vec![]).1.unwrap();
        near(placed.card.top(), 153.78, "signed in: card y");
        near(placed.card.height(), 592.44, "signed in: card height");
        near(placed.sign_out.left(), 516.0, "Sign out x");
        near(placed.sign_out.top(), 412.47, "Sign out y");
        near(placed.sign_out.width(), 75.31, "Sign out width");
        near(placed.fields[2].top(), 549.05, "signed in: API key y");
    }

    #[test]
    fn typing_saving_and_closing_through_egui() {
        let fake = Fake::default();
        let ctx = context(&fake);
        let mut s = state();
        assert_eq!(pass(&ctx, &s, 0.0, vec![]), (vec![], None));
        assert!(fake.asked().is_empty(), "nothing asked while closed");
        s.set_open(Open::Keys, true);
        pass(&ctx, &s, 0.0, vec![]);
        let placed = pass(&ctx, &s, 1.0, vec![]).1.unwrap();
        assert_eq!(fake.asked(), ["load"]);
        // the API key: a click, the text, Enter (the form's implicit submission)
        pass(&ctx, &s, 2.0, click(placed.fields[2].center()));
        pass(&ctx, &s, 2.1, vec![Event::Text("abc".into())]);
        assert_eq!(dialog(&ctx).lock().unwrap().form.trafikverket, "abc");
        pass(&ctx, &s, 2.2, vec![key(Key::Enter)]);
        assert_eq!(fake.asked()[1], "save   abc");
        // busy: the field drops the keyboard and takes nothing
        pass(&ctx, &s, 2.3, vec![Event::Text("x".into())]);
        assert_eq!(dialog(&ctx).lock().unwrap().form.trafikverket, "abc");
        fake.script().saves[0].send(Ok(keys("", "", "abc"))).unwrap();
        pass(&ctx, &s, 2.4, vec![]);
        assert_eq!(dialog(&ctx).lock().unwrap().status, Status::ok(SAVED));
        // the checkbox's whole row toggles it
        pass(&ctx, &s, 3.0, click(Pos2::new(placed.check.right() - 10.0, placed.check.center().y)));
        assert!(dialog(&ctx).lock().unwrap().with_login);
        // Export
        pass(&ctx, &s, 4.0, click(placed.export.center()));
        assert!(fake.asked()[2].starts_with("export rustradar-keys.json"));
        // Escape while typing closes it, as the backdrop and the close button do
        pass(&ctx, &s, 5.0, click(placed.fields[0].center()));
        let (actions, _) = pass(&ctx, &s, 5.1, vec![key(Key::Escape)]);
        assert_eq!(actions, [Action::Open(Open::Keys, false)]);
        let (actions, _) = pass(&ctx, &s, 6.0, click(Pos2::new(100.0, 100.0)));
        assert_eq!(actions, [Action::Open(Open::Keys, false)]);
        let (actions, _) = pass(&ctx, &s, 7.0, click(placed.close.center()));
        assert_eq!(actions, [Action::Open(Open::Keys, false)]);
        // the page's keys are off while it is open
        assert!(s.shortcuts.shortcuts_off());
        s.set_open(Open::Keys, false);
        assert!(!s.shortcuts.shortcuts_off());
        assert_eq!(pass(&ctx, &s, 8.0, vec![]).1, None);
        // reopened: the status is cleared and the keys asked again
        dialog(&ctx).lock().unwrap().status = Status::ok("old");
        s.set_open(Open::Keys, true);
        pass(&ctx, &s, 9.0, vec![]);
        assert_eq!(dialog(&ctx).lock().unwrap().status, None);
        assert_eq!(fake.asked().iter().filter(|a| *a == "load").count(), 2);
    }

    #[test]
    fn a_sign_in_prompt_puts_the_keyboard_in_the_email_field_and_takes_it_back() {
        let fake = Fake::default();
        let ctx = context(&fake);
        let mut s = state();
        let prompt = Id::unique("the overview's sign-in note");
        pass(&ctx, &s, 0.0, vec![]);
        prompt_sign_in(&ctx, Some(prompt));
        s.set_open(Open::Keys, true);
        pass(&ctx, &s, 1.0, vec![]);
        pass(&ctx, &s, 2.0, vec![]);
        pass(&ctx, &s, 2.5, vec![Event::Text("me".into())]);
        assert_eq!(dialog(&ctx).lock().unwrap().form.fr24_email, "me", "the email field has the keyboard");
        s.set_open(Open::Keys, false);
        pass(&ctx, &s, 3.0, vec![]);
        assert_eq!(ctx.memory(|m| m.focused()), Some(prompt));
        // opened otherwise: no field takes the keyboard
        s.set_open(Open::Keys, true);
        pass(&ctx, &s, 5.0, vec![]);
        pass(&ctx, &s, 6.0, vec![Event::Text("x".into())]);
        assert_eq!(dialog(&ctx).lock().unwrap().form.fr24_email, "", "reloaded, and nothing typed into it");
    }

    #[test]
    fn the_sign_out_button_shows_signed_in_and_hands_the_session_over() {
        let fake = Fake::default();
        let ctx = context(&fake);
        let mut s = state();
        s.aircraft.set_session(SessionInfo { authenticated: true, expires: None });
        s.set_open(Open::Keys, true);
        pass(&ctx, &s, 0.0, vec![]);
        let placed = pass(&ctx, &s, 1.0, vec![]).1.unwrap();
        pass(&ctx, &s, 2.0, click(placed.sign_out.center()));
        let (actions, _) = pass(&ctx, &s, 2.1, vec![]);
        assert_eq!(actions, [Action::Session(SessionInfo { authenticated: false, expires: None })]);
        assert_eq!(fake.asked()[1..3], ["sign out".to_owned(), "save   ".to_owned()]);
        // applied, the session reaches the state
        s.apply(actions[0], 0.0);
        assert_eq!(s.aircraft.session(), Some(SessionInfo { authenticated: false, expires: None }));
    }
}
