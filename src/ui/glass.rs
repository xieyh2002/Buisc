use gtk::{gdk, glib, graphene, gsk, prelude::*, subclass::prelude::*};

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct GlassPane;

    #[glib::object_subclass]
    impl ObjectSubclass for GlassPane {
        const NAME: &'static str = "BuiscGlassPane";
        type Type = super::GlassPane;
        type ParentType = gtk::Widget;

        fn class_init(class: &mut Self::Class) {
            class.set_layout_manager_type::<gtk::BinLayout>();
        }
    }

    impl ObjectImpl for GlassPane {
        fn dispose(&self) {
            while let Some(child) = self.obj().first_child() {
                child.unparent();
            }
        }
    }

    impl WidgetImpl for GlassPane {
        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let pane = self.obj();
            let (Some(page), Some(dock)) = (pane.first_child(), pane.last_child()) else {
                return;
            };

            // The visible page and blur share the same render node in this frame.
            let page_snapshot = gtk::Snapshot::new();
            pane.snapshot_child(&page, &page_snapshot);
            if let Some(page_node) = page_snapshot.to_node() {
                snapshot.append_node(&page_node);
                if dock.is_mapped()
                    && let Some(bounds) = dock.compute_bounds(&*pane)
                    && bounds.width() > 0.
                    && bounds.height() > 0.
                {
                    let sample = graphene::Rect::new(
                        bounds.x() - 24.,
                        bounds.y() - 24.,
                        bounds.width() + 48.,
                        bounds.height() + 48.,
                    );
                    snapshot.push_rounded_clip(&gsk::RoundedRect::from_rect(bounds, 22.));
                    snapshot.push_blur(12.);
                    snapshot.push_clip(&sample);
                    snapshot.append_color(&gdk::RGBA::WHITE, &sample);
                    snapshot.append_node(&page_node);
                    snapshot.pop();
                    snapshot.pop();
                    snapshot.pop();
                }
            }
            pane.snapshot_child(&dock, snapshot);
        }
    }
}

glib::wrapper! {
    pub struct GlassPane(ObjectSubclass<imp::GlassPane>)
        @extends gtk::Widget,
        @implements gtk::Buildable, gtk::ConstraintTarget;
}

impl GlassPane {
    pub(super) fn new(page: &impl IsA<gtk::Widget>, dock: &impl IsA<gtk::Widget>) -> Self {
        let pane: Self = glib::Object::new();
        pane.set_hexpand(true);
        pane.set_vexpand(true);
        page.set_parent(&pane);
        dock.set_parent(&pane);
        pane
    }
}
