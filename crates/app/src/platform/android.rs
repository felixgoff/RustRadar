//! Android (plan 1.2, 1.5; doc 07 §11.1, §13.4; doc 06 §2.4, §29): the folders the Tauri build
//! used, the safe area, the soft keyboard's text and the back gesture.
//!
//! The activity is `platform/android/app/src/main/java/com/felix/rustradar/MainActivity.kt`, a
//! `GameActivity` (winit's `android-game-activity`). Besides what GameActivity does itself, it
//! calls into this file:
//! - `nativeInsets(left, top, right, bottom, keyboard)`: the window insets GameActivity's surface
//!   gets (system bars and display cutout; and the soft keyboard's height), device pixels, each
//!   time they change;
//! - `nativeBack()`: the back gesture or key. The activity takes the key before GameActivity's
//!   native key path would swallow it, and AndroidX hands the gesture over on Android 13+. It
//!   answers whether the app takes it: not before `init`, and then the activity leaves at once;
//! - `nativeEditorAction(action)`: the keyboard's action key (Done). android-activity delivers it
//!   only to winit's input loop, which drops it;
//! - `nativeTextChanged()`: the keyboard changed the text, so a frame reads it soon;
//!
//! and it offers `leave()`: what the back gesture does when the app does not take it (finish, or
//! go to the background).
//!
//! The frame loop's part (on Android only):
//! - `init(event_loop.android_app(), wake)` before the backend starts, then [`dirs`] for it;
//! - each frame, after egui-winit's `take_egui_input`: `before_frame(ctx, &mut raw_input,
//!   pixels_per_point)`. It sets the safe area, turns the keyboard's edits into egui events and
//!   returns whether back was pressed: then `Action::CloseTop`, and [`leave`] when that closed
//!   nothing (doc 06 §3.11; the web build's history entry did the same);
//! - after the egui pass, before egui-winit's `handle_platform_output`: `after_frame(ctx,
//!   &platform_output)`, which mirrors the focused text field to the keyboard;
//! - a text field widget calls [`text::report`] with its text each frame it is shown.
//!
//! The safe area does not come from `AndroidApp::content_rect`: GameActivity reports there its
//! surface view's own rectangle in the window, which never leaves an inset against the surface
//! (and with edge to edge, enforced from target SDK 35, it is the whole window). The insets are
//! GameActivity's `onApplyWindowInsets` instead, the ones its surface view gets.
//!
//! Everything but the glue touching `AndroidApp` and JNI builds and is tested on every platform.

use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use rustradar_backend::{Dirs, Wake};

/// The folders the Tauri build used on Android (tauri 2.12 `PathPlugin.kt`): config is the app's
/// data folder (`Context.getDataDir()`, so `keys.json` sits right in it), cache is
/// `Context.getCacheDir()` (`<data>/cache`). No downloads: exports are desktop only in the first
/// native release (plan section 7).
///
/// GameActivity hands Rust `getFilesDir()`, which is `<data>/files`. A files folder not named
/// `files` (never seen) is taken as the data folder itself.
pub fn folders(files_dir: &Path) -> Dirs {
    let data = match (files_dir.file_name(), files_dir.parent()) {
        (Some(name), Some(parent)) if name == "files" => parent,
        _ => files_dir,
    };
    Dirs { cache: data.join("cache"), config: data.to_path_buf(), downloads: None }
}

/// The window insets GameActivity's surface gets, device pixels: what the system bars and the
/// display cutout cover at each edge, and the soft keyboard's height (0 when hidden).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Insets {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
    pub keyboard: i32,
}

impl Insets {
    pub const ZERO: Insets = Insets { left: 0, top: 0, right: 0, bottom: 0, keyboard: 0 };
}

static INSETS: Mutex<Insets> = Mutex::new(Insets::ZERO);
static BACK: AtomicBool = AtomicBool::new(false);
static ACTION: AtomicBool = AtomicBool::new(false);
static WAKE: Mutex<Option<Wake>> = Mutex::new(None);

/// Who to wake when the activity reports something (the event loop's wake).
pub fn set_wake(wake: Wake) {
    if let Ok(mut w) = WAKE.lock() {
        *w = Some(wake);
    }
}

/// Wakes the event loop; false when nobody has asked to be woken.
fn wake() -> bool {
    let wake = WAKE.lock().ok().and_then(|w| w.clone());
    if let Some(wake) = &wake {
        wake();
    }
    wake.is_some()
}

/// The activity's new insets (`nativeInsets`).
pub fn report_insets(insets: Insets) {
    if let Ok(mut i) = INSETS.lock() {
        *i = insets;
    }
    wake();
}

/// The last insets the activity reported.
pub fn insets() -> Insets {
    INSETS.lock().map(|i| *i).unwrap_or_default()
}

/// The safe area egui pads by (doc 06 §2.4: the top bar, the sheet's bottom, the controls' right
/// inset), points. The soft keyboard is not part of it, as on iOS.
pub fn safe_area(insets: Insets, pixels_per_point: f32) -> egui::SafeAreaInsets {
    let points = |px: i32| px.max(0) as f32 / pixels_per_point;
    egui::SafeAreaInsets(egui::epaint::MarginF32 {
        left: points(insets.left),
        right: points(insets.right),
        top: points(insets.top),
        bottom: points(insets.bottom),
    })
}

/// The soft keyboard's height, points (0 when hidden).
pub fn keyboard_height(insets: Insets, pixels_per_point: f32) -> f32 {
    insets.keyboard.max(0) as f32 / pixels_per_point
}

/// The back gesture or key (`nativeBack`). Returns whether the app takes it: only once the frame
/// loop listens (a wake is set), else back must do what it does without the app.
pub fn back_pressed() -> bool {
    if WAKE.lock().map(|w| w.is_none()).unwrap_or(true) {
        return false;
    }
    BACK.store(true, Ordering::Release);
    wake()
}

/// Whether back was pressed since the last call.
pub fn take_back() -> bool {
    BACK.swap(false, Ordering::AcqRel)
}

/// The keyboard's action key (`nativeEditorAction`): Enter for the focused field.
pub fn editor_action() {
    ACTION.store(true, Ordering::Release);
    wake();
}

/// Whether the keyboard's action key was pressed since the last call.
pub fn take_action() -> bool {
    ACTION.swap(false, Ordering::AcqRel)
}

/// The text bridge (plan 1.5): the focused egui text field and GameActivity's `GameTextInput`
/// kept equal.
///
/// winit 0.30 shows and hides the soft keyboard (egui-winit asks it to while a text field has
/// focus) but never reads what is typed on it. So while a field has focus its text and selection
/// are mirrored to the keyboard (`AndroidApp::set_text_input_state`), and each frame the
/// keyboard's state is read back (`text_input_state`): the edit between what the keyboard held
/// and what it holds now becomes egui events on the field (select the replaced characters, then
/// type the new ones, or Backspace). The field is the truth: when it differs from what the
/// keyboard holds (typed with a hardware keyboard, pasted, cleared by the app), it is sent to the
/// keyboard again. All of the app's text fields are single line, so a newline is Enter.
pub mod text {
    use std::ops::Range;

    use egui::output::OutputEvent;
    use egui::text::{CCursor, CCursorRange};
    use egui::text_edit::TextEditState;
    use egui::{Context, Event, Id, Key, Modifiers, PlatformOutput, RawInput, Role};

    /// What the soft keyboard holds: `GameTextInput`'s text and selection. The selection is in
    /// UTF-16 code units (Java's string indices, which GameTextInput passes through unchanged),
    /// start ≤ end.
    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct KeyboardText {
        pub text: String,
        pub selection: Range<usize>,
    }

    impl KeyboardText {
        /// The keyboard's state as `GameTextInput` reports it: the two ends in either order,
        /// possibly past the end or undefined (-1 read as a huge number).
        pub fn new(text: String, start: usize, end: usize) -> Self {
            let len = text.encode_utf16().count();
            let (start, end) = (start.min(len), end.min(len));
            KeyboardText { text, selection: start.min(end)..start.max(end) }
        }

        /// A field's text and its selection in characters.
        pub fn of_field(text: &str, chars: Range<usize>) -> Self {
            let a = char_to_utf16(text, chars.start);
            let b = char_to_utf16(text, chars.end);
            KeyboardText { text: text.to_owned(), selection: a.min(b)..a.max(b) }
        }

        /// The selection in characters.
        pub fn char_selection(&self) -> Range<usize> {
            utf16_to_char(&self.text, self.selection.start)..utf16_to_char(&self.text, self.selection.end)
        }
    }

    /// The character index at a UTF-16 index (one inside a surrogate pair counts that character
    /// as passed; past the end is the end).
    pub fn utf16_to_char(text: &str, utf16: usize) -> usize {
        let mut units = 0;
        for (i, c) in text.chars().enumerate() {
            if units >= utf16 {
                return i;
            }
            units += c.len_utf16();
        }
        text.chars().count()
    }

    /// The UTF-16 index of a character index (past the end is the end).
    pub fn char_to_utf16(text: &str, chars: usize) -> usize {
        text.chars().take(chars).map(char::len_utf16).sum()
    }

    /// The one replacement that turns `old` into `new`: the characters of `old` it replaces and
    /// what it puts there (the shortest, after the common start and end).
    pub fn edit(old: &str, new: &str) -> (Range<usize>, String) {
        let old: Vec<char> = old.chars().collect();
        let new: Vec<char> = new.chars().collect();
        let start = old.iter().zip(&new).take_while(|(a, b)| a == b).count();
        let room = old.len().min(new.len()) - start;
        let end = old.iter().rev().zip(new.iter().rev()).take(room).take_while(|(a, b)| a == b).count();
        (start..old.len() - end, new[start..new.len() - end].iter().collect())
    }

    /// What [`Bridge::after_frame`] asks of the keyboard.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub enum Sync {
        /// Nothing changed.
        Nothing,
        /// A text field took focus: set up the keyboard for it with this text.
        Start(KeyboardText),
        /// The field changed: the keyboard should hold this.
        Update(KeyboardText),
        /// No field has focus any more (egui-winit hides the keyboard).
        Stop,
        /// The keyboard's edit left its cursor elsewhere than egui's (an autocorrected word, say):
        /// the field's cursor was moved to the keyboard's after the pass, so draw again.
        Moved,
    }

    /// The focused field and what the keyboard holds for it.
    #[derive(Debug)]
    struct Field {
        id: Id,
        keyboard: KeyboardText,
        /// After the keyboard's edit: its selection, in characters, for the field once the edit
        /// is in (egui leaves the cursor after the typed text).
        cursor: Option<Range<usize>>,
    }

    /// The bridge's memory between frames: which field is mirrored and what the keyboard holds.
    #[derive(Debug, Default)]
    pub struct Bridge {
        field: Option<Field>,
    }

    /// Where a text field leaves its text for the bridge.
    fn report_id(id: Id) -> Id {
        Id::unique("rustradar.text_bridge").with(id)
    }

    /// A text field's text, given each frame the field is shown (the field with id `id`). Without
    /// it the bridge learns the text from egui's widget events, which miss changes the app makes.
    pub fn report(ctx: &Context, id: Id, text: &str) {
        let key = report_id(id);
        if ctx.data(|d| d.get_temp::<String>(key)).as_deref() != Some(text) {
            ctx.data_mut(|d| d.insert_temp(key, text.to_owned()));
        }
    }

    fn reported(ctx: &Context, id: Id) -> Option<String> {
        ctx.data(|d| d.get_temp::<String>(report_id(id)))
    }

    /// The text of the last text field egui reported on this frame (focus, click, change,
    /// selection).
    fn text_of_events(output: &PlatformOutput) -> Option<String> {
        output.events.iter().rev().map(OutputEvent::widget_info).find(|w| w.role == Role::TextInput).and_then(|w| w.current_text_value.clone())
    }

    fn key(key: Key, pressed: bool) -> Event {
        Event::Key { key, physical_key: None, pressed, repeat: false, modifiers: Modifiers::NONE }
    }

    fn select(ctx: &Context, id: Id, chars: Range<usize>) {
        let mut state = TextEditState::load(ctx, id).unwrap_or_default();
        state.cursor.set_char_range(Some(CCursorRange::two(CCursor::new(chars.start), CCursor::new(chars.end))));
        state.store(ctx, id);
    }

    impl Bridge {
        pub const fn new() -> Self {
            Bridge { field: None }
        }

        /// Whether a field is mirrored (the keyboard is worth reading).
        pub fn is_open(&self) -> bool {
            self.field.is_some()
        }

        /// Before the egui pass: what the keyboard holds now (`keyboard`, None when not read) and
        /// whether its action key was pressed become events on the focused field in `raw`.
        /// Returns whether anything was added.
        pub fn before_frame(&mut self, ctx: &Context, keyboard: Option<KeyboardText>, action: bool, raw: &mut RawInput) -> bool {
            let Some(field) = &mut self.field else { return false };
            let mut changed = false;
            if let Some(keyboard) = keyboard
                && keyboard != field.keyboard
            {
                if keyboard.text == field.keyboard.text {
                    // the selection alone moved (the keyboard's cursor keys or handles)
                    select(ctx, field.id, keyboard.char_selection());
                } else {
                    let (replaced, inserted) = edit(&field.keyboard.text, &keyboard.text);
                    let enter = inserted.contains(['\n', '\r']);
                    let typed: String = inserted.chars().filter(|c| !matches!(c, '\n' | '\r')).collect();
                    select(ctx, field.id, replaced.clone());
                    if !typed.is_empty() {
                        raw.events.push(Event::Text(typed));
                    } else if !replaced.is_empty() {
                        raw.events.extend([key(Key::Backspace, true), key(Key::Backspace, false)]);
                    }
                    if enter {
                        raw.events.extend([key(Key::Enter, true), key(Key::Enter, false)]);
                    }
                    field.cursor = Some(keyboard.char_selection());
                }
                field.keyboard = keyboard;
                changed = true;
            }
            if action {
                raw.events.extend([key(Key::Enter, true), key(Key::Enter, false)]);
                changed = true;
            }
            changed
        }

        /// After the egui pass: which field has focus now (`output.ime` is set while one does),
        /// its text and selection, against what the keyboard holds.
        pub fn after_frame(&mut self, ctx: &Context, output: &PlatformOutput) -> Sync {
            let focused = output.ime.as_ref().and_then(|_| ctx.memory(|m| m.focused()));
            let Some(id) = focused else {
                return if self.field.take().is_some() { Sync::Stop } else { Sync::Nothing };
            };
            let text = reported(ctx, id).or_else(|| text_of_events(output));
            let selection = TextEditState::load(ctx, id).and_then(|s| s.cursor.char_range()).map(|r| {
                let r = r.as_sorted_char_range();
                r.start.0..r.end.0
            });
            match &mut self.field {
                Some(field) if field.id == id => {
                    let text = text.unwrap_or_else(|| field.keyboard.text.clone());
                    let mut selection = selection.unwrap_or_else(|| field.keyboard.char_selection());
                    if let Some(cursor) = field.cursor.take()
                        && text == field.keyboard.text
                        && selection != cursor
                    {
                        // the edit went in: the cursor where the keyboard has it
                        select(ctx, id, cursor.clone());
                        selection = cursor;
                        if KeyboardText::of_field(&text, selection.clone()) == field.keyboard {
                            return Sync::Moved;
                        }
                    }
                    let wanted = KeyboardText::of_field(&text, selection);
                    if wanted == field.keyboard {
                        Sync::Nothing
                    } else {
                        field.keyboard = wanted.clone();
                        Sync::Update(wanted)
                    }
                }
                _ => {
                    let text = text.unwrap_or_default();
                    let end = text.chars().count();
                    let keyboard = KeyboardText::of_field(&text, selection.unwrap_or(end..end));
                    self.field = Some(Field { id, keyboard: keyboard.clone(), cursor: None });
                    Sync::Start(keyboard)
                }
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use egui::{Pos2, Rect, TextEdit, vec2};

        use super::*;

        #[test]
        fn utf16_and_char_indices() {
            let s = "Åre ✈ 😀x";
            assert_eq!(char_to_utf16(s, 0), 0);
            assert_eq!(char_to_utf16(s, 6), 6, "the emoji starts at unit 6");
            assert_eq!(char_to_utf16(s, 7), 8, "and takes two units");
            assert_eq!(char_to_utf16(s, 99), 9);
            assert_eq!(utf16_to_char(s, 6), 6);
            assert_eq!(utf16_to_char(s, 7), 7, "inside the pair counts the emoji as passed");
            assert_eq!(utf16_to_char(s, 8), 7);
            assert_eq!(utf16_to_char(s, 9), 8);
            assert_eq!(utf16_to_char(s, 99), 8);
            let k = KeyboardText::new(s.into(), 99, usize::MAX);
            assert_eq!(k.selection, 9..9, "clamped to the text");
            let k = KeyboardText::new("abc".into(), 3, 1);
            assert_eq!(k.selection, 1..3, "ends in either order");
            // a selection dragged backwards (the anchor after the cursor)
            let backwards = Range { start: 8, end: 6 };
            assert_eq!(KeyboardText::of_field(s, backwards), KeyboardText { text: s.into(), selection: 6..9 });
        }

        #[test]
        fn the_shortest_edit() {
            assert_eq!(edit("Göteborg", "Göteborgs"), (8..8, "s".into()));
            assert_eq!(edit("Göteborgs", "Göteborg"), (8..9, String::new()));
            assert_eq!(edit("teh", "the "), (1..3, "he ".into()));
            assert_eq!(edit("", "Åre"), (0..0, "Åre".into()));
            assert_eq!(edit("aaa", "aa"), (2..3, String::new()), "the common end does not overlap the start");
            assert_eq!(edit("ab", "ab"), (2..2, String::new()));
            assert_eq!(edit("abc", "xyz"), (0..3, "xyz".into()));
        }

        /// A test window with one single-line field; the bridge runs around each pass as the
        /// frame loop runs it.
        struct Window {
            ctx: Context,
            bridge: Bridge,
            id: Id,
            text: String,
        }

        impl Window {
            fn new(text: &str) -> Self {
                Window { ctx: Context::default(), bridge: Bridge::new(), id: Id::unique("search"), text: text.into() }
            }

            /// One frame: the keyboard's state and action key, then `events`; returns what the
            /// bridge asks of the keyboard. `focus` asks the field for focus.
            fn frame(&mut self, keyboard: Option<KeyboardText>, action: bool, events: Vec<Event>, focus: bool, report: bool) -> Sync {
                let mut raw = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(400.0, 300.0))), events, ..Default::default() };
                self.bridge.before_frame(&self.ctx, keyboard, action, &mut raw);
                let (id, text) = (self.id, &mut self.text);
                let mut output = self.ctx.run_ui(raw, |ui| {
                    let r = ui.add(TextEdit::singleline(text).id(id));
                    if focus {
                        r.request_focus();
                    }
                    if report {
                        super::report(ui.ctx(), id, text);
                    }
                });
                // no renderer takes the font atlas here
                output.textures_delta.clear();
                self.bridge.after_frame(&self.ctx, &output.platform_output)
            }

            fn focus(&mut self) -> Sync {
                self.frame(None, false, vec![], true, true);
                self.frame(None, false, vec![], false, true)
            }

            fn type_on_keyboard(&mut self, text: &str, cursor: usize) -> Sync {
                self.frame(Some(KeyboardText::new(text.into(), cursor, cursor)), false, vec![], false, true)
            }
        }

        fn kb(text: &str, selection: Range<usize>) -> KeyboardText {
            KeyboardText { text: text.into(), selection }
        }

        #[test]
        fn a_focused_field_starts_the_keyboard_with_its_text() {
            let mut w = Window::new("Stock");
            assert_eq!(w.frame(None, false, vec![], false, true), Sync::Nothing, "no focus, no keyboard");
            assert_eq!(w.focus(), Sync::Start(kb("Stock", 5..5)), "the cursor at the end");
            assert!(w.bridge.is_open());
            assert_eq!(w.frame(None, false, vec![], false, true), Sync::Nothing);
        }

        #[test]
        fn typing_on_the_keyboard_edits_the_field() {
            let mut w = Window::new("");
            assert_eq!(w.focus(), Sync::Start(kb("", 0..0)));
            // Swedish letters, one at a time as Gboard commits them
            for (i, typed) in ["G", "Gö", "Göt", "Göte", "Göteb", "Götebo", "Göteborg"].iter().enumerate() {
                let len = typed.encode_utf16().count();
                assert_eq!(w.type_on_keyboard(typed, len), Sync::Nothing, "step {i}: field and keyboard agree, nothing sent back");
                assert_eq!(w.text, *typed);
            }
            // backspace held: two characters at once
            assert_eq!(w.type_on_keyboard("Götebo", 6), Sync::Nothing);
            assert_eq!(w.text, "Götebo");
            // autocorrect replaces a word: egui's cursor would stop before "rg", the keyboard's
            // is at the end, and stays there
            w.type_on_keyboard("Göteborg", 8);
            assert_eq!(w.type_on_keyboard("Gothenburg", 10), Sync::Moved);
            assert_eq!(w.text, "Gothenburg");
            assert_eq!(w.frame(None, false, vec![], false, true), Sync::Nothing, "the field's cursor is the keyboard's now");
            assert_eq!(w.type_on_keyboard("Gothenburg!", 11), Sync::Nothing);
            assert_eq!(w.text, "Gothenburg!");
        }

        #[test]
        fn a_moved_keyboard_cursor_moves_the_field_cursor() {
            let mut w = Window::new("Åre");
            w.focus();
            assert_eq!(w.type_on_keyboard("Åre", 1), Sync::Nothing);
            // the field's cursor is after Å now: typing there goes in the middle
            assert_eq!(w.type_on_keyboard("Åsre", 2), Sync::Nothing);
            assert_eq!(w.text, "Åsre");
        }

        #[test]
        fn changes_made_in_the_field_go_to_the_keyboard() {
            let mut w = Window::new("Kiruna");
            w.focus();
            // the app clears the field
            w.text.clear();
            assert_eq!(w.frame(None, false, vec![], false, true), Sync::Update(kb("", 0..0)));
            // a hardware keyboard types into the field directly
            assert_eq!(w.frame(None, false, vec![Event::Text("Lund".into())], false, true), Sync::Update(kb("Lund", 4..4)));
            assert_eq!(w.text, "Lund");
        }

        #[test]
        fn without_report_a_tap_brings_the_text() {
            let mut w = Window::new("Malmö");
            let at = Pos2::new(40.0, 10.0);
            let button = |pressed| Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
            // egui hit-tests against the last frame's widgets: one frame to lay the field out
            w.frame(None, false, vec![], false, false);
            let pressed = w.frame(None, false, vec![Event::PointerMoved(at), button(true)], false, false);
            let Sync::Start(keyboard) = pressed else { panic!("a touch focuses the field, got {pressed:?}") };
            assert_eq!(keyboard.text, "Malmö", "the focus event carries the text");
            w.frame(None, false, vec![button(false)], false, false);
            let end = KeyboardText::new("Malmö".into(), 5, 5);
            w.frame(Some(end.clone()), false, vec![], false, false);
            assert_eq!(w.frame(Some(kb("Malmöx", 6..6)), false, vec![], false, false), Sync::Nothing);
            assert_eq!(w.text, "Malmöx");
        }

        #[test]
        fn a_newline_or_the_action_key_is_enter() {
            let mut w = Window::new("");
            w.focus();
            // Enter makes a single-line field give up focus
            assert_eq!(w.type_on_keyboard("Visby\n", 6), Sync::Stop);
            assert_eq!(w.text, "Visby", "the newline is not typed");
            assert!(!w.bridge.is_open());
            assert_eq!(w.frame(None, false, vec![], false, true), Sync::Nothing);

            let mut w = Window::new("Umeå");
            w.focus();
            assert_eq!(w.frame(None, true, vec![], false, true), Sync::Stop, "the action key is Enter");
            assert_eq!(w.text, "Umeå");
        }

        #[test]
        fn losing_focus_stops_the_bridge() {
            let mut w = Window::new("Luleå");
            w.focus();
            w.ctx.memory_mut(|m| m.stop_text_input());
            assert_eq!(w.frame(None, false, vec![], false, true), Sync::Stop);
            assert_eq!(w.frame(None, false, vec![], false, true), Sync::Nothing);
            assert!(!w.bridge.before_frame(&w.ctx, Some(kb("x", 1..1)), true, &mut RawInput::default()), "nothing mirrored, nothing typed");
        }
    }
}

/// The glue that touches the activity: Android only.
#[cfg(target_os = "android")]
mod glue {
    use std::ffi::c_void;
    use std::sync::Mutex;

    use rustradar_backend::{Dirs, Wake};
    use winit::platform::android::activity::AndroidApp;
    use winit::platform::android::activity::input::{ImeOptions, InputType, TextInputAction, TextInputState, TextSpan};

    use super::text::{Bridge, KeyboardText, Sync};

    static APP: Mutex<Option<AndroidApp>> = Mutex::new(None);
    static BRIDGE: Mutex<Bridge> = Mutex::new(Bridge::new());

    fn app() -> Option<AndroidApp> {
        APP.lock().ok()?.clone()
    }

    /// The activity is up (`android_main` may run again for a new activity in the same process):
    /// logcat logging, the activity for the calls below, and the event loop's wake.
    pub fn init(app: &AndroidApp, wake: Wake) {
        android_logger::init_once(
            android_logger::Config::default().with_tag("rustradar").with_max_level(log::LevelFilter::Info),
        );
        if let Ok(mut a) = APP.lock() {
            *a = Some(app.clone());
        }
        if let Ok(mut b) = BRIDGE.lock() {
            *b = Bridge::new();
        }
        super::set_wake(wake);
    }

    /// The folders of the Tauri build ([`super::folders`]), from the activity's.
    pub fn dirs() -> Option<Dirs> {
        let app = app()?;
        Some(super::folders(&app.internal_data_path()?))
    }

    /// Before the egui pass: the safe area, the keyboard's edits; whether back was pressed.
    pub fn before_frame(ctx: &egui::Context, raw: &mut egui::RawInput, pixels_per_point: f32) -> bool {
        raw.safe_area_insets = Some(super::safe_area(super::insets(), pixels_per_point));
        let action = super::take_action();
        if let (Some(app), Ok(mut bridge)) = (app(), BRIDGE.lock()) {
            let keyboard = bridge.is_open().then(|| {
                let s = app.text_input_state();
                KeyboardText::new(s.text, s.selection.start, s.selection.end)
            });
            bridge.before_frame(ctx, keyboard, action, raw);
        }
        super::take_back()
    }

    /// After the egui pass: the focused field mirrored to the keyboard.
    pub fn after_frame(ctx: &egui::Context, output: &egui::PlatformOutput) {
        let Some(app) = app() else { return };
        let sync = match BRIDGE.lock() {
            Ok(mut bridge) => bridge.after_frame(ctx, output),
            Err(_) => return,
        };
        let state = |k: KeyboardText| TextInputState {
            text: k.text,
            selection: TextSpan { start: k.selection.start, end: k.selection.end },
            compose_region: None,
        };
        match sync {
            Sync::Start(keyboard) => {
                // a plain text keyboard with a Done key, never full screen over the app
                app.set_ime_editor_info(
                    InputType::TYPE_CLASS_TEXT,
                    TextInputAction::Done,
                    ImeOptions::IME_FLAG_NO_FULLSCREEN | ImeOptions::IMG_FLAG_NO_EXTRACT_UI,
                );
                app.set_text_input_state(state(keyboard));
                app.show_soft_input(false);
            }
            Sync::Update(keyboard) => app.set_text_input_state(state(keyboard)),
            // the frame drawn showed egui's cursor: one more with the keyboard's
            Sync::Moved => {
                let _ = super::wake();
            }
            Sync::Stop | Sync::Nothing => {}
        }
    }

    /// Back took nothing: what back does without the app (`MainActivity.leave()`).
    pub fn leave() {
        let Some(app) = app() else { return };
        // SAFETY: android-activity hands out the process's JavaVM, which lives for the process
        let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) };
        let activity = app.activity_as_ptr() as jni::sys::jobject;
        let called = vm.attach_current_thread(|env| -> jni::errors::Result<()> {
            // SAFETY: the activity's global reference lives as long as `app`
            let activity = unsafe { env.as_cast_raw::<jni::refs::Global<jni::objects::JObject>>(&activity)? };
            let called = env.call_method(&activity, jni::jni_str!("leave"), jni::jni_sig!("()V"), &[]);
            if called.is_err() {
                // a Java exception stays pending until cleared
                env.exception_clear();
            }
            called.map(|_| ())
        });
        if let Err(e) = called {
            log::error!("cannot leave the activity: {e}");
        }
    }

    // What MainActivity.kt calls (JNI, on the activity's UI thread).

    #[unsafe(no_mangle)]
    extern "system" fn Java_com_felix_rustradar_MainActivity_nativeInsets(
        _env: *mut c_void,
        _this: *mut c_void,
        left: i32,
        top: i32,
        right: i32,
        bottom: i32,
        keyboard: i32,
    ) {
        super::report_insets(super::Insets { left, top, right, bottom, keyboard });
    }

    /// A JNI `boolean`: whether the app takes back.
    #[unsafe(no_mangle)]
    extern "system" fn Java_com_felix_rustradar_MainActivity_nativeBack(_env: *mut c_void, _this: *mut c_void) -> u8 {
        u8::from(super::back_pressed())
    }

    #[unsafe(no_mangle)]
    extern "system" fn Java_com_felix_rustradar_MainActivity_nativeEditorAction(_env: *mut c_void, _this: *mut c_void, _action: i32) {
        super::editor_action();
    }

    #[unsafe(no_mangle)]
    extern "system" fn Java_com_felix_rustradar_MainActivity_nativeTextChanged(_env: *mut c_void, _this: *mut c_void) {
        let _ = super::wake();
    }
}

#[cfg(target_os = "android")]
pub use glue::{after_frame, before_frame, dirs, init, leave};

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::sync::atomic::AtomicUsize;

    use super::*;

    #[test]
    fn the_folders_of_the_tauri_build() {
        let dirs = folders(Path::new("/data/user/0/com.felix.rustradar/files"));
        assert_eq!(dirs.config, PathBuf::from("/data/user/0/com.felix.rustradar"));
        assert_eq!(dirs.config_file("keys.json"), PathBuf::from("/data/user/0/com.felix.rustradar/keys.json"));
        assert_eq!(dirs.cache, PathBuf::from("/data/user/0/com.felix.rustradar/cache"));
        assert_eq!(dirs.downloads, None, "no exports on phones");
        let odd = folders(Path::new("/data/data/com.felix.rustradar"));
        assert_eq!(odd.config, PathBuf::from("/data/data/com.felix.rustradar"), "not named files: the data folder itself");
        assert_eq!(odd.cache, PathBuf::from("/data/data/com.felix.rustradar/cache"));
    }

    #[test]
    fn insets_in_points() {
        let i = Insets { left: 0, top: 147, right: 30, bottom: 102, keyboard: 900 };
        let s = safe_area(i, 3.0);
        assert_eq!((s.0.left, s.0.top, s.0.right, s.0.bottom), (0.0, 49.0, 10.0, 34.0));
        assert_eq!(keyboard_height(i, 3.0), 300.0);
        let s = safe_area(Insets { left: -4, ..Insets::ZERO }, 2.0);
        assert_eq!(s.0.left, 0.0, "never negative");
    }

    #[test]
    fn the_activity_reports_wake_the_loop() {
        assert!(!back_pressed(), "before the loop listens back is not the app's");
        assert!(!take_back());
        let woken = Arc::new(AtomicUsize::new(0));
        let w = woken.clone();
        set_wake(Arc::new(move || {
            w.fetch_add(1, Ordering::SeqCst);
        }));
        let i = Insets { left: 1, top: 2, right: 3, bottom: 4, keyboard: 5 };
        report_insets(i);
        assert_eq!(insets(), i);
        assert!(!take_back());
        assert!(back_pressed(), "the app takes back");
        assert!(take_back());
        assert!(!take_back(), "taken once");
        editor_action();
        assert!(take_action());
        assert!(!take_action());
        assert_eq!(woken.load(Ordering::SeqCst), 3);
        report_insets(Insets::ZERO);
    }
}
