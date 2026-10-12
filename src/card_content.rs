use flashcards::equations::{self, Segment};
use libadwaita::{self as adw, glib, gtk, prelude::*};

/// A selectable native text view with equation images embedded among the prose.
pub fn new(css_class: &str, font_size_pt: i64) -> gtk::TextView {
    let view = gtk::TextView::builder()
        .editable(false)
        .cursor_visible(false)
        .wrap_mode(gtk::WrapMode::WordChar)
        .justification(gtk::Justification::Center)
        .css_classes([css_class, "study-content"])
        .margin_top(18)
        .margin_bottom(6)
        .build();
    let manager = adw::StyleManager::default();
    let handler = manager.connect_dark_notify(glib::clone!(
        #[weak]
        view,
        move |_| {
            if let Some(source) = view.tooltip_text() {
                set_text(&view, &source, font_size_pt);
            }
        }
    ));
    let handler = std::cell::RefCell::new(Some(handler));
    view.connect_destroy(move |_| {
        if let Some(handler) = handler.borrow_mut().take() {
            manager.disconnect(handler);
        }
    });
    view
}

pub fn set_text(view: &gtk::TextView, source: &str, font_size_pt: i64) {
    // Keep source available on hover and expose it to assistive technology.
    view.set_tooltip_text(Some(source));
    view.update_property(&[gtk::accessible::Property::Label(source)]);
    let buffer = view.buffer();
    buffer.set_text("");
    let dark = adw::StyleManager::default().is_dark();
    let mut iter = buffer.start_iter();
    for segment in equations::segments(source) {
        match segment {
            Segment::Text(text) => buffer.insert(&mut iter, text),
            Segment::Math {
                source,
                latex,
                display,
            } => {
                let texture = equations::render_png(latex, display, font_size_pt, dark)
                    .ok()
                    .and_then(|png| {
                        gtk::gdk::Texture::from_bytes(&glib::Bytes::from_owned(png)).ok()
                    });
                if let Some(texture) = texture {
                    if display && iter.offset() > 0 && !iter.starts_line() {
                        buffer.insert(&mut iter, "\n");
                    }
                    buffer.insert_paintable(&mut iter, &texture);
                    if display {
                        buffer.insert(&mut iter, "\n");
                    }
                } else {
                    buffer.insert(&mut iter, source);
                }
            }
        }
    }
}
