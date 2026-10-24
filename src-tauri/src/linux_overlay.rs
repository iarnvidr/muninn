//! Linux development build only: Tauri packs child web views into the window's
//! vertical GtkBox, one under the other, while macOS (and Windows) let them overlap
//! at arbitrary bounds. To exercise the real layout under xvfb, the chrome web
//! view is wrapped in a GtkOverlay and each tab's web view becomes an overlay child
//! positioned with margins. Nothing here ships in the macOS app.

use gtk::prelude::*;
use std::cell::RefCell;
use tauri::Webview;

thread_local! {
    static OVERLAY: RefCell<Option<gtk::Overlay>> = const { RefCell::new(None) };
}

/// Wraps the chrome web view in a GtkOverlay (once). Must run on the GTK thread.
pub fn wrap_chrome(chrome: &Webview) {
    let _ = chrome.with_webview(|w| {
        let view = w.inner();
        let widget: &gtk::Widget = view.upcast_ref();
        OVERLAY.with(|cell| {
            if cell.borrow().is_some() {
                return;
            }
            let Some(parent) = widget.parent().and_then(|p| p.downcast::<gtk::Box>().ok()) else {
                return;
            };
            let overlay = gtk::Overlay::new();
            parent.remove(widget);
            overlay.add(widget);
            parent.pack_start(&overlay, true, true, 0);
            overlay.show_all();
            *cell.borrow_mut() = Some(overlay);
        });
    });
}

/// Moves a freshly created tab view into the overlay.
pub fn adopt(tab: &Webview) {
    let _ = tab.with_webview(|w| {
        let view = w.inner();
        let widget: &gtk::Widget = view.upcast_ref();
        OVERLAY.with(|cell| {
            let overlay = cell.borrow();
            let Some(overlay) = overlay.as_ref() else {
                return;
            };
            if let Some(parent) = widget.parent().and_then(|p| p.downcast::<gtk::Box>().ok()) {
                parent.remove(widget);
            }
            overlay.add_overlay(widget);
            widget.set_halign(gtk::Align::Start);
            widget.set_valign(gtk::Align::Start);
            widget.show_all();
        });
    });
}

pub fn place(tab: &Webview, x: f64, y: f64, w: f64, h: f64, visible: bool) {
    let (x, y, w, h) = (x as i32, y as i32, w as i32, h as i32);
    let _ = tab.with_webview(move |wv| {
        let view = wv.inner();
        let widget: &gtk::Widget = view.upcast_ref();
        widget.set_margin_start(x.max(0));
        widget.set_margin_top(y.max(0));
        widget.set_size_request(w.max(1), h.max(1));
        if visible {
            widget.show();
        } else {
            widget.hide();
        }
    });
}
