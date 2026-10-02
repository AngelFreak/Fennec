//! A box that wraps its children onto new lines, like text (tag chips).

use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

/// A box that lays its children out left to right at their natural width,
/// wrapping onto a new line when the next one does not fit (tag chips).
pub(crate) fn wrap_box() -> gtk::Box {
    let b = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    b.set_layout_manager(Some(glib::Object::new::<WrapLayout>()));
    b
}

const GAP: i32 = 6;

glib::wrapper! {
    pub struct WrapLayout(ObjectSubclass<imp::WrapLayout>) @extends gtk::LayoutManager;
}

fn children(w: &gtk::Widget) -> Vec<gtk::Widget> {
    let mut out = Vec::new();
    let mut c = w.first_child();
    while let Some(x) = c {
        c = x.next_sibling();
        if x.is_visible() {
            out.push(x);
        }
    }
    out
}

/// Where each child goes for a line `width` wide, and the total height.
#[allow(clippy::type_complexity)]
fn place(w: &gtk::Widget, width: i32) -> (Vec<(gtk::Widget, gtk::Allocation)>, i32) {
    let (mut x, mut y, mut line_h) = (0, 0, 0);
    let mut out = Vec::new();
    for c in children(w) {
        let cw = c.measure(gtk::Orientation::Horizontal, -1).1.min(width.max(1));
        let ch = c.measure(gtk::Orientation::Vertical, cw).1;
        if x > 0 && x + cw > width {
            x = 0;
            y += line_h + GAP;
            line_h = 0;
        }
        out.push((c, gtk::Allocation::new(x, y, cw, ch)));
        x += cw + GAP;
        line_h = line_h.max(ch);
    }
    (out, y + line_h)
}

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct WrapLayout;

    #[glib::object_subclass]
    impl ObjectSubclass for WrapLayout {
        const NAME: &'static str = "FennecWrapLayout";
        type Type = super::WrapLayout;
        type ParentType = gtk::LayoutManager;
    }

    impl ObjectImpl for WrapLayout {}

    impl LayoutManagerImpl for WrapLayout {
        fn request_mode(&self, _: &gtk::Widget) -> gtk::SizeRequestMode {
            gtk::SizeRequestMode::HeightForWidth
        }

        fn measure(
            &self,
            w: &gtk::Widget,
            orientation: gtk::Orientation,
            for_size: i32,
        ) -> (i32, i32, i32, i32) {
            if orientation == gtk::Orientation::Horizontal {
                let kids = children(w);
                let sizes: Vec<(i32, i32)> = kids
                    .iter()
                    .map(|c| {
                        let (min, nat, _, _) = c.measure(gtk::Orientation::Horizontal, -1);
                        (min, nat)
                    })
                    .collect();
                // Asks only for the widest child: the parent decides the line
                // length, and the chips wrap to it (a sum here would widen
                // the sidebar to fit every tag on one line).
                let min = sizes.iter().map(|s| s.0).max().unwrap_or(0);
                let nat = sizes.iter().map(|s| s.1).max().unwrap_or(0);
                (min, nat.max(min), -1, -1)
            } else {
                let width = if for_size < 0 { i32::MAX / 4 } else { for_size };
                let h = place(w, width).1;
                (h, h, -1, -1)
            }
        }

        fn allocate(&self, w: &gtk::Widget, width: i32, _: i32, _: i32) {
            for (c, a) in place(w, width).0 {
                c.size_allocate(&a, -1);
            }
        }
    }
}
