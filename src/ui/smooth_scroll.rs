use gtk::{gdk, glib, prelude::*};
use std::{cell::Cell, rc::Rc};

const WHEEL_STEP: f64 = 64.;
const DURATION_US: f64 = 150_000.;

#[derive(Clone, Copy)]
struct Motion {
    from: f64,
    target: f64,
    last: f64,
    started: i64,
}

pub(super) fn attach(scroll: &gtk::ScrolledWindow) {
    let motion = Rc::new(Cell::<Option<Motion>>::new(None));
    let ticking = Rc::new(Cell::new(false));
    let controller = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::BOTH_AXES);
    controller.set_propagation_phase(gtk::PropagationPhase::Capture);
    let weak = scroll.downgrade();
    controller.connect_scroll(move |controller, dx, dy| {
        let Some(scroll) = weak.upgrade() else {
            return glib::Propagation::Proceed;
        };
        if controller.unit() != gdk::ScrollUnit::Wheel
            || dx != 0.
            || dy == 0.
            || !scroll.settings().is_gtk_enable_animations()
        {
            motion.set(None);
            return glib::Propagation::Proceed;
        }

        let adjustment = scroll.vadjustment();
        let value = adjustment.value();
        let lower = adjustment.lower();
        let upper = (adjustment.upper() - adjustment.page_size()).max(lower);
        let previous = motion
            .get()
            .filter(|m| (m.last - value).abs() < 0.5 && (m.target - value).signum() == dy.signum());
        let target = (previous.map_or(value, |m| m.target) + dy * WHEEL_STEP).clamp(lower, upper);
        if (target - value).abs() < 0.5 {
            motion.set(None);
            // Let GTK handle edge gestures, including pagination on a short list.
            return glib::Propagation::Proceed;
        }

        motion.set(Some(Motion {
            from: value,
            target,
            last: value,
            started: glib::monotonic_time(),
        }));
        if !ticking.replace(true) {
            let motion = motion.clone();
            let ticking = ticking.clone();
            scroll.add_tick_callback(move |scroll, _| {
                let Some(mut step) = motion.get() else {
                    ticking.set(false);
                    return glib::ControlFlow::Break;
                };
                let adjustment = scroll.vadjustment();
                if !scroll.is_mapped() || (adjustment.value() - step.last).abs() >= 0.5 {
                    // A scrollbar drag, keyboard scroll, or a new list took over.
                    motion.set(None);
                    ticking.set(false);
                    return glib::ControlFlow::Break;
                }

                let fraction =
                    ((glib::monotonic_time() - step.started) as f64 / DURATION_US).clamp(0., 1.);
                let eased = 1. - (1. - fraction).powi(3);
                adjustment.set_value(step.from + (step.target - step.from) * eased);
                step.last = adjustment.value();
                if fraction >= 1. {
                    motion.set(None);
                    ticking.set(false);
                    glib::ControlFlow::Break
                } else {
                    motion.set(Some(step));
                    glib::ControlFlow::Continue
                }
            });
        }
        glib::Propagation::Stop
    });
    scroll.add_controller(controller);
}
