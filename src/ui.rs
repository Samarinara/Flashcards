use std::{
    cell::{Cell, RefCell},
    fs,
    path::{Path, PathBuf},
    rc::Rc,
    time::SystemTime,
};

use adw::{
    gio, glib,
    gtk::{self, gdk},
    prelude::*,
};
use chrono::{Local, Offset, Utc};
use flashcards::{
    model::{Card, Deck, Rating, StudyOrder},
    preferences::Preferences,
    storage::{self, LibraryEntry},
    study::{StudyPhase, StudySession},
};
use glib::subclass::prelude::ObjectSubclassIsExt;
use libadwaita as adw;
use uuid::Uuid;

const APP_ID: &str = "page.polli.Flashcards";
const MINIMAL_DECK_EXAMPLE: &str = r#"{
  "schema_version": 1,
  "id": "9fb0c901-6118-4a68-a5c5-68381d2ca6f8",
  "title": "Capitals",
  "cards": [
    {
      "id": "91dbdba2-33ad-424a-8c4d-314fe77369fe",
      "front": "Portugal",
      "back": "Lisbon",
      "stats": {
        "again_count": 0,
        "good_count": 0,
        "current_good_streak": 0
      }
    }
  ]
}"#;

#[derive(Clone)]
struct AppState {
    preferences: Rc<RefCell<Preferences>>,
    monitor: Rc<RefCell<Option<gio::FileMonitor>>>,
    editor_guard: Rc<RefCell<Option<EditorGuard>>>,
}

#[derive(Clone)]
struct EditorGuard {
    editor: Rc<RefCell<EditorState>>,
    save_button: glib::WeakRef<gtk::Button>,
}

struct EditorState {
    path: PathBuf,
    deck: Deck,
    selected: Option<Uuid>,
    dirty: bool,
    updating_form: bool,
    loaded_modified: Option<SystemTime>,
}

mod flip_card {
    use super::*;
    use gtk::subclass::prelude::*;

    #[derive(Default)]
    pub struct FlipCard {
        pub child: RefCell<Option<gtk::Widget>>,
        pub angle: Cell<f32>,
        pub vertical_offset: Cell<f32>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for FlipCard {
        const NAME: &'static str = "FlashcardsFlipCard";
        type Type = super::FlipCard;
        type ParentType = gtk::Widget;

        fn class_init(class: &mut Self::Class) {
            class.set_layout_manager_type::<gtk::BinLayout>();
        }
    }

    impl ObjectImpl for FlipCard {
        fn dispose(&self) {
            if let Some(child) = self.child.borrow_mut().take() {
                child.unparent();
            }
        }
    }

    impl WidgetImpl for FlipCard {
        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let Some(child) = self.child.borrow().as_ref().cloned() else {
                return;
            };
            let widget = self.obj();
            let center = gtk::graphene::Point::new(
                widget.width() as f32 / 2.0,
                widget.height() as f32 / 2.0,
            );
            let inverse_center = gtk::graphene::Point::new(-center.x(), -center.y());

            snapshot.save();
            snapshot.translate(&gtk::graphene::Point::new(0.0, self.vertical_offset.get()));
            snapshot.translate(&center);
            // A 2D flip also works with GTK's software renderer, which cannot
            // render perspective nodes and would display a pink rectangle.
            snapshot.scale(self.angle.get().to_radians().cos().abs(), 1.0);
            snapshot.translate(&inverse_center);
            widget.snapshot_child(&child, snapshot);
            snapshot.restore();
        }
    }
}

glib::wrapper! {
    pub struct FlipCard(ObjectSubclass<flip_card::FlipCard>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl FlipCard {
    fn new(child: &impl IsA<gtk::Widget>) -> Self {
        let flip_card: Self = glib::Object::builder().build();
        let child = child.clone().upcast::<gtk::Widget>();
        child.set_parent(&flip_card);
        flip_card.imp().child.replace(Some(child));
        flip_card.set_overflow(gtk::Overflow::Visible);
        flip_card
    }

    fn set_angle(&self, angle: f32) {
        self.imp().angle.set(angle);
        self.queue_draw();
    }

    fn set_vertical_offset(&self, offset: f32) {
        self.imp().vertical_offset.set(offset);
        self.queue_draw();
    }
}

pub fn run() -> glib::ExitCode {
    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_startup(|_| install_css());
    app.connect_activate(build_window);
    app.run()
}

fn build_window(app: &adw::Application) {
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("Flashcards")
        .default_width(980)
        .default_height(700)
        .width_request(390)
        .height_request(500)
        .build();
    let navigation = adw::NavigationView::new();
    window.set_content(Some(&navigation));

    let state = AppState {
        preferences: Rc::new(RefCell::new(Preferences::load())),
        monitor: Rc::new(RefCell::new(None)),
        editor_guard: Rc::new(RefCell::new(None)),
    };
    let _ = fs::create_dir_all(&state.preferences.borrow().library);
    install_actions(app, &window, &navigation, &state);
    show_library(&navigation, &window, &state);
    install_monitor(&navigation, &window, &state);
    window.connect_close_request(glib::clone!(
        #[strong]
        state,
        move |window| {
            let dirty = state
                .editor_guard
                .borrow()
                .as_ref()
                .is_some_and(|guard| guard.editor.borrow().dirty);
            if !dirty {
                return glib::Propagation::Proceed;
            }
            let weak_window = window.downgrade();
            leave_editor(window, &state, move || {
                if let Some(window) = weak_window.upgrade() {
                    window.close();
                }
            });
            glib::Propagation::Stop
        }
    ));
    window.present();
}

// All app-level exits use the same save/discard decision as editor navigation.
fn leave_editor(window: &adw::ApplicationWindow, state: &AppState, next: impl Fn() + 'static) {
    let guard = state.editor_guard.borrow().clone();
    let Some(guard) = guard.filter(|guard| guard.editor.borrow().dirty) else {
        next();
        return;
    };
    let dialog = adw::AlertDialog::builder()
        .heading("Save Changes?")
        .body("This deck has unsaved changes. Save them before leaving?")
        .build();
    dialog.add_response("cancel", "Keep Editing");
    dialog.add_response("discard", "Discard Changes");
    dialog.add_response("save", "Save");
    dialog.set_response_appearance("discard", adw::ResponseAppearance::Destructive);
    dialog.set_response_appearance("save", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("save"));
    dialog.set_close_response("cancel");
    dialog.connect_response(
        None,
        glib::clone!(
            #[weak]
            window,
            move |_, response| {
                match response {
                    "discard" => {
                        let path = guard.editor.borrow().path.clone();
                        if let Ok(deck) = storage::load_deck(&path) {
                            let mut editor = guard.editor.borrow_mut();
                            editor.deck = deck;
                            editor.selected = None;
                            editor.loaded_modified = modified_time(&path);
                        }
                        guard.editor.borrow_mut().dirty = false;
                        if let Some(button) = guard.save_button.upgrade() {
                            button.set_sensitive(false);
                        }
                        next();
                    }
                    "save" => {
                        if let Some(button) = guard.save_button.upgrade()
                            && save_editor(&window, &guard.editor, &button)
                        {
                            next();
                        }
                    }
                    _ => {}
                }
            }
        ),
    );
    dialog.present(Some(window));
}

fn install_actions(
    app: &adw::Application,
    window: &adw::ApplicationWindow,
    navigation: &adw::NavigationView,
    state: &AppState,
) {
    app.set_accels_for_action("app.new-deck", &["<Control>n"]);
    app.set_accels_for_action("app.import", &["<Control>o"]);
    app.set_accels_for_action("app.quit", &["<Control>q"]);

    let action = gio::SimpleAction::new("quit", None);
    action.connect_activate(glib::clone!(
        #[weak]
        window,
        move |_, _| window.close()
    ));
    app.add_action(&action);

    let action = gio::SimpleAction::new("new-deck", None);
    action.connect_activate(glib::clone!(
        #[weak]
        window,
        #[weak]
        navigation,
        #[strong]
        state,
        move |_, _| leave_editor(
            &window,
            &state,
            glib::clone!(
                #[weak]
                window,
                #[weak]
                navigation,
                #[strong]
                state,
                move || create_deck_dialog(&window, &navigation, &state)
            )
        )
    ));
    app.add_action(&action);

    let action = gio::SimpleAction::new("import", None);
    action.connect_activate(glib::clone!(
        #[weak]
        window,
        #[weak]
        navigation,
        #[strong]
        state,
        move |_, _| leave_editor(
            &window,
            &state,
            glib::clone!(
                #[weak]
                window,
                #[weak]
                navigation,
                #[strong]
                state,
                move || import_dialog(&window, &navigation, &state)
            )
        )
    ));
    app.add_action(&action);

    let action = gio::SimpleAction::new("preferences", None);
    action.connect_activate(glib::clone!(
        #[weak]
        window,
        #[weak]
        navigation,
        #[strong]
        state,
        move |_, _| show_preferences(&window, &navigation, &state)
    ));
    app.add_action(&action);

    let action = gio::SimpleAction::new("deck-format", None);
    action.connect_activate(glib::clone!(
        #[weak]
        window,
        move |_, _| show_deck_format(&window)
    ));
    app.add_action(&action);

    let action = gio::SimpleAction::new("about", None);
    action.connect_activate(glib::clone!(
        #[weak]
        window,
        move |_, _| {
            adw::AboutDialog::builder()
                .application_name("Flashcards")
                .application_icon(APP_ID)
                .developer_name("Polli")
                .version(env!("CARGO_PKG_VERSION"))
                .comments("Study human-readable JSON decks.")
                .website("https://polli.page")
                .license_type(gtk::License::MitX11)
                .build()
                .present(Some(&window));
        }
    ));
    app.add_action(&action);
}

fn page_shell(title: &str, back: bool) -> (adw::ToolbarView, adw::HeaderBar) {
    let toolbar = adw::ToolbarView::new();
    let header = adw::HeaderBar::new();
    header.set_title_widget(Some(&adw::WindowTitle::builder().title(title).build()));
    header.set_show_back_button(back);
    toolbar.add_top_bar(&header);
    (toolbar, header)
}

fn show_library(
    navigation: &adw::NavigationView,
    window: &adw::ApplicationWindow,
    state: &AppState,
) {
    state.editor_guard.borrow_mut().take();
    let (toolbar, header) = page_shell("Flashcards", false);
    let new_button = gtk::Button::builder()
        .icon_name("list-add-symbolic")
        .tooltip_text("Create a New Deck")
        .action_name("app.new-deck")
        .build();
    header.pack_start(&new_button);
    let menu = gio::Menu::new();
    menu.append(Some("Import Deck…"), Some("app.import"));
    menu.append(Some("Deck File Format"), Some("app.deck-format"));
    menu.append(Some("Preferences"), Some("app.preferences"));
    menu.append(Some("About Flashcards"), Some("app.about"));
    menu.append(Some("Quit"), Some("app.quit"));
    header.pack_end(
        &gtk::MenuButton::builder()
            .icon_name("open-menu-symbolic")
            .tooltip_text("Main Menu")
            .menu_model(&menu)
            .build(),
    );

    let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
    content.set_margin_top(18);
    content.set_margin_bottom(18);
    content.set_margin_start(18);
    content.set_margin_end(18);
    let search = gtk::SearchEntry::builder()
        .placeholder_text("Search decks")
        .hexpand(true)
        .build();
    content.append(&search);
    let list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .valign(gtk::Align::Start)
        .css_classes(["boxed-list"])
        .build();
    let scrolled = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .child(&list)
        .build();
    content.append(&scrolled);
    let clamp = adw::Clamp::builder()
        .maximum_size(800)
        .child(&content)
        .build();
    toolbar.set_content(Some(&clamp));

    refresh_library_list(&list, window, navigation, state, "");
    search.connect_search_changed(glib::clone!(
        #[weak]
        list,
        #[weak]
        window,
        #[weak]
        navigation,
        #[strong]
        state,
        move |entry| refresh_library_list(&list, &window, &navigation, &state, &entry.text())
    ));

    let page = adw::NavigationPage::with_tag(&toolbar, "Flashcards", "library");
    navigation.replace(&[page]);
}

fn show_deck_format(window: &adw::ApplicationWindow) {
    let example = gtk::Label::builder()
        .label(MINIMAL_DECK_EXAMPLE)
        .selectable(true)
        .xalign(0.0)
        .yalign(0.0)
        .css_classes(["monospace"])
        .build();
    let frame = gtk::Frame::builder().child(&example).build();
    frame.set_margin_top(6);
    frame.set_margin_bottom(6);
    frame.set_margin_start(6);
    frame.set_margin_end(6);

    let example_scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Automatic)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .min_content_height(330)
        .child(&frame)
        .build();

    let dialog = adw::AlertDialog::builder()
        .heading("Deck File Format")
        .body(
            "Decks are UTF-8 JSON files. A deck requires schema_version, id, title, and cards. \
             Every card requires id, front, back, and stats; stats requires the three counters \
             shown below. IDs must be unique UUIDs. Optional description, tags, review history, \
             and sessions can be omitted. Unknown fields are rejected.",
        )
        .extra_child(&example_scroll)
        .content_width(620)
        .build();
    dialog.add_response("close", "Close");
    dialog.set_default_response(Some("close"));
    dialog.set_close_response("close");
    dialog.present(Some(window));
}

fn refresh_library_list(
    list: &gtk::ListBox,
    window: &adw::ApplicationWindow,
    navigation: &adw::NavigationView,
    state: &AppState,
    query: &str,
) {
    while let Some(child) = list.first_child() {
        list.remove(&child);
    }
    let library = state.preferences.borrow().library.clone();
    let entries = match storage::scan_library(&library) {
        Ok(entries) => entries,
        Err(error) => {
            list.append(&status_row(
                "Couldn’t Open the Library",
                &error.to_string(),
                "dialog-error-symbolic",
            ));
            return;
        }
    };
    let query = query.trim().to_lowercase();
    let mut shown = 0;
    for entry in entries {
        match entry {
            LibraryEntry::Valid { path, deck } => {
                if !query.is_empty()
                    && !deck.title.to_lowercase().contains(&query)
                    && !deck.description.to_lowercase().contains(&query)
                {
                    continue;
                }
                shown += 1;
                let today = Local::now().date_naive();
                let summary = deck.summary(today);
                let count = format!(
                    "{} card{}",
                    summary.cards,
                    if summary.cards == 1 { "" } else { "s" }
                );
                let subtitle = if summary.reviews == 0 {
                    format!("{count} · Not studied yet")
                } else {
                    format!(
                        "{} · {:.0}% recalled · {} day streak",
                        count,
                        summary.success_rate * 100.0,
                        summary.study_day_streak
                    )
                };
                let row = adw::ActionRow::builder()
                    .title(glib::markup_escape_text(&deck.title))
                    .subtitle(subtitle)
                    .activatable(true)
                    .build();
                row.add_prefix(
                    &gtk::Image::builder()
                        .icon_name("view-paged-symbolic")
                        .pixel_size(24)
                        .build(),
                );
                row.add_suffix(&gtk::Image::builder().icon_name("go-next-symbolic").build());
                row.connect_activated(glib::clone!(
                    #[weak]
                    window,
                    #[weak]
                    navigation,
                    #[strong]
                    state,
                    move |_| open_deck(&navigation, &window, &state, &path)
                ));
                list.append(&row);
            }
            LibraryEntry::Invalid {
                path,
                title,
                error,
                issues,
            } => {
                if !query.is_empty() && !title.to_lowercase().contains(&query) {
                    continue;
                }
                shown += 1;
                let detail = issues
                    .first()
                    .map(|issue| format!("{}: {}", issue.path, issue.message))
                    .unwrap_or(error);
                let row = adw::ActionRow::builder()
                    .title(glib::markup_escape_text(&title))
                    .subtitle(detail)
                    .build();
                row.add_prefix(
                    &gtk::Image::builder()
                        .icon_name("dialog-warning-symbolic")
                        .css_classes(["warning"])
                        .build(),
                );
                let button = gtk::Button::builder()
                    .label("Open in Editor")
                    .valign(gtk::Align::Center)
                    .build();
                button.connect_clicked(glib::clone!(
                    #[weak]
                    window,
                    #[strong]
                    state,
                    move |_| launch_editor(&window, &state, &path)
                ));
                row.add_suffix(&button);
                list.append(&row);
            }
        }
    }
    if shown == 0 {
        let empty = adw::StatusPage::builder()
            .icon_name(if query.is_empty() {
                "view-paged-symbolic"
            } else {
                "system-search-symbolic"
            })
            .title(if query.is_empty() {
                "Your First Deck Starts Here"
            } else {
                "No Matching Decks"
            })
            .description(if query.is_empty() {
                "Create a deck to start learning, or import one you already have."
            } else {
                "Try a different deck name or description."
            })
            .build();
        if query.is_empty() {
            let actions = gtk::Box::new(gtk::Orientation::Vertical, 12);
            actions.set_halign(gtk::Align::Center);
            actions.append(
                &gtk::Button::builder()
                    .label("Create Deck")
                    .action_name("app.new-deck")
                    .css_classes(["suggested-action", "pill"])
                    .build(),
            );
            actions.append(
                &gtk::Button::builder()
                    .label("Import Deck…")
                    .action_name("app.import")
                    .css_classes(["pill"])
                    .build(),
            );
            empty.set_child(Some(&actions));
        }
        let row = gtk::ListBoxRow::builder()
            .selectable(false)
            .activatable(false)
            .child(&empty)
            .build();
        list.append(&row);
        list.remove_css_class("boxed-list");
        list.add_css_class("empty-library");
    } else {
        list.remove_css_class("empty-library");
        list.add_css_class("boxed-list");
    }
}

fn status_row(title: &str, subtitle: &str, icon: &str) -> adw::ActionRow {
    let row = adw::ActionRow::builder()
        .title(title)
        .subtitle(subtitle)
        .build();
    row.add_prefix(&gtk::Image::builder().icon_name(icon).build());
    row
}

fn open_deck(
    navigation: &adw::NavigationView,
    window: &adw::ApplicationWindow,
    state: &AppState,
    path: &Path,
) {
    let deck = match storage::load_deck(path) {
        Ok(deck) => deck,
        Err(error) => {
            toast_window(window, &error.to_string());
            return;
        }
    };
    let editor = Rc::new(RefCell::new(EditorState {
        path: path.to_owned(),
        deck,
        selected: None,
        dirty: false,
        updating_form: false,
        loaded_modified: modified_time(path),
    }));
    let (toolbar, header) = page_shell(&editor.borrow().deck.title, false);
    let window_title = adw::WindowTitle::new(&editor.borrow().deck.title, "All changes saved");
    header.set_title_widget(Some(&window_title));
    let back_button = gtk::Button::builder()
        .icon_name("go-previous-symbolic")
        .tooltip_text("Back to Library")
        .build();
    header.pack_start(&back_button);
    let save_button = gtk::Button::builder()
        .label("Save")
        .tooltip_text("Save Changes (Ctrl+S)")
        .sensitive(false)
        .css_classes(["suggested-action"])
        .build();
    header.pack_end(&save_button);
    *state.editor_guard.borrow_mut() = Some(EditorGuard {
        editor: editor.clone(),
        save_button: save_button.downgrade(),
    });
    let source_button = gtk::Button::builder()
        .icon_name("document-edit-symbolic")
        .tooltip_text("Open JSON in Text Editor")
        .build();

    let trash_deck = gtk::Button::builder()
        .icon_name("user-trash-symbolic")
        .tooltip_text("Move Deck to Trash")
        .build();

    let show_cards = gtk::Button::builder()
        .icon_name("sidebar-show-symbolic")
        .tooltip_text("Show Card List")
        .build();
    header.pack_start(&show_cards);

    let properties = gtk::Button::with_label("Deck Properties…");
    source_button.set_label("Open JSON in Text Editor");
    trash_deck.set_label("Move Deck to Trash…");
    let menu_content = gtk::Box::new(gtk::Orientation::Vertical, 6);
    menu_content.set_margin_top(6);
    menu_content.set_margin_bottom(6);
    menu_content.set_margin_start(6);
    menu_content.set_margin_end(6);
    for button in [&properties, &source_button, &trash_deck] {
        button.add_css_class("flat");
        menu_content.append(button);
    }
    let popover = gtk::Popover::builder().child(&menu_content).build();
    header.pack_end(
        &gtk::MenuButton::builder()
            .icon_name("view-more-symbolic")
            .tooltip_text("Deck Options")
            .popover(&popover)
            .build(),
    );
    for button in [&source_button, &trash_deck] {
        button.connect_clicked(glib::clone!(
            #[weak]
            popover,
            move |_| popover.popdown()
        ));
    }

    let title = gtk::Entry::builder()
        .text(&editor.borrow().deck.title)
        .placeholder_text("Deck title")
        .build();
    let description = gtk::Entry::builder()
        .text(&editor.borrow().deck.description)
        .placeholder_text("Description (optional)")
        .build();
    let search = gtk::SearchEntry::builder()
        .placeholder_text("Search cards")
        .build();
    let card_list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::Single)
        .valign(gtk::Align::Start)
        .css_classes(["boxed-list"])
        .build();
    let card_scroll = gtk::ScrolledWindow::builder()
        .min_content_width(260)
        .vexpand(true)
        .child(&card_list)
        .build();
    let add = gtk::Button::builder().label("Add Card").build();
    let study = gtk::Button::builder()
        .label("Study")
        .css_classes(["suggested-action"])
        .build();
    let sidebar = gtk::Box::new(gtk::Orientation::Vertical, 12);
    sidebar.set_margin_top(12);
    sidebar.set_margin_bottom(12);
    sidebar.set_margin_start(12);
    sidebar.set_margin_end(12);
    properties.connect_clicked(glib::clone!(
        #[weak]
        window,
        #[weak]
        popover,
        #[strong]
        title,
        #[strong]
        description,
        move |_| {
            popover.popdown();
            let fields = gtk::Box::new(gtk::Orientation::Vertical, 12);
            fields.append(&field("Deck Title", &title));
            fields.append(&field("Description", &description));
            let dialog = adw::AlertDialog::builder()
                .heading("Deck Properties")
                .body("Changes are included when you save the deck.")
                .extra_child(&fields)
                .build();
            dialog.add_response("done", "Done");
            dialog.set_close_response("done");
            dialog.connect_closed(glib::clone!(
                #[strong]
                title,
                #[strong]
                description,
                move |_| {
                    // Detach reusable entries from this dialog's field wrappers.
                    if let Some(parent) = title.parent().and_downcast::<gtk::Box>() {
                        parent.remove(&title);
                    }
                    if let Some(parent) = description.parent().and_downcast::<gtk::Box>() {
                        parent.remove(&description);
                    }
                }
            ));
            dialog.present(Some(&window));
        }
    ));
    let cards_heading = gtk::Label::builder()
        .label("Cards")
        .halign(gtk::Align::Start)
        .css_classes(["heading"])
        .build();
    sidebar.append(&cards_heading);
    sidebar.append(&search);
    sidebar.append(&card_scroll);
    add.set_hexpand(true);
    sidebar.append(&add);
    header.pack_start(&study);
    let narrow_study = gtk::Button::builder()
        .label("Study Deck")
        .css_classes(["suggested-action"])
        .visible(false)
        .build();
    narrow_study.connect_clicked(glib::clone!(
        #[weak]
        study,
        move |_| study.emit_clicked()
    ));
    sidebar.append(&narrow_study);

    let empty = adw::StatusPage::builder()
        .icon_name("view-paged-symbolic")
        .title("Select a Card")
        .description("Choose a card from the list, or add a new one.")
        .build();
    let front = gtk::TextView::builder()
        .wrap_mode(gtk::WrapMode::WordChar)
        .top_margin(12)
        .bottom_margin(12)
        .left_margin(12)
        .right_margin(12)
        .build();
    let back = gtk::TextView::builder()
        .wrap_mode(gtk::WrapMode::WordChar)
        .top_margin(12)
        .bottom_margin(12)
        .left_margin(12)
        .right_margin(12)
        .build();
    let tags = gtk::Entry::builder()
        .placeholder_text("grammar, verbs")
        .build();
    let duplicate = gtk::Button::builder().label("Duplicate").build();
    let move_up = gtk::Button::builder()
        .icon_name("go-up-symbolic")
        .tooltip_text("Move Card Up")
        .build();
    let move_down = gtk::Button::builder()
        .icon_name("go-down-symbolic")
        .tooltip_text("Move Card Down")
        .build();
    let delete = gtk::Button::builder()
        .icon_name("user-trash-symbolic")
        .tooltip_text("Delete Card")
        .build();
    let form = gtk::Box::new(gtk::Orientation::Vertical, 12);
    form.set_margin_top(24);
    form.set_margin_bottom(24);
    form.set_margin_start(24);
    form.set_margin_end(24);
    form.append(&field("Front", &front));
    form.append(&field("Back", &back));
    let equation_hint = gtk::Label::builder()
        .label(
            r"Equations: $x^2$ inline or $$\frac{1}{2}$$ on its own line. Rendered when studying.",
        )
        .wrap(true)
        .xalign(0.0)
        .css_classes(["caption", "dim-label"])
        .build();
    form.append(&equation_hint);
    form.append(&field("Tags", &tags));
    let form_actions = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    form_actions.append(&duplicate);
    form_actions.append(&move_up);
    form_actions.append(&move_down);
    let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    form_actions.append(&spacer);
    form_actions.append(&delete);
    form.append(&form_actions);
    let form_clamp = adw::Clamp::builder().maximum_size(720).child(&form).build();
    let form_scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .child(&form_clamp)
        .build();
    let detail_stack = gtk::Stack::builder()
        .transition_type(gtk::StackTransitionType::Crossfade)
        .hexpand(true)
        .vexpand(true)
        .build();
    detail_stack.add_named(&empty, Some("empty"));
    detail_stack.add_named(&form_scroll, Some("form"));
    detail_stack.set_visible_child_name("empty");

    let split = adw::OverlaySplitView::builder()
        .sidebar(&sidebar)
        .content(&detail_stack)
        .min_sidebar_width(260.0)
        .max_sidebar_width(360.0)
        .build();
    let adaptive = adw::BreakpointBin::builder()
        .child(&split)
        .width_request(360)
        .height_request(300)
        .build();
    let breakpoint = adw::Breakpoint::new(
        adw::BreakpointCondition::parse("max-width: 700px").expect("valid breakpoint"),
    );
    breakpoint.add_setter(&split, "collapsed", Some(&true.to_value()));
    breakpoint.add_setter(&study, "visible", Some(&false.to_value()));
    breakpoint.add_setter(&narrow_study, "visible", Some(&true.to_value()));
    adaptive.add_breakpoint(breakpoint);
    show_cards.set_visible(split.is_collapsed());
    split.connect_collapsed_notify(glib::clone!(
        #[weak]
        show_cards,
        #[weak]
        detail_stack,
        move |split| {
            show_cards.set_visible(split.is_collapsed());
            if split.is_collapsed() && detail_stack.visible_child_name().as_deref() == Some("form")
            {
                split.set_show_sidebar(false);
            }
        }
    ));
    toolbar.set_content(Some(&adaptive));
    show_cards.connect_clicked(glib::clone!(
        #[weak]
        split,
        move |_| split.set_show_sidebar(true)
    ));

    populate_card_list(&card_list, &editor.borrow().deck, "");
    search.connect_search_changed(glib::clone!(
        #[weak]
        card_list,
        #[strong]
        editor,
        move |entry| populate_card_list(&card_list, &editor.borrow().deck, &entry.text())
    ));
    card_list.connect_row_selected(glib::clone!(
        #[weak]
        front,
        #[weak]
        back,
        #[weak]
        tags,
        #[weak]
        detail_stack,
        #[weak]
        split,
        #[strong]
        editor,
        move |_, row| {
            let Some(row) = row else {
                // List rebuilds happen while the model may be borrowed. Clear
                // the visible selection without borrowing the model here.
                detail_stack.set_visible_child_name("empty");
                return;
            };
            let Some(id) = row
                .widget_name()
                .strip_prefix("card-")
                .and_then(|id| Uuid::parse_str(id).ok())
            else {
                return;
            };
            editor.borrow_mut().updating_form = true;
            let borrowed = editor.borrow();
            let Some(card) = borrowed.deck.cards.iter().find(|card| card.id == id) else {
                drop(borrowed);
                editor.borrow_mut().updating_form = false;
                return;
            };
            front.buffer().set_text(&card.front);
            back.buffer().set_text(&card.back);
            tags.set_text(&card.tags.join(", "));
            detail_stack.set_visible_child_name("form");
            drop(borrowed);
            let mut editor = editor.borrow_mut();
            editor.selected = Some(id);
            editor.updating_form = false;
            if split.is_collapsed() {
                split.set_show_sidebar(false);
            }
        }
    ));

    connect_text_edit(&front, &editor, &save_button, &card_list, |card, text| {
        card.front = text
    });
    connect_text_edit(&back, &editor, &save_button, &card_list, |card, text| {
        card.back = text
    });
    tags.connect_changed(glib::clone!(
        #[weak]
        save_button,
        #[strong]
        editor,
        move |entry| {
            if editor.borrow().updating_form {
                return;
            }
            let tags = entry
                .text()
                .split(',')
                .map(str::to_owned)
                .collect::<Vec<_>>();
            if mutate_selected_card(&editor, |card| {
                card.tags = flashcards::model::normalize_tags(tags);
            }) {
                save_button.set_sensitive(true);
            }
        }
    ));
    title.connect_changed(glib::clone!(
        #[weak]
        window_title,
        #[weak]
        save_button,
        #[strong]
        editor,
        move |entry| {
            if editor.borrow().updating_form {
                return;
            }
            editor.borrow_mut().deck.title = entry.text().to_string();
            window_title.set_title(&entry.text());
            set_dirty(&editor, &save_button);
        }
    ));
    description.connect_changed(glib::clone!(
        #[weak]
        save_button,
        #[strong]
        editor,
        move |entry| {
            if editor.borrow().updating_form {
                return;
            }
            editor.borrow_mut().deck.description = entry.text().to_string();
            set_dirty(&editor, &save_button);
        }
    ));

    add.connect_clicked(glib::clone!(
        #[weak]
        card_list,
        #[weak]
        save_button,
        #[strong]
        editor,
        move |_| {
            let card = Card::new("New question", "New answer");
            let id = card.id;
            editor.borrow_mut().deck.cards.push(card);
            set_dirty(&editor, &save_button);
            populate_card_list(&card_list, &editor.borrow().deck, "");
            select_card(&card_list, id);
        }
    ));
    duplicate.connect_clicked(glib::clone!(
        #[weak]
        card_list,
        #[weak]
        save_button,
        #[strong]
        editor,
        move |_| {
            let Some(id) = editor.borrow().selected else {
                return;
            };
            let Some(source) = editor
                .borrow()
                .deck
                .cards
                .iter()
                .find(|card| card.id == id)
                .cloned()
            else {
                return;
            };
            let mut card = source;
            card.id = Uuid::new_v4();
            card.stats = Default::default();
            let new_id = card.id;
            editor.borrow_mut().deck.cards.push(card);
            set_dirty(&editor, &save_button);
            populate_card_list(&card_list, &editor.borrow().deck, "");
            select_card(&card_list, new_id);
        }
    ));
    connect_move_card(-1, &move_up, &card_list, &save_button, &editor);
    connect_move_card(1, &move_down, &card_list, &save_button, &editor);
    delete.connect_clicked(glib::clone!(
        #[weak]
        window,
        #[weak]
        card_list,
        #[weak]
        save_button,
        #[weak]
        detail_stack,
        #[weak]
        search,
        #[strong]
        editor,
        move |_| {
            let Some(id) = editor.borrow().selected else {
                return;
            };
            let dialog = adw::AlertDialog::builder()
                .heading("Delete Card?")
                .body("The card will be removed from this deck when you save.")
                .build();
            dialog.add_response("cancel", "Cancel");
            dialog.add_response("delete", "Delete Card");
            dialog.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
            dialog.set_close_response("cancel");
            dialog.connect_response(
                None,
                glib::clone!(
                    #[weak]
                    card_list,
                    #[weak]
                    save_button,
                    #[weak]
                    detail_stack,
                    #[weak]
                    search,
                    #[strong]
                    editor,
                    move |_, response| {
                        if response != "delete" {
                            return;
                        }
                        editor.borrow_mut().deck.cards.retain(|card| card.id != id);
                        editor.borrow_mut().selected = None;
                        set_dirty(&editor, &save_button);
                        populate_card_list(&card_list, &editor.borrow().deck, &search.text());
                        detail_stack.set_visible_child_name("empty");
                    }
                ),
            );
            dialog.present(Some(&window));
        }
    ));
    save_button.connect_clicked(glib::clone!(
        #[weak]
        window,
        #[weak]
        save_button,
        #[strong]
        editor,
        move |_| {
            save_editor(&window, &editor, &save_button);
        }
    ));
    source_button.connect_clicked(glib::clone!(
        #[weak]
        window,
        #[strong]
        state,
        #[strong]
        editor,
        move |_| launch_editor(&window, &state, &editor.borrow().path)
    ));
    trash_deck.connect_clicked(glib::clone!(
        #[weak]
        window,
        #[weak]
        navigation,
        #[strong]
        state,
        #[strong]
        editor,
        move |_| {
            let dialog = adw::AlertDialog::builder()
                .heading("Move Deck to Trash?")
                .body("The JSON file can be recovered from the system Trash.")
                .build();
            dialog.add_response("cancel", "Cancel");
            dialog.add_response("trash", "Move to Trash");
            dialog.set_response_appearance("trash", adw::ResponseAppearance::Destructive);
            dialog.set_close_response("cancel");
            dialog.connect_response(
                None,
                glib::clone!(
                    #[weak]
                    window,
                    #[weak]
                    navigation,
                    #[strong]
                    state,
                    #[strong]
                    editor,
                    move |_, response| {
                        if response != "trash" {
                            return;
                        }
                        let file = gio::File::for_path(&editor.borrow().path);
                        match file.trash(gio::Cancellable::NONE) {
                            Ok(()) => {
                                show_library(&navigation, &window, &state);
                                toast_window(&window, "Deck moved to Trash");
                            }
                            Err(error) => toast_window(
                                &window,
                                &format!("Couldn’t move deck to Trash: {error}"),
                            ),
                        }
                    }
                ),
            );
            dialog.present(Some(&window));
        }
    ));
    study.connect_clicked(glib::clone!(
        #[weak]
        window,
        #[weak]
        navigation,
        #[weak]
        save_button,
        #[strong]
        editor,
        move |_| {
            if editor.borrow().dirty && !save_editor(&window, &editor, &save_button) {
                return;
            }
            show_study_setup(&navigation, &window, &editor.borrow().path);
        }
    ));

    let save_action = gio::SimpleAction::new("save-deck", None);
    save_action.connect_activate(glib::clone!(
        #[weak]
        window,
        #[weak]
        save_button,
        #[strong]
        editor,
        move |_, _| {
            save_editor(&window, &editor, &save_button);
        }
    ));
    window.add_action(&save_action);
    if let Some(app) = window.application() {
        app.set_accels_for_action("win.save-deck", &["<Control>s"]);
    }

    let page = adw::NavigationPage::new(&toolbar, &editor.borrow().deck.title);
    save_button.connect_sensitive_notify(glib::clone!(
        #[weak]
        page,
        #[weak]
        window_title,
        move |button| {
            page.set_can_pop(!button.is_sensitive());
            window_title.set_subtitle(if button.is_sensitive() {
                "Unsaved changes"
            } else {
                "All changes saved"
            });
        }
    ));
    back_button.connect_clicked(glib::clone!(
        #[weak]
        window,
        #[weak]
        navigation,
        #[strong]
        state,
        move |_| leave_editor(
            &window,
            &state,
            glib::clone!(
                #[weak]
                window,
                #[weak]
                navigation,
                #[strong]
                state,
                move || show_library(&navigation, &window, &state)
            )
        )
    ));
    // Save/conflict resolution can normalize or replace the model. Refresh after
    // the save callback releases its borrow, preserving selection where possible.
    save_button.connect_sensitive_notify(glib::clone!(
        #[weak]
        card_list,
        #[weak]
        search,
        #[weak]
        title,
        #[weak]
        description,
        #[weak]
        detail_stack,
        #[weak]
        window_title,
        #[strong]
        editor,
        move |button| {
            if button.is_sensitive() {
                return;
            }
            glib::idle_add_local_once(glib::clone!(
                #[weak]
                card_list,
                #[weak]
                search,
                #[weak]
                title,
                #[weak]
                description,
                #[weak]
                detail_stack,
                #[weak]
                window_title,
                #[strong]
                editor,
                move || {
                    let (deck, selected) = {
                        let state = editor.borrow();
                        (state.deck.clone(), state.selected)
                    };
                    editor.borrow_mut().updating_form = true;
                    title.set_text(&deck.title);
                    description.set_text(&deck.description);
                    window_title.set_title(&deck.title);
                    editor.borrow_mut().updating_form = false;
                    populate_card_list(&card_list, &deck, &search.text());
                    detail_stack.set_visible_child_name("empty");
                    if let Some(id) = selected {
                        select_card(&card_list, id);
                    }
                }
            ));
        }
    ));
    page.connect_shown(glib::clone!(
        #[strong]
        editor,
        #[weak]
        save_button,
        move |_| {
            let path = editor.borrow().path.clone();
            if editor.borrow().dirty || editor.borrow().loaded_modified == modified_time(&path) {
                return;
            }
            if let Ok(deck) = storage::load_deck(&path) {
                let mut state = editor.borrow_mut();
                state.deck = deck;
                state.loaded_modified = modified_time(&path);
                drop(state);
                save_button.notify("sensitive");
            }
        }
    ));
    navigation.push(&page);
}

fn field(label: &str, child: &impl IsA<gtk::Widget>) -> gtk::Box {
    let container = gtk::Box::new(gtk::Orientation::Vertical, 6);
    let label = gtk::Label::builder()
        .label(label)
        .halign(gtk::Align::Start)
        .css_classes(["heading"])
        .build();
    container.append(&label);
    if child.is::<gtk::TextView>() {
        let frame = gtk::Frame::builder().child(child).build();
        frame.add_css_class("card-input");
        container.append(&frame);
    } else {
        container.append(child);
    }
    container
}

fn connect_text_edit(
    view: &gtk::TextView,
    editor: &Rc<RefCell<EditorState>>,
    save_button: &gtk::Button,
    card_list: &gtk::ListBox,
    update: impl Fn(&mut Card, String) + 'static,
) {
    view.buffer().connect_changed(glib::clone!(
        #[weak]
        card_list,
        #[weak]
        save_button,
        #[strong]
        editor,
        move |buffer| {
            if editor.borrow().updating_form {
                return;
            }
            if mutate_selected_card(&editor, |card| {
                update(
                    card,
                    buffer
                        .text(&buffer.start_iter(), &buffer.end_iter(), false)
                        .to_string(),
                );
            }) {
                save_button.set_sensitive(true);
                let state = editor.borrow();
                if let Some(card) = state
                    .deck
                    .cards
                    .iter()
                    .find(|card| Some(card.id) == state.selected)
                {
                    let mut child = card_list.first_child();
                    while let Some(widget) = child {
                        if widget.widget_name() == format!("card-{}", card.id) {
                            if let Some(row) = widget.downcast_ref::<adw::ActionRow>() {
                                row.set_title(&glib::markup_escape_text(&single_line(&card.front)));
                                row.set_subtitle(&glib::markup_escape_text(&single_line(
                                    &card.back,
                                )));
                            }
                            break;
                        }
                        child = widget.next_sibling();
                    }
                }
            }
        }
    ));
}

fn mutate_selected_card(editor: &Rc<RefCell<EditorState>>, update: impl FnOnce(&mut Card)) -> bool {
    let mut editor = editor.borrow_mut();
    let Some(id) = editor.selected else {
        return false;
    };
    let Some(card) = editor.deck.card_mut(id) else {
        return false;
    };
    update(card);
    editor.dirty = true;
    true
}

fn connect_move_card(
    direction: isize,
    button: &gtk::Button,
    card_list: &gtk::ListBox,
    save_button: &gtk::Button,
    editor: &Rc<RefCell<EditorState>>,
) {
    button.connect_clicked(glib::clone!(
        #[weak]
        card_list,
        #[weak]
        save_button,
        #[strong]
        editor,
        move |_| {
            let Some(id) = editor.borrow().selected else {
                return;
            };
            let Some(index) = editor
                .borrow()
                .deck
                .cards
                .iter()
                .position(|card| card.id == id)
            else {
                return;
            };
            let target = index as isize + direction;
            if target < 0 || target >= editor.borrow().deck.cards.len() as isize {
                return;
            }
            editor.borrow_mut().deck.cards.swap(index, target as usize);
            set_dirty(&editor, &save_button);
            populate_card_list(&card_list, &editor.borrow().deck, "");
            select_card(&card_list, id);
        }
    ));
}

fn set_dirty(editor: &Rc<RefCell<EditorState>>, save_button: &gtk::Button) {
    editor.borrow_mut().dirty = true;
    save_button.set_sensitive(true);
}

fn populate_card_list(list: &gtk::ListBox, deck: &Deck, query: &str) {
    while let Some(child) = list.first_child() {
        list.remove(&child);
    }
    let query = query.trim().to_lowercase();
    for card in &deck.cards {
        if !query.is_empty()
            && !card.front.to_lowercase().contains(&query)
            && !card.back.to_lowercase().contains(&query)
            && !card.tags.iter().any(|tag| tag.contains(&query))
        {
            continue;
        }
        let row = adw::ActionRow::builder()
            .title(glib::markup_escape_text(&single_line(&card.front)))
            .subtitle(glib::markup_escape_text(&single_line(&card.back)))
            .title_lines(1)
            .subtitle_lines(1)
            .activatable(true)
            .build();
        row.set_widget_name(&format!("card-{}", card.id));
        list.append(&row);
    }
    if list.first_child().is_none() {
        let row = status_row(
            if query.is_empty() {
                "No Cards Yet"
            } else {
                "No Matching Cards"
            },
            if query.is_empty() {
                "Add a card to start building this deck."
            } else {
                "Try another question, answer, or tag."
            },
            "system-search-symbolic",
        );
        row.set_selectable(false);
        list.append(&row);
    }
}

fn select_card(list: &gtk::ListBox, id: Uuid) {
    let wanted = format!("card-{id}");
    let mut child = list.first_child();
    while let Some(widget) = child {
        if widget.widget_name() == wanted {
            if let Ok(row) = widget.downcast::<gtk::ListBoxRow>() {
                list.select_row(Some(&row));
            }
            return;
        }
        child = widget.next_sibling();
    }
}

fn save_editor(
    window: &adw::ApplicationWindow,
    editor: &Rc<RefCell<EditorState>>,
    save_button: &gtk::Button,
) -> bool {
    let mut state = editor.borrow_mut();
    state.deck.normalize();
    let current_modified = modified_time(&state.path);
    if state.dirty && state.loaded_modified.is_some() && current_modified != state.loaded_modified {
        drop(state);
        show_save_conflict(window, editor, save_button);
        return false;
    }
    match storage::save_deck(&state.path, &state.deck) {
        Ok(()) => {
            state.dirty = false;
            state.loaded_modified = modified_time(&state.path);
            save_button.set_sensitive(false);
            true
        }
        Err(error) => {
            toast_window(window, &error.to_string());
            false
        }
    }
}

fn show_save_conflict(
    window: &adw::ApplicationWindow,
    editor: &Rc<RefCell<EditorState>>,
    save_button: &gtk::Button,
) {
    let dialog = adw::AlertDialog::builder()
        .heading("The JSON Changed Externally")
        .body("Choose which version to keep. Saving a copy preserves both versions.")
        .build();
    dialog.add_response("cancel", "Keep Editing");
    dialog.add_response("reload", "Reload External");
    dialog.add_response("copy", "Save a Copy");
    dialog.add_response("overwrite", "Overwrite");
    dialog.set_response_appearance("overwrite", adw::ResponseAppearance::Destructive);
    dialog.set_default_response(Some("copy"));
    dialog.set_close_response("cancel");
    dialog.connect_response(
        None,
        glib::clone!(
            #[weak]
            window,
            #[weak]
            save_button,
            #[strong]
            editor,
            move |_, response| match response {
                "reload" => {
                    let path = editor.borrow().path.clone();
                    match storage::load_deck(&path) {
                        Ok(deck) => {
                            let mut editor = editor.borrow_mut();
                            editor.deck = deck;
                            editor.selected = None;
                            editor.dirty = false;
                            editor.loaded_modified = modified_time(&path);
                            save_button.set_sensitive(false);
                            toast_window(&window, "External version loaded");
                        }
                        Err(error) => toast_window(&window, &error.to_string()),
                    }
                }
                "copy" => {
                    let mut editor = editor.borrow_mut();
                    editor.deck.normalize();
                    let parent = editor.path.parent().unwrap_or_else(|| Path::new("."));
                    let path = storage::unique_deck_path(
                        parent,
                        &format!("{} native copy", editor.deck.title),
                    );
                    match storage::save_deck(&path, &editor.deck) {
                        Ok(()) => {
                            editor.path = path;
                            editor.dirty = false;
                            editor.loaded_modified = modified_time(&editor.path);
                            save_button.set_sensitive(false);
                            toast_window(&window, "Native changes saved as a copy");
                        }
                        Err(error) => toast_window(&window, &error.to_string()),
                    }
                }
                "overwrite" => {
                    let mut editor = editor.borrow_mut();
                    editor.deck.normalize();
                    match storage::save_deck(&editor.path, &editor.deck) {
                        Ok(()) => {
                            editor.dirty = false;
                            editor.loaded_modified = modified_time(&editor.path);
                            save_button.set_sensitive(false);
                            toast_window(&window, "External version overwritten");
                        }
                        Err(error) => toast_window(&window, &error.to_string()),
                    }
                }
                _ => {}
            }
        ),
    );
    dialog.present(Some(window));
}

fn show_study_setup(
    navigation: &adw::NavigationView,
    window: &adw::ApplicationWindow,
    path: &Path,
) {
    let deck = match storage::load_deck(path) {
        Ok(deck) => deck,
        Err(error) => {
            toast_window(window, &error.to_string());
            return;
        }
    };
    if deck.cards.is_empty() {
        toast_window(window, "Add at least one card before studying");
        return;
    }
    let dialog = adw::AlertDialog::builder()
        .heading("Start a Study Session")
        .body("Choose the card order and optional tag filter.")
        .build();
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("start", "Start");
    dialog.set_response_appearance("start", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("start"));
    dialog.set_close_response("cancel");
    let controls = gtk::Box::new(gtk::Orientation::Vertical, 12);
    controls.set_margin_top(12);
    let order = adw::ComboRow::builder()
        .title("Order")
        .model(&gtk::StringList::new(&["Sequential", "Shuffled"]))
        .selected(1)
        .build();
    let tags = gtk::Entry::builder()
        .placeholder_text("Optional tags, separated by commas")
        .build();
    controls.append(&order);
    controls.append(&tags);
    dialog.set_extra_child(Some(&controls));
    let path = path.to_owned();
    dialog.connect_response(
        None,
        glib::clone!(
            #[weak]
            window,
            #[weak]
            navigation,
            move |_, response| {
                if response != "start" {
                    return;
                }
                let chosen_order = if order.selected() == 0 {
                    StudyOrder::Sequential
                } else {
                    StudyOrder::Shuffled
                };
                let tag_filter = flashcards::model::normalize_tags(
                    tags.text()
                        .split(',')
                        .map(str::to_owned)
                        .collect::<Vec<_>>(),
                );
                show_study(&navigation, &window, &path, chosen_order, tag_filter);
            }
        ),
    );
    dialog.present(Some(window));
}

fn show_study(
    navigation: &adw::NavigationView,
    window: &adw::ApplicationWindow,
    path: &Path,
    order: StudyOrder,
    tag_filter: Vec<String>,
) {
    let deck = match storage::load_deck(path) {
        Ok(deck) => deck,
        Err(error) => {
            toast_window(window, &error.to_string());
            return;
        }
    };
    let session = StudySession::new(&deck, order, tag_filter);
    if session.current_card().is_none() {
        toast_window(window, "No cards match those tags");
        return;
    }
    let deck = Rc::new(deck);
    let session = Rc::new(RefCell::new(session));
    let path = Rc::new(path.to_owned());
    let (toolbar, header) = page_shell("Study", true);
    header.set_title_widget(Some(&adw::WindowTitle::new(&deck.title, "Study")));
    let phase = gtk::Label::builder()
        .halign(gtk::Align::Start)
        .hexpand(true)
        .css_classes(["heading"])
        .build();
    let counter = gtk::Label::builder()
        .halign(gtk::Align::End)
        .css_classes(["dim-label"])
        .build();
    let status = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    status.append(&phase);
    status.append(&counter);
    let progress = gtk::ProgressBar::builder()
        .css_classes(["study-progress"])
        .build();
    let progress_box = gtk::Box::new(gtk::Orientation::Vertical, 8);
    progress_box.append(&status);
    progress_box.append(&progress);
    let progress_clamp = adw::Clamp::builder()
        .maximum_size(720)
        .margin_top(18)
        .margin_start(24)
        .margin_end(24)
        .child(&progress_box)
        .build();
    let question_label = gtk::Label::builder()
        .label("Question")
        .halign(gtk::Align::Start)
        .css_classes(["caption", "dim-label"])
        .build();
    let prompt = crate::card_content::new("title-1", 20);
    let answer_label = gtk::Label::builder()
        .label("Answer")
        .halign(gtk::Align::Start)
        .css_classes(["caption", "dim-label"])
        .build();
    let answer = crate::card_content::new("title-2", 16);
    let reveal = gtk::Button::builder()
        .label("Show Answer")
        .css_classes(["suggested-action", "pill"])
        .hexpand(true)
        .build();
    let again = gtk::Button::builder()
        .label("Again")
        .css_classes(["pill"])
        .hexpand(true)
        .build();
    let good = gtk::Button::builder()
        .label("Good")
        .css_classes(["suggested-action", "pill"])
        .hexpand(true)
        .build();
    let ratings = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    ratings.set_homogeneous(true);
    ratings.append(&again);
    ratings.append(&good);
    let actions = gtk::Stack::builder()
        .transition_type(gtk::StackTransitionType::Crossfade)
        .transition_duration(160)
        .build();
    actions.add_named(&reveal, Some("reveal"));
    actions.add_named(&ratings, Some("ratings"));
    actions.set_visible_child_name("reveal");
    let actions_clamp = adw::Clamp::builder()
        .maximum_size(520)
        .margin_start(24)
        .margin_end(24)
        .margin_bottom(28)
        .child(&actions)
        .build();
    let question_content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    question_content.set_vexpand(true);
    question_content.append(&question_label);
    let question_scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Automatic)
        .vexpand(true)
        .child(&prompt)
        .build();
    let question_face = gtk::Box::new(gtk::Orientation::Vertical, 0);
    question_face.add_css_class("card");
    question_face.add_css_class("study-card");
    question_content.append(&question_scroll);
    question_face.append(&question_content);
    let answer_content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    answer_content.set_vexpand(true);
    answer_content.append(&answer_label);
    let answer_scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Automatic)
        .vexpand(true)
        .child(&answer)
        .build();
    let answer_face = gtk::Box::new(gtk::Orientation::Vertical, 0);
    answer_face.add_css_class("card");
    answer_face.add_css_class("study-card");
    answer_content.append(&answer_scroll);
    answer_face.append(&answer_content);
    let faces = gtk::Stack::builder()
        .hhomogeneous(true)
        .vhomogeneous(true)
        .transition_type(gtk::StackTransitionType::None)
        .build();
    faces.add_named(&question_face, Some("question"));
    faces.add_named(&answer_face, Some("answer"));
    faces.set_visible_child_name("question");
    let card = FlipCard::new(&faces);
    let card_clamp = adw::Clamp::builder()
        .maximum_size(720)
        .margin_top(24)
        .margin_bottom(24)
        .margin_start(24)
        .margin_end(24)
        .vexpand(true)
        .child(&card)
        .build();
    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content.append(&progress_clamp);
    content.append(&card_clamp);
    content.append(&actions_clamp);
    let confetti_progress = Rc::new(Cell::new(1.0));
    let confetti = gtk::DrawingArea::builder()
        .can_target(false)
        .hexpand(true)
        .vexpand(true)
        .visible(false)
        .build();
    confetti.set_draw_func({
        let confetti_progress = Rc::clone(&confetti_progress);
        move |_, context, width, height| {
            draw_confetti(context, width, height, confetti_progress.get());
        }
    });
    let overlay = gtk::Overlay::new();
    overlay.set_child(Some(&content));
    overlay.add_overlay(&confetti);
    let card_animation = Rc::new(RefCell::new(None));
    let flip_animation = Rc::new(RefCell::new(None));
    let confetti_animation = Rc::new(RefCell::new(None));
    toolbar.set_content(Some(&overlay));

    update_study_card(
        &deck,
        &session.borrow(),
        &prompt,
        &answer,
        &progress,
        &phase,
        &counter,
        &again,
    );
    reveal.connect_clicked(glib::clone!(
        #[weak]
        card,
        #[weak]
        faces,
        #[weak]
        actions,
        #[weak]
        good,
        #[strong]
        flip_animation,
        move |_| {
            animate_card_flip(&card, &faces, &actions, &good, &flip_animation);
        }
    ));
    connect_rating(
        Rating::Again,
        &again,
        window,
        navigation,
        &deck,
        &session,
        &path,
        &prompt,
        &answer,
        &progress,
        &phase,
        &counter,
        &card,
        &card_animation,
        &faces,
        &confetti,
        &confetti_progress,
        &confetti_animation,
        &actions,
        &reveal,
        &good,
    );
    connect_rating(
        Rating::Good,
        &good,
        window,
        navigation,
        &deck,
        &session,
        &path,
        &prompt,
        &answer,
        &progress,
        &phase,
        &counter,
        &card,
        &card_animation,
        &faces,
        &confetti,
        &confetti_progress,
        &confetti_animation,
        &actions,
        &reveal,
        &again,
    );
    let page = adw::NavigationPage::new(&toolbar, "Study");
    navigation.push(&page);
    glib::idle_add_local_once(glib::clone!(
        #[weak]
        card,
        #[weak]
        actions,
        #[weak]
        reveal,
        #[strong]
        card_animation,
        move || animate_card_entrance(&card, &actions, &reveal, &card_animation)
    ));
}

#[allow(clippy::too_many_arguments)]
fn connect_rating(
    rating: Rating,
    button: &gtk::Button,
    window: &adw::ApplicationWindow,
    navigation: &adw::NavigationView,
    snapshot: &Rc<Deck>,
    session: &Rc<RefCell<StudySession>>,
    path: &Rc<PathBuf>,
    prompt: &gtk::TextView,
    answer: &gtk::TextView,
    progress: &gtk::ProgressBar,
    phase: &gtk::Label,
    counter: &gtk::Label,
    card: &FlipCard,
    card_animation: &Rc<RefCell<Option<adw::TimedAnimation>>>,
    faces: &gtk::Stack,
    confetti: &gtk::DrawingArea,
    confetti_progress: &Rc<Cell<f64>>,
    confetti_animation: &Rc<RefCell<Option<adw::TimedAnimation>>>,
    actions: &gtk::Stack,
    reveal: &gtk::Button,
    other_rating: &gtk::Button,
) {
    button.connect_clicked(glib::clone!(
        #[weak]
        window,
        #[weak]
        navigation,
        #[weak]
        prompt,
        #[weak]
        answer,
        #[weak]
        progress,
        #[weak]
        phase,
        #[weak]
        counter,
        #[weak]
        card,
        #[weak]
        faces,
        #[weak]
        confetti,
        #[weak]
        actions,
        #[strong]
        card_animation,
        #[strong]
        confetti_progress,
        #[strong]
        confetti_animation,
        #[weak]
        reveal,
        #[weak]
        other_rating,
        #[strong]
        snapshot,
        #[strong]
        session,
        #[strong]
        path,
        move |button| {
            let Some(card_id) = session.borrow().current_card() else {
                return;
            };
            let mut latest = match storage::load_deck(&path) {
                Ok(deck) => deck,
                Err(error) => {
                    toast_window(&window, &format!("Couldn’t record rating: {error}"));
                    return;
                }
            };
            let offset = Local::now().offset().fix();
            if let Err(error) =
                latest.record_rating(card_id, session.borrow().id, rating, Utc::now(), offset)
            {
                toast_window(&window, &error);
                return;
            }
            if let Err(error) = storage::save_deck(&path, &latest) {
                toast_window(&window, &format!("Couldn’t record rating: {error}"));
                return;
            }
            session.borrow_mut().rate(rating);
            let complete = session.borrow().is_complete();
            if complete {
                let summary = session.borrow().clone().finish(Utc::now());
                if let Ok(mut latest) = storage::load_deck(&path) {
                    latest.sessions.push(summary);
                    let _ = storage::save_deck(&path, &latest);
                }
            }
            actions.set_sensitive(false);
            let rated_button = button.clone();
            let exit_direction = match rating {
                Rating::Again => 1.0,
                Rating::Good => -1.0,
            };
            animate_card_exit(
                &card,
                exit_direction,
                &card_animation,
                glib::clone!(
                    #[weak]
                    window,
                    #[weak]
                    navigation,
                    #[weak]
                    prompt,
                    #[weak]
                    answer,
                    #[weak]
                    progress,
                    #[weak]
                    phase,
                    #[weak]
                    counter,
                    #[weak]
                    card,
                    #[weak]
                    faces,
                    #[weak]
                    confetti,
                    #[weak]
                    actions,
                    #[weak]
                    reveal,
                    #[weak]
                    rated_button,
                    #[weak]
                    other_rating,
                    #[strong]
                    snapshot,
                    #[strong]
                    path,
                    #[strong]
                    session,
                    #[strong]
                    card_animation,
                    #[strong]
                    confetti_progress,
                    #[strong]
                    confetti_animation,
                    move || {
                        if complete {
                            show_study_summary(
                                &window,
                                &navigation,
                                &snapshot,
                                &session.borrow(),
                                &path,
                            );
                            return;
                        }
                        let again_button = match rating {
                            Rating::Again => &rated_button,
                            Rating::Good => &other_rating,
                        };
                        update_study_card(
                            &snapshot,
                            &session.borrow(),
                            &prompt,
                            &answer,
                            &progress,
                            &phase,
                            &counter,
                            again_button,
                        );
                        card.set_angle(0.0);
                        faces.set_visible_child_name("question");
                        actions.set_visible_child_name("reveal");
                        animate_card_entrance(&card, &actions, &reveal, &card_animation);
                        if rating == Rating::Good {
                            animate_confetti(&confetti, &confetti_progress, &confetti_animation);
                        }
                    }
                ),
            );
        }
    ));
}

fn show_study_summary(
    window: &adw::ApplicationWindow,
    navigation: &adw::NavigationView,
    deck: &Deck,
    session: &StudySession,
    path: &Path,
) {
    let summary = session.clone().finish(Utc::now());
    let (toolbar, _) = page_shell("Session Complete", true);
    let results = adw::StatusPage::builder()
        .icon_name("object-select-symbolic")
        .title("Session Complete")
        .description(&deck.title)
        .build();
    let content = gtk::Box::new(gtk::Orientation::Vertical, 24);
    content.set_halign(gtk::Align::Center);
    let totals = gtk::Label::builder()
        .label(format!(
            "{} card{} studied\nRatings: {} recalled · {} still learning",
            summary.unique_card_count,
            if summary.unique_card_count == 1 {
                ""
            } else {
                "s"
            },
            summary.good_count,
            summary.again_count
        ))
        .justify(gtk::Justification::Center)
        .wrap(true)
        .build();
    content.append(&totals);
    let restart = gtk::Button::builder()
        .label("Study Again")
        .css_classes(["suggested-action", "pill"])
        .build();
    let done = gtk::Button::builder()
        .label("Back to Deck")
        .css_classes(["pill"])
        .build();
    content.append(&restart);
    content.append(&done);
    results.set_child(Some(&content));
    toolbar.set_content(Some(&results));
    let path = path.to_owned();
    let order = session.order;
    let tags = session.tag_filter.clone();
    restart.connect_clicked(glib::clone!(
        #[weak]
        window,
        #[weak]
        navigation,
        move |_| {
            navigation.pop();
            show_study(&navigation, &window, &path, order, tags.clone());
        }
    ));
    done.connect_clicked(glib::clone!(
        #[weak]
        navigation,
        move |_| {
            navigation.pop();
        }
    ));
    navigation.pop();
    navigation.push(&adw::NavigationPage::new(&toolbar, "Session Complete"));
}

#[allow(clippy::too_many_arguments)]
fn update_study_card(
    deck: &Deck,
    session: &StudySession,
    prompt: &gtk::TextView,
    answer: &gtk::TextView,
    progress: &gtk::ProgressBar,
    phase: &gtk::Label,
    counter: &gtk::Label,
    again: &gtk::Button,
) {
    if let Some(card) = session
        .current_card()
        .and_then(|id| deck.cards.iter().find(|card| card.id == id))
    {
        crate::card_content::set_text(prompt, &card.front, 20);
        crate::card_content::set_text(answer, &card.back, 16);
        for label in [prompt, answer] {
            if let Some(scroll) = label
                .ancestor(gtk::ScrolledWindow::static_type())
                .and_downcast::<gtk::ScrolledWindow>()
            {
                let adjustment = scroll.vadjustment();
                adjustment.set_value(adjustment.lower());
            }
        }
    }
    let study_progress = session.progress();
    progress.set_fraction(if study_progress.total == 0 {
        0.0
    } else {
        study_progress.completed as f64 / study_progress.total as f64
    });
    let current = (study_progress.completed + 1).min(study_progress.total);
    match study_progress.phase {
        StudyPhase::FirstPass => {
            again.set_label("Again");
            again.set_tooltip_text(Some("Show this card once more at the end"));
            progress.remove_css_class("warning");
            phase.set_text("First pass");
            counter.set_text(&format!("Card {current} of {}", study_progress.total));
            progress.set_tooltip_text(Some("Initial card pass"));
        }
        StudyPhase::MissedCards => {
            again.set_label("Still Learning");
            again.set_tooltip_text(Some(
                "Record that this card needs more practice; continue to the next card",
            ));
            progress.add_css_class("warning");
            phase.set_text("Final review");
            counter.set_text(&format!("Card {current} of {}", study_progress.total));
            progress.set_tooltip_text(Some("Reviewing cards rated Again"));
        }
    }
}

fn animate_card_flip(
    card: &FlipCard,
    faces: &gtk::Stack,
    actions: &gtk::Stack,
    good: &gtk::Button,
    current_animation: &Rc<RefCell<Option<adw::TimedAnimation>>>,
) {
    actions.set_sensitive(false);
    if !adw::is_animations_enabled(card) {
        card.set_angle(0.0);
        faces.set_visible_child_name("answer");
        actions.set_visible_child_name("ratings");
        actions.set_sensitive(true);
        good.grab_focus();
        return;
    }

    let showing_answer = Rc::new(Cell::new(false));
    let target = adw::CallbackAnimationTarget::new(glib::clone!(
        #[weak]
        card,
        #[weak]
        faces,
        #[strong]
        showing_answer,
        move |value| {
            let second_half = value >= 0.5;
            if second_half && !showing_answer.replace(true) {
                faces.set_visible_child_name("answer");
            }
            let angle = if second_half {
                ((value - 1.0) * 180.0) as f32
            } else {
                (value * 180.0) as f32
            };
            card.set_angle(angle);
        }
    ));
    let animation = adw::TimedAnimation::new(card, 0.0, 1.0, 420, target);
    animation.set_easing(adw::Easing::EaseInOutCubic);
    animation.set_follow_enable_animations_setting(true);
    animation.connect_done(glib::clone!(
        #[weak]
        card,
        #[weak]
        faces,
        #[weak]
        actions,
        #[weak]
        good,
        move |_| {
            card.set_angle(0.0);
            faces.set_visible_child_name("answer");
            actions.set_visible_child_name("ratings");
            actions.set_sensitive(true);
            good.grab_focus();
        }
    ));
    animation.play();
    *current_animation.borrow_mut() = Some(animation);
}

fn animate_card_entrance(
    card: &FlipCard,
    actions: &gtk::Stack,
    reveal: &gtk::Button,
    current_animation: &Rc<RefCell<Option<adw::TimedAnimation>>>,
) {
    actions.set_sensitive(false);
    card.set_angle(0.0);
    if !adw::is_animations_enabled(card) {
        card.set_vertical_offset(0.0);
        card.set_opacity(1.0);
        actions.set_sensitive(true);
        reveal.grab_focus();
        return;
    }

    let distance = (card.height() as f64 + 48.0).max(120.0);
    card.set_vertical_offset(distance as f32);
    card.set_opacity(0.35);
    let target = adw::CallbackAnimationTarget::new(glib::clone!(
        #[weak]
        card,
        move |value| {
            card.set_vertical_offset((distance * (1.0 - value)) as f32);
            card.set_opacity(0.35 + value * 0.65);
        }
    ));
    let animation = adw::TimedAnimation::new(card, 0.0, 1.0, 280, target);
    animation.set_easing(adw::Easing::EaseOutCubic);
    animation.set_follow_enable_animations_setting(true);
    animation.connect_done(glib::clone!(
        #[weak]
        card,
        #[weak]
        actions,
        #[weak]
        reveal,
        move |_| {
            card.set_vertical_offset(0.0);
            card.set_opacity(1.0);
            actions.set_sensitive(true);
            reveal.grab_focus();
        }
    ));
    *current_animation.borrow_mut() = Some(animation.clone());
    animation.play();
}

fn animate_card_exit(
    card: &FlipCard,
    direction: f64,
    current_animation: &Rc<RefCell<Option<adw::TimedAnimation>>>,
    on_done: impl FnOnce() + 'static,
) {
    if !adw::is_animations_enabled(card) {
        card.set_vertical_offset(0.0);
        card.set_opacity(1.0);
        on_done();
        return;
    }

    let distance = (card.height() as f64 + 48.0).max(120.0) * direction;
    let target = adw::CallbackAnimationTarget::new(glib::clone!(
        #[weak]
        card,
        move |value| {
            card.set_vertical_offset((distance * value) as f32);
            card.set_opacity(1.0 - value * 0.65);
        }
    ));
    let animation = adw::TimedAnimation::new(card, 0.0, 1.0, 240, target);
    animation.set_easing(adw::Easing::EaseInCubic);
    animation.set_follow_enable_animations_setting(true);
    let on_done = Rc::new(RefCell::new(Some(on_done)));
    animation.connect_done(move |_| {
        if let Some(on_done) = on_done.borrow_mut().take() {
            on_done();
        }
    });
    *current_animation.borrow_mut() = Some(animation.clone());
    animation.play();
}

fn animate_confetti(
    layer: &gtk::DrawingArea,
    progress: &Rc<Cell<f64>>,
    current_animation: &Rc<RefCell<Option<adw::TimedAnimation>>>,
) {
    if !adw::is_animations_enabled(layer) {
        return;
    }
    progress.set(0.0);
    layer.set_visible(true);
    let target = adw::CallbackAnimationTarget::new(glib::clone!(
        #[weak]
        layer,
        #[strong]
        progress,
        move |value| {
            progress.set(value);
            layer.queue_draw();
        }
    ));
    let animation = adw::TimedAnimation::new(layer, 0.0, 1.0, 1_200, target);
    animation.set_easing(adw::Easing::EaseOutCubic);
    animation.set_follow_enable_animations_setting(true);
    animation.connect_done(glib::clone!(
        #[weak]
        layer,
        move |_| layer.set_visible(false)
    ));
    animation.play();
    *current_animation.borrow_mut() = Some(animation);
}

fn draw_confetti(context: &gtk::cairo::Context, width: i32, height: i32, progress: f64) {
    const COLORS: [(f64, f64, f64); 6] = [
        (0.208, 0.518, 0.894),
        (0.200, 0.820, 0.478),
        (0.965, 0.827, 0.176),
        (1.000, 0.471, 0.000),
        (0.569, 0.255, 0.675),
        (0.835, 0.380, 0.600),
    ];
    const PARTICLES: [(f64, f64, f64, f64); 18] = [
        (-0.48, -1.30, -1.8, 5.0),
        (-0.40, -0.95, 1.2, 4.0),
        (-0.34, -1.55, -0.7, 5.0),
        (-0.28, -1.15, 2.0, 3.5),
        (-0.20, -1.42, -1.3, 4.5),
        (-0.13, -1.05, 0.8, 4.0),
        (-0.07, -1.62, -2.1, 5.0),
        (-0.02, -1.22, 1.5, 3.5),
        (0.05, -1.48, -0.9, 4.5),
        (0.11, -1.08, 1.8, 4.0),
        (0.17, -1.58, -1.6, 5.0),
        (0.23, -1.18, 0.6, 3.5),
        (0.29, -1.38, 2.2, 4.5),
        (0.35, -1.02, -1.1, 4.0),
        (0.41, -1.50, 1.4, 5.0),
        (0.47, -1.20, -2.0, 3.5),
        (-0.53, -1.08, 0.9, 4.0),
        (0.54, -1.34, -0.5, 4.5),
    ];

    let width = f64::from(width);
    let height = f64::from(height);
    let spread = width.min(620.0);
    let origin_x = width / 2.0;
    let origin_y = height * 0.78;
    let alpha = (1.0 - ((progress - 0.66) / 0.34).clamp(0.0, 1.0)) * 0.9;

    for (index, &(velocity_x, velocity_y, spin, size)) in PARTICLES.iter().enumerate() {
        let x = origin_x + velocity_x * spread * progress;
        let y = origin_y + velocity_y * 150.0 * progress + 210.0 * progress * progress;
        let (red, green, blue) = COLORS[index % COLORS.len()];
        let _ = context.save();
        context.translate(x, y);
        context.rotate(spin * progress);
        context.set_source_rgba(red, green, blue, alpha);
        if index % 3 == 0 {
            context.arc(0.0, 0.0, size / 2.0, 0.0, std::f64::consts::TAU);
        } else {
            context.rectangle(-size / 2.0, -size, size, size * 2.0);
        }
        let _ = context.fill();
        let _ = context.restore();
    }
}

fn create_deck_dialog(
    window: &adw::ApplicationWindow,
    navigation: &adw::NavigationView,
    state: &AppState,
) {
    let dialog = adw::AlertDialog::builder()
        .heading("Create a New Deck")
        .body("Give the deck a short, descriptive name.")
        .build();
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("create", "Create");
    dialog.set_response_appearance("create", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("create"));
    dialog.set_close_response("cancel");
    let entry = gtk::Entry::builder()
        .placeholder_text("Deck name")
        .activates_default(true)
        .build();
    dialog.set_extra_child(Some(&entry));
    dialog.connect_response(
        None,
        glib::clone!(
            #[weak]
            window,
            #[weak]
            navigation,
            #[strong]
            state,
            move |_, response| {
                if response != "create" {
                    return;
                }
                let title = entry.text().trim().to_owned();
                if title.is_empty() {
                    toast_window(&window, "Enter a deck name");
                    return;
                }
                let deck = Deck::new(&title);
                let library = state.preferences.borrow().library.clone();
                let path = storage::unique_deck_path(&library, &title);
                match storage::save_deck(&path, &deck) {
                    Ok(()) => {
                        show_library(&navigation, &window, &state);
                        open_deck(&navigation, &window, &state, &path);
                    }
                    Err(error) => toast_window(&window, &error.to_string()),
                }
            }
        ),
    );
    dialog.present(Some(window));
}

fn import_dialog(
    window: &adw::ApplicationWindow,
    navigation: &adw::NavigationView,
    state: &AppState,
) {
    let dialog = gtk::FileDialog::builder().title("Import a Deck").build();
    let filter = gtk::FileFilter::new();
    filter.set_name(Some("JSON decks"));
    filter.add_mime_type("application/json");
    filter.add_pattern("*.json");
    let filters = gio::ListStore::new::<gtk::FileFilter>();
    filters.append(&filter);
    dialog.set_filters(Some(&filters));
    dialog.open(
        Some(window),
        gio::Cancellable::NONE,
        glib::clone!(
            #[weak]
            window,
            #[weak]
            navigation,
            #[strong]
            state,
            move |result| {
                let Ok(file) = result else { return };
                let Some(source) = file.path() else {
                    toast_window(&window, "Only local JSON files can be imported");
                    return;
                };
                match storage::import_deck(&source, &state.preferences.borrow().library) {
                    Ok(path) => {
                        show_library(&navigation, &window, &state);
                        open_deck(&navigation, &window, &state, &path);
                    }
                    Err(error) => toast_window(&window, &error.to_string()),
                }
            }
        ),
    );
}

fn launch_editor(window: &adw::ApplicationWindow, state: &AppState, path: &Path) {
    let file = gio::File::for_path(path);
    if !is_flatpak()
        && !state.preferences.borrow().ask_editor_each_time
        && let Some(app_id) = state.preferences.borrow().editor_app_id.clone()
        && let Some(app) = gio::AppInfo::all()
            .into_iter()
            .find(|app| app.id().as_deref() == Some(app_id.as_str()))
    {
        if let Err(error) = app.launch(&[file], gio::AppLaunchContext::NONE) {
            toast_window(window, &format!("Couldn’t open the editor: {error}"));
        }
        return;
    }
    let launcher = gtk::FileLauncher::new(Some(&file));
    launcher.set_always_ask(state.preferences.borrow().ask_editor_each_time);
    launcher.set_writable(true);
    launcher.launch(
        Some(window),
        gio::Cancellable::NONE,
        glib::clone!(
            #[weak]
            window,
            move |result| {
                if let Err(error) = result {
                    toast_window(&window, &format!("Couldn’t open the editor: {error}"));
                }
            }
        ),
    );
}

fn show_preferences(
    window: &adw::ApplicationWindow,
    navigation: &adw::NavigationView,
    state: &AppState,
) {
    let dialog = adw::PreferencesDialog::new();
    dialog.set_title("Preferences");
    let page = adw::PreferencesPage::new();
    let library_group = adw::PreferencesGroup::builder().title("Library").build();
    let folder = adw::ActionRow::builder()
        .title("Library Folder")
        .subtitle(state.preferences.borrow().library.to_string_lossy())
        .activatable(true)
        .build();
    folder.add_suffix(&gtk::Image::builder().icon_name("go-next-symbolic").build());
    library_group.add(&folder);
    page.add(&library_group);
    let editor_group = adw::PreferencesGroup::builder()
        .title("Text Editor")
        .build();
    let ask = adw::SwitchRow::builder()
        .title("Ask Which Editor to Use")
        .subtitle("Otherwise, use the preferred or system-default editor")
        .active(state.preferences.borrow().ask_editor_each_time)
        .build();
    editor_group.add(&ask);
    if !is_flatpak() {
        let editor_name = selected_editor_name(&state.preferences.borrow())
            .unwrap_or_else(|| "System Default".to_owned());
        let editor = adw::ActionRow::builder()
            .title("Preferred Editor")
            .subtitle(&editor_name)
            .activatable(true)
            .build();
        editor.add_suffix(&gtk::Image::builder().icon_name("go-next-symbolic").build());
        let reset = gtk::Button::builder()
            .label("Use System Default")
            .halign(gtk::Align::Start)
            .build();
        editor_group.add(&editor);
        editor_group.add(&reset);
        editor.connect_activated(glib::clone!(
            #[weak]
            window,
            #[weak]
            editor,
            #[strong]
            state,
            move |_| choose_editor(&window, &editor, &state)
        ));
        reset.connect_clicked(glib::clone!(
            #[weak]
            editor,
            #[strong]
            state,
            move |_| {
                state.preferences.borrow_mut().editor_app_id = None;
                let _ = state.preferences.borrow().save();
                editor.set_subtitle("System Default");
            }
        ));
    }
    page.add(&editor_group);
    dialog.add(&page);

    ask.connect_active_notify(glib::clone!(
        #[strong]
        state,
        move |row| {
            state.preferences.borrow_mut().ask_editor_each_time = row.is_active();
            let _ = state.preferences.borrow().save();
        }
    ));
    folder.connect_activated(glib::clone!(
        #[weak]
        window,
        #[weak]
        navigation,
        #[weak]
        folder,
        #[strong]
        state,
        move |_| {
            let chooser = gtk::FileDialog::builder()
                .title("Choose Library Folder")
                .build();
            chooser.select_folder(
                Some(&window),
                gio::Cancellable::NONE,
                glib::clone!(
                    #[weak]
                    window,
                    #[weak]
                    navigation,
                    #[weak]
                    folder,
                    #[strong]
                    state,
                    move |result| {
                        let Ok(file) = result else { return };
                        let Some(path) = file.path() else { return };
                        state.preferences.borrow_mut().library = path.clone();
                        let _ = state.preferences.borrow().save();
                        let _ = fs::create_dir_all(&path);
                        folder.set_subtitle(&path.to_string_lossy());
                        install_monitor(&navigation, &window, &state);
                    }
                ),
            );
        }
    ));
    dialog.present(Some(window));
}

fn install_monitor(
    navigation: &adw::NavigationView,
    window: &adw::ApplicationWindow,
    state: &AppState,
) {
    let file = gio::File::for_path(&state.preferences.borrow().library);
    let Ok(monitor) =
        file.monitor_directory(gio::FileMonitorFlags::WATCH_MOVES, gio::Cancellable::NONE)
    else {
        return;
    };
    monitor.set_rate_limit(500);
    monitor.connect_changed(glib::clone!(
        #[weak]
        navigation,
        #[weak]
        window,
        #[strong]
        state,
        move |_, _, _, _| {
            if navigation.visible_page_tag().as_deref() == Some("library") {
                show_library(&navigation, &window, &state);
            }
        }
    ));
    *state.monitor.borrow_mut() = Some(monitor);
}

#[allow(deprecated)]
fn choose_editor(window: &adw::ApplicationWindow, row: &adw::ActionRow, state: &AppState) {
    let chooser = gtk::AppChooserDialog::for_content_type(
        Some(window),
        gtk::DialogFlags::MODAL,
        "application/json",
    );
    chooser.set_heading("Choose a text editor for deck JSON");
    chooser.connect_response(glib::clone!(
        #[weak]
        row,
        #[strong]
        state,
        move |dialog, response| {
            if response == gtk::ResponseType::Ok
                && let Some(app) = dialog.app_info()
                && let Some(id) = app.id()
            {
                state.preferences.borrow_mut().editor_app_id = Some(id.to_string());
                let _ = state.preferences.borrow().save();
                row.set_subtitle(&app.display_name());
            }
            dialog.close();
        }
    ));
    chooser.present();
}

fn selected_editor_name(preferences: &Preferences) -> Option<String> {
    let wanted = preferences.editor_app_id.as_deref()?;
    gio::AppInfo::all()
        .into_iter()
        .find(|app| app.id().as_deref() == Some(wanted))
        .map(|app| app.display_name().to_string())
}

fn is_flatpak() -> bool {
    Path::new("/.flatpak-info").exists()
}

fn toast_window(window: &adw::ApplicationWindow, message: &str) {
    let toast = adw::Toast::new(message);
    if let Some(content) = window.content() {
        if let Ok(overlay) = content.clone().downcast::<adw::ToastOverlay>() {
            overlay.add_toast(toast);
            return;
        }
        let overlay = adw::ToastOverlay::new();
        window.set_content(None::<&gtk::Widget>);
        overlay.set_child(Some(&content));
        window.set_content(Some(&overlay));
        overlay.add_toast(toast);
    }
}

fn modified_time(path: &Path) -> Option<SystemTime> {
    fs::metadata(path).ok()?.modified().ok()
}

fn single_line(text: &str) -> String {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.chars().count() > 72 {
        format!("{}…", text.chars().take(72).collect::<String>())
    } else {
        text
    }
}

fn install_css() {
    let provider = gtk::CssProvider::new();
    provider.load_from_string(
        "
        .empty-library,
        .empty-library > row {
            background: transparent;
        }
        .study-card {
            padding: 24px;
            border-radius: 18px;
        }
        .study-content,
        .study-content text {
            background-color: transparent;
        }
        .study-progress trough,
        .study-progress progress {
            min-height: 8px;
            border-radius: 999px;
        }
        .card-input textview {
            min-height: 140px;
            border-radius: 8px;
        }
        .card-input {
            border-radius: 8px;
        }
        ",
    );
    gtk::style_context_add_provider_for_display(
        &gdk::Display::default().expect("A display is required"),
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    // Run under a desktop session or xvfb-run with --ignored --test-threads=1.
    #[test]
    #[ignore = "requires a graphical display"]
    fn gui_editing_and_study_flow() {
        adw::init().expect("GTK initialization");
        gtk::Settings::default()
            .unwrap()
            .set_gtk_enable_animations(false);
        install_css();
        let temporary = tempfile::tempdir().unwrap();
        let app = adw::Application::builder()
            .application_id("page.polli.Flashcards.GuiTest")
            .build();
        app.register(gio::Cancellable::NONE).unwrap();
        let window = adw::ApplicationWindow::builder()
            .application(&app)
            .default_width(980)
            .default_height(700)
            .build();
        let navigation = adw::NavigationView::new();
        window.set_content(Some(&navigation));
        let state = AppState {
            preferences: Rc::new(RefCell::new(Preferences {
                library: temporary.path().to_owned(),
                ..Preferences::default()
            })),
            monitor: Rc::new(RefCell::new(None)),
            editor_guard: Rc::new(RefCell::new(None)),
        };
        install_actions(&app, &window, &navigation, &state);
        show_library(&navigation, &window, &state);
        window.present();
        settle();
        assert!(
            widgets::<gtk::Button>(&window)
                .iter()
                .any(|button| button.label().as_deref() == Some("Create Deck"))
        );
        capture(&window, "library-empty");

        let mut deck = Deck::new("Capitals");
        deck.cards.push(Card::new("Portugal", "Lisbon"));
        let path = temporary.path().join("capitals.json");
        storage::save_deck(&path, &deck).unwrap();
        show_library(&navigation, &window, &state);
        settle();
        capture(&window, "library");
        open_deck(&navigation, &window, &state, &path);
        settle();
        let list = widgets::<gtk::ListBox>(&window)
            .into_iter()
            .find(|list| list.selection_mode() == gtk::SelectionMode::Single)
            .unwrap();
        select_card(&list, deck.cards[0].id);
        let views = widgets::<gtk::TextView>(&window);
        views[0].buffer().set_text("Updated question");
        let guard = state.editor_guard.borrow().clone().unwrap();
        assert!(guard.editor.borrow().dirty);
        let row = list
            .row_at_index(0)
            .unwrap()
            .downcast::<adw::ActionRow>()
            .unwrap();
        assert_eq!(row.title(), "Updated question");
        assert!(!navigation.visible_page().unwrap().can_pop());
        settle();
        capture(&window, "editor");

        let continued = Rc::new(Cell::new(false));
        leave_editor(&window, &state, {
            let continued = continued.clone();
            move || continued.set(true)
        });
        respond(&window, "Keep Editing");
        settle();
        assert!(!continued.get());
        assert!(guard.editor.borrow().dirty);
        leave_editor(&window, &state, {
            let continued = continued.clone();
            move || continued.set(true)
        });
        respond(&window, "Save");
        settle();
        assert!(continued.get());
        assert_eq!(
            storage::load_deck(&path).unwrap().cards[0].front,
            "Updated question"
        );
        assert!(!guard.editor.borrow().dirty);
        assert!(navigation.visible_page().unwrap().can_pop());

        // Reopening properties must not reparent entries still owned by a dialog.
        let properties = widgets::<gtk::Button>(&window)
            .into_iter()
            .find(|button| button.label().as_deref() == Some("Deck Properties…"))
            .unwrap();
        for _ in 0..2 {
            properties.emit_clicked();
            settle();
            let dialog = window
                .visible_dialog()
                .unwrap()
                .downcast::<adw::AlertDialog>()
                .unwrap();
            let entries = widgets::<gtk::Entry>(&dialog);
            entries[0].set_text("Renamed Deck");
            respond(&window, "Done");
            settle();
        }
        assert!(
            widgets::<adw::WindowTitle>(&window)
                .iter()
                .any(|title| title.title() == "Renamed Deck")
        );
        leave_editor(&window, &state, || {});
        respond(&window, "Discard Changes");
        settle();
        assert_eq!(guard.editor.borrow().deck.title, "Capitals");
        assert!(!guard.editor.borrow().dirty);
        select_card(&list, deck.cards[0].id);

        window.set_default_size(390, 600);
        settle();
        capture(&window, "editor-narrow");
        show_study(&navigation, &window, &path, StudyOrder::Sequential, vec![]);
        settle();
        capture(&window, "study-narrow");
        let reveal = widgets::<gtk::Button>(&window)
            .into_iter()
            .find(|button| button.label().as_deref() == Some("Show Answer"))
            .unwrap();
        reveal.emit_clicked();
        settle();
        let good = widgets::<gtk::Button>(&window)
            .into_iter()
            .find(|button| button.label().as_deref() == Some("Good"))
            .unwrap();
        good.emit_clicked();
        settle();
        assert_eq!(
            navigation.visible_page().unwrap().title(),
            "Session Complete"
        );
        assert_eq!(storage::load_deck(&path).unwrap().sessions.len(), 1);
        capture(&window, "completion");
        widgets::<gtk::Button>(&window)
            .into_iter()
            .find(|button| button.label().as_deref() == Some("Back to Deck"))
            .unwrap()
            .emit_clicked();
        settle();
        assert_eq!(guard.editor.borrow().deck.sessions.len(), 1);

        // A reload must replace both the sidebar preview and the editable form.
        let mut external = storage::load_deck(&path).unwrap();
        external.cards[0].front = "External question".into();
        storage::save_deck(&path, &external).unwrap();
        views[0].buffer().set_text("Local conflicting question");
        guard.editor.borrow_mut().loaded_modified = Some(SystemTime::UNIX_EPOCH);
        assert!(!save_editor(
            &window,
            &guard.editor,
            &guard.save_button.upgrade().unwrap()
        ));
        respond(&window, "Reload External");
        settle();
        let row = list
            .row_at_index(0)
            .unwrap()
            .downcast::<adw::ActionRow>()
            .unwrap();
        assert_eq!(row.title(), "External question");
        assert!(!guard.editor.borrow().dirty);

        window.set_default_size(980, 700);
        adw::StyleManager::default().set_color_scheme(adw::ColorScheme::ForceDark);
        external.cards[0].front = "A longer question with several lines of text. ".repeat(35);
        storage::save_deck(&path, &external).unwrap();
        show_study(&navigation, &window, &path, StudyOrder::Sequential, vec![]);
        settle();
        capture(&window, "study-long-dark");
        let long_content = widgets::<gtk::TextView>(&window)
            .into_iter()
            .find(|view| view.tooltip_text().as_deref() == Some(&external.cards[0].front))
            .unwrap();
        let scroll = long_content
            .ancestor(gtk::ScrolledWindow::static_type())
            .and_downcast::<gtk::ScrolledWindow>()
            .unwrap();
        assert!(scroll.vadjustment().upper() > scroll.vadjustment().page_size());

        // Both faces render math without changing the portable source text.
        external.cards[0].front = r"Solve $ax^2 + bx + c = 0$.".into();
        external.cards[0].back = r"$$x = \frac{-b \pm \sqrt{b^2 - 4ac}}{2a}$$".into();
        storage::save_deck(&path, &external).unwrap();
        adw::StyleManager::default().set_color_scheme(adw::ColorScheme::ForceLight);
        window.set_default_size(390, 600);
        show_study(&navigation, &window, &path, StudyOrder::Sequential, vec![]);
        settle();
        let equation_views: Vec<_> = widgets::<gtk::TextView>(&window)
            .into_iter()
            .filter(|view| {
                let source = view.tooltip_text();
                source.as_deref() == Some(&external.cards[0].front)
                    || source.as_deref() == Some(&external.cards[0].back)
            })
            .collect();
        assert_eq!(equation_views.len(), 2);
        for view in &equation_views {
            let buffer = view.buffer();
            assert!(
                (0..buffer.char_count())
                    .any(|offset| buffer.iter_at_offset(offset).paintable().is_some())
            );
            assert!(!view.is_editable());
        }
        let light_pixels = equation_pixels(&equation_views[0]);
        capture(&window, "study-equations-light");
        widgets::<gtk::Button>(&navigation.visible_page().unwrap())
            .into_iter()
            .find(|button| button.label().as_deref() == Some("Show Answer"))
            .unwrap()
            .emit_clicked();
        settle();
        capture(&window, "study-equation-answer");
        adw::StyleManager::default().set_color_scheme(adw::ColorScheme::ForceDark);
        settle();
        for view in &equation_views {
            let buffer = view.buffer();
            assert!(
                (0..buffer.char_count())
                    .any(|offset| buffer.iter_at_offset(offset).paintable().is_some())
            );
        }
        assert_ne!(light_pixels, equation_pixels(&equation_views[0]));
        capture(&window, "study-equations-dark");
        let stored = storage::load_deck(&path).unwrap();
        assert_eq!(stored.cards[0].front, external.cards[0].front);
        assert_eq!(stored.cards[0].back, external.cards[0].back);

        external.cards[0].front = r"Invalid $\frac{$ and <plain> & text".into();
        storage::save_deck(&path, &external).unwrap();
        show_study(&navigation, &window, &path, StudyOrder::Sequential, vec![]);
        settle();
        let fallback = widgets::<gtk::TextView>(&window)
            .into_iter()
            .find(|view| view.tooltip_text().as_deref() == Some(&external.cards[0].front))
            .unwrap();
        let buffer = fallback.buffer();
        assert_eq!(
            buffer.text(&buffer.start_iter(), &buffer.end_iter(), true),
            external.cards[0].front
        );

        external.cards[0].front = format!("$$ {} $$", vec!["x^2"; 40].join(" + "));
        storage::save_deck(&path, &external).unwrap();
        show_study(&navigation, &window, &path, StudyOrder::Sequential, vec![]);
        settle();
        let wide = widgets::<gtk::TextView>(&window)
            .into_iter()
            .find(|view| view.tooltip_text().as_deref() == Some(&external.cards[0].front))
            .unwrap();
        assert!(!equation_pixels(&wide).is_empty());
        let scroll = wide
            .ancestor(gtk::ScrolledWindow::static_type())
            .and_downcast::<gtk::ScrolledWindow>()
            .unwrap();
        assert!(scroll.hadjustment().upper() > scroll.hadjustment().page_size());
        window.close();
    }

    fn equation_pixels(view: &gtk::TextView) -> Vec<u8> {
        let buffer = view.buffer();
        let texture = (0..buffer.char_count())
            .find_map(|offset| buffer.iter_at_offset(offset).paintable())
            .unwrap()
            .downcast::<gdk::Texture>()
            .unwrap();
        let stride = texture.width() as usize * 4;
        let mut pixels = vec![0; stride * texture.height() as usize];
        texture.download(&mut pixels, stride);
        pixels
    }

    fn widgets<T: IsA<gtk::Widget> + glib::object::IsClass>(
        root: &impl IsA<gtk::Widget>,
    ) -> Vec<T> {
        fn visit<T: IsA<gtk::Widget> + glib::object::IsClass>(
            widget: &gtk::Widget,
            found: &mut Vec<T>,
        ) {
            if let Ok(item) = widget.clone().downcast::<T>() {
                found.push(item);
            }
            let mut child = widget.first_child();
            while let Some(widget) = child {
                visit(&widget, found);
                child = widget.next_sibling();
            }
        }
        let mut found = Vec::new();
        visit(root.as_ref(), &mut found);
        found
    }

    fn respond(window: &adw::ApplicationWindow, label: &str) {
        let dialog = window.visible_dialog().expect("an open dialog");
        widgets::<gtk::Button>(&dialog)
            .into_iter()
            .find(|button| button.label().as_deref() == Some(label))
            .expect("dialog response button")
            .emit_clicked();
    }

    fn settle() {
        let loop_ = glib::MainLoop::new(None, false);
        glib::timeout_add_local_once(std::time::Duration::from_millis(150), {
            let loop_ = loop_.clone();
            move || loop_.quit()
        });
        loop_.run();
    }

    fn capture(window: &adw::ApplicationWindow, name: &str) {
        let Ok(directory) = std::env::var("FLASHCARDS_SCREENSHOTS") else {
            return;
        };
        let paintable = gtk::WidgetPaintable::new(Some(window));
        for _ in 0..10 {
            let snapshot = gtk::Snapshot::new();
            paintable.snapshot(&snapshot, window.width() as f64, window.height() as f64);
            if let Some(node) = snapshot.to_node() {
                let texture = window.renderer().unwrap().render_texture(&node, None);
                texture
                    .save_to_png(Path::new(&directory).join(format!("{name}.png")))
                    .unwrap();
                return;
            }
            window.queue_draw();
            settle();
        }
        panic!("window snapshot was not ready: {name}");
    }

    #[test]
    fn selected_card_mutation_does_not_nest_refcell_borrows() {
        let mut deck = Deck::new("Deck");
        let card = Card::new("Old front", "Back");
        let card_id = card.id;
        deck.cards.push(card);
        let editor = Rc::new(RefCell::new(EditorState {
            path: PathBuf::from("deck.json"),
            deck,
            selected: Some(card_id),
            dirty: false,
            updating_form: false,
            loaded_modified: None,
        }));

        assert!(mutate_selected_card(&editor, |card| {
            card.front = "New front".to_owned();
        }));
        let editor = editor.borrow();
        assert!(editor.dirty);
        assert_eq!(editor.deck.cards[0].front, "New front");
    }

    #[test]
    fn menu_deck_example_is_valid() {
        let deck: Deck =
            serde_json::from_str(MINIMAL_DECK_EXAMPLE).expect("example should deserialize");

        assert!(deck.validate().is_empty());
    }
}
