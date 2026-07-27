use std::{
    cell::RefCell,
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
use libadwaita as adw;
use uuid::Uuid;

const APP_ID: &str = "page.polli.Flashcards";

#[derive(Clone)]
struct AppState {
    preferences: Rc<RefCell<Preferences>>,
    monitor: Rc<RefCell<Option<gio::FileMonitor>>>,
}

struct EditorState {
    path: PathBuf,
    deck: Deck,
    selected: Option<Uuid>,
    dirty: bool,
    updating_form: bool,
    loaded_modified: Option<SystemTime>,
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
    };
    let _ = fs::create_dir_all(&state.preferences.borrow().library);
    install_actions(app, &window, &navigation, &state);
    show_library(&navigation, &window, &state);
    install_monitor(&navigation, &window, &state);
    window.present();
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
        move |_, _| create_deck_dialog(&window, &navigation, &state)
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
        move |_, _| import_dialog(&window, &navigation, &state)
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
    let (toolbar, header) = page_shell("Flashcards", false);
    let new_button = gtk::Button::builder()
        .icon_name("list-add-symbolic")
        .tooltip_text("Create a New Deck")
        .action_name("app.new-deck")
        .build();
    header.pack_start(&new_button);
    let menu = gio::Menu::new();
    menu.append(Some("Import Deck…"), Some("app.import"));
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
        .css_classes(["boxed-list"])
        .build();
    let scrolled = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .child(&list)
        .build();
    content.append(&scrolled);
    toolbar.set_content(Some(&content));

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
                let subtitle = format!(
                    "{} cards  •  {} reviews  •  {:.0}% good  •  {} day streak",
                    summary.cards,
                    summary.reviews,
                    summary.success_rate * 100.0,
                    summary.study_day_streak
                );
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
        let row = if query.is_empty() {
            status_row(
                "No Decks Yet",
                "Create a deck or import an existing JSON file.",
                "view-paged-symbolic",
            )
        } else {
            status_row(
                "No Matching Decks",
                "Try a different search.",
                "system-search-symbolic",
            )
        };
        list.append(&row);
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
    let (toolbar, header) = page_shell(&editor.borrow().deck.title, true);
    let save_button = gtk::Button::builder()
        .label("Save")
        .tooltip_text("Save Changes (Ctrl+S)")
        .sensitive(false)
        .css_classes(["suggested-action"])
        .build();
    header.pack_end(&save_button);
    let source_button = gtk::Button::builder()
        .icon_name("document-edit-symbolic")
        .tooltip_text("Open JSON in Text Editor")
        .build();
    header.pack_end(&source_button);
    let trash_deck = gtk::Button::builder()
        .icon_name("user-trash-symbolic")
        .tooltip_text("Move Deck to Trash")
        .build();
    header.pack_end(&trash_deck);
    let show_cards = gtk::Button::builder()
        .icon_name("sidebar-show-symbolic")
        .tooltip_text("Show Card List")
        .build();
    header.pack_start(&show_cards);

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
        .css_classes(["boxed-list"])
        .build();
    let card_scroll = gtk::ScrolledWindow::builder()
        .min_content_width(260)
        .vexpand(true)
        .child(&card_list)
        .build();
    let add = gtk::Button::builder()
        .label("Add Card")
        .icon_name("list-add-symbolic")
        .build();
    let study = gtk::Button::builder()
        .label("Study")
        .icon_name("media-playback-start-symbolic")
        .css_classes(["suggested-action"])
        .build();
    let sidebar = gtk::Box::new(gtk::Orientation::Vertical, 12);
    sidebar.set_margin_top(12);
    sidebar.set_margin_bottom(12);
    sidebar.set_margin_start(12);
    sidebar.set_margin_end(12);
    sidebar.append(&title);
    sidebar.append(&description);
    sidebar.append(&search);
    sidebar.append(&card_scroll);
    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    actions.append(&add);
    actions.append(&study);
    sidebar.append(&actions);

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
    let duplicate = gtk::Button::builder()
        .label("Duplicate")
        .icon_name("edit-copy-symbolic")
        .build();
    let move_up = gtk::Button::builder()
        .icon_name("go-up-symbolic")
        .tooltip_text("Move Card Up")
        .build();
    let move_down = gtk::Button::builder()
        .icon_name("go-down-symbolic")
        .tooltip_text("Move Card Down")
        .build();
    let delete = gtk::Button::builder()
        .label("Delete")
        .icon_name("user-trash-symbolic")
        .css_classes(["destructive-action"])
        .build();
    let form = gtk::Box::new(gtk::Orientation::Vertical, 12);
    form.set_margin_top(24);
    form.set_margin_bottom(24);
    form.set_margin_start(24);
    form.set_margin_end(24);
    form.append(&field("Front", &front));
    form.append(&field("Back", &back));
    form.append(&field("Tags", &tags));
    let form_actions = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    form_actions.append(&duplicate);
    form_actions.append(&move_up);
    form_actions.append(&move_down);
    form_actions.append(&delete);
    form.append(&form_actions);
    let form_scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .child(&form)
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
    let adaptive = adw::BreakpointBin::builder().child(&split).build();
    let breakpoint = adw::Breakpoint::new(
        adw::BreakpointCondition::parse("max-width: 700px").expect("valid breakpoint"),
    );
    breakpoint.add_setter(&split, "collapsed", Some(&true.to_value()));
    adaptive.add_breakpoint(breakpoint);
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

    connect_text_edit(&front, &editor, &save_button, |card, text| {
        card.front = text
    });
    connect_text_edit(&back, &editor, &save_button, |card, text| card.back = text);
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
            if let Some(id) = editor.borrow().selected
                && let Some(card) = editor.borrow_mut().deck.card_mut(id)
            {
                card.tags = flashcards::model::normalize_tags(tags);
                set_dirty(&editor, &save_button);
            }
        }
    ));
    title.connect_changed(glib::clone!(
        #[weak]
        save_button,
        #[strong]
        editor,
        move |entry| {
            editor.borrow_mut().deck.title = entry.text().to_string();
            set_dirty(&editor, &save_button);
        }
    ));
    description.connect_changed(glib::clone!(
        #[weak]
        save_button,
        #[strong]
        editor,
        move |entry| {
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
        card_list,
        #[weak]
        save_button,
        #[weak]
        detail_stack,
        #[strong]
        editor,
        move |_| {
            let Some(id) = editor.borrow().selected else {
                return;
            };
            editor.borrow_mut().deck.cards.retain(|card| card.id != id);
            editor.borrow_mut().selected = None;
            set_dirty(&editor, &save_button);
            populate_card_list(&card_list, &editor.borrow().deck, "");
            detail_stack.set_visible_child_name("empty");
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
        state,
        #[strong]
        editor,
        move |_| {
            if editor.borrow().dirty && !save_editor(&window, &editor, &save_button) {
                return;
            }
            show_study_setup(&navigation, &window, &state, &editor.borrow().path);
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
    let frame = gtk::Frame::builder().child(child).build();
    container.append(&frame);
    container
}

fn connect_text_edit(
    view: &gtk::TextView,
    editor: &Rc<RefCell<EditorState>>,
    save_button: &gtk::Button,
    update: impl Fn(&mut Card, String) + 'static,
) {
    view.buffer().connect_changed(glib::clone!(
        #[weak]
        save_button,
        #[strong]
        editor,
        move |buffer| {
            if editor.borrow().updating_form {
                return;
            }
            let Some(id) = editor.borrow().selected else {
                return;
            };
            if let Some(card) = editor.borrow_mut().deck.card_mut(id) {
                update(
                    card,
                    buffer
                        .text(&buffer.start_iter(), &buffer.end_iter(), false)
                        .to_string(),
                );
                set_dirty(&editor, &save_button);
            }
        }
    ));
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
            .activatable(true)
            .build();
        row.set_widget_name(&format!("card-{}", card.id));
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
            toast_window(window, "Deck saved");
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
    dialog.add_response("reload", "Reload External");
    dialog.add_response("copy", "Save Native Copy");
    dialog.add_response("overwrite", "Overwrite");
    dialog.set_response_appearance("overwrite", adw::ResponseAppearance::Destructive);
    dialog.set_default_response(Some("copy"));
    dialog.set_close_response("reload");
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
                            toast_window(
                                &window,
                                "External version loaded; reselect a card to refresh the form",
                            );
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
            #[strong]
            state,
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
                show_study(
                    &navigation,
                    &window,
                    &state,
                    &path,
                    chosen_order,
                    tag_filter,
                );
            }
        ),
    );
    dialog.present(Some(window));
}

fn show_study(
    navigation: &adw::NavigationView,
    window: &adw::ApplicationWindow,
    _state: &AppState,
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
    let (toolbar, _header) = page_shell("Study", true);
    let progress = gtk::ProgressBar::builder()
        .show_text(true)
        .margin_top(18)
        .margin_start(18)
        .margin_end(18)
        .build();
    let prompt = gtk::Label::builder()
        .wrap(true)
        .justify(gtk::Justification::Center)
        .selectable(true)
        .css_classes(["title-1", "study-card"])
        .build();
    let answer = gtk::Label::builder()
        .wrap(true)
        .justify(gtk::Justification::Center)
        .selectable(true)
        .css_classes(["title-2", "dim-label"])
        .visible(false)
        .build();
    let reveal = gtk::Button::builder()
        .label("Reveal Answer")
        .css_classes(["suggested-action", "pill"])
        .build();
    let again = gtk::Button::builder()
        .label("Again")
        .css_classes(["destructive-action", "pill"])
        .visible(false)
        .build();
    let good = gtk::Button::builder()
        .label("Good")
        .css_classes(["suggested-action", "pill"])
        .visible(false)
        .build();
    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    buttons.set_halign(gtk::Align::Center);
    buttons.append(&again);
    buttons.append(&reveal);
    buttons.append(&good);
    let content = gtk::Box::new(gtk::Orientation::Vertical, 24);
    content.set_margin_bottom(32);
    content.append(&progress);
    let card = gtk::Box::new(gtk::Orientation::Vertical, 24);
    card.set_vexpand(true);
    card.set_valign(gtk::Align::Center);
    card.set_margin_start(36);
    card.set_margin_end(36);
    card.append(&prompt);
    card.append(&answer);
    content.append(&card);
    content.append(&buttons);
    toolbar.set_content(Some(&content));

    update_study_card(
        &deck,
        &session.borrow(),
        &prompt,
        &answer,
        &progress,
        &again,
    );
    reveal.connect_clicked(glib::clone!(
        #[weak]
        answer,
        #[weak]
        reveal,
        #[weak]
        again,
        #[weak]
        good,
        move |_| {
            answer.set_visible(true);
            reveal.set_visible(false);
            again.set_visible(true);
            good.set_visible(true);
            good.grab_focus();
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
        &reveal,
        &again,
    );
    let page = adw::NavigationPage::new(&toolbar, "Study");
    navigation.push(&page);
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
    prompt: &gtk::Label,
    answer: &gtk::Label,
    progress: &gtk::ProgressBar,
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
            if session.borrow().is_complete() {
                let summary = session.borrow().clone().finish(Utc::now());
                if let Ok(mut latest) = storage::load_deck(&path) {
                    latest.sessions.push(summary);
                    let _ = storage::save_deck(&path, &latest);
                }
                toast_window(&window, "Session complete");
                navigation.pop();
                return;
            }
            let again_button = match rating {
                Rating::Again => button,
                Rating::Good => &other_rating,
            };
            update_study_card(
                &snapshot,
                &session.borrow(),
                &prompt,
                &answer,
                &progress,
                again_button,
            );
            answer.set_visible(false);
            button.set_visible(false);
            other_rating.set_visible(false);
            reveal.set_visible(true);
            reveal.grab_focus();
        }
    ));
}

fn update_study_card(
    deck: &Deck,
    session: &StudySession,
    prompt: &gtk::Label,
    answer: &gtk::Label,
    progress: &gtk::ProgressBar,
    again: &gtk::Button,
) {
    if let Some(card) = session
        .current_card()
        .and_then(|id| deck.cards.iter().find(|card| card.id == id))
    {
        prompt.set_text(&card.front);
        answer.set_text(&card.back);
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
            progress.set_text(Some(&format!("{current} of {}", study_progress.total)));
            progress.set_tooltip_text(Some("Initial card pass"));
        }
        StudyPhase::MissedCards => {
            again.set_label("Bad");
            again.set_tooltip_text(Some("Mark as incorrect without showing it again"));
            progress.add_css_class("warning");
            progress.set_text(Some(&format!(
                "Reviewing missed cards — {current} of {}",
                study_progress.total
            )));
            progress.set_tooltip_text(Some("Reviewing cards rated Again"));
        }
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
        .study-card {
            padding: 42px;
        }
        textview {
            min-height: 110px;
        }
        ",
    );
    gtk::style_context_add_provider_for_display(
        &gdk::Display::default().expect("A display is required"),
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}
