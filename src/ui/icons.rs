//! Tiny Cairo vectors: consistent at every scale, independent of installed icon fonts.
use gtk::{cairo, prelude::*};
#[derive(Clone, Copy)]
pub(super) enum Icon {
    Play,
    Pause,
    Previous,
    Next,
    Heart,
    Search,
    Home,
    History,
    Queue,
    Cloud,
    Settings,
    Plus,
}
fn line(cr: &cairo::Context, points: &[(f64, f64)]) {
    if let Some(&(x, y)) = points.first() {
        cr.move_to(x, y);
        for &(x, y) in &points[1..] {
            cr.line_to(x, y);
        }
        let _ = cr.stroke();
    }
}
fn triangle(cr: &cairo::Context, points: &[(f64, f64)]) {
    cr.move_to(points[0].0, points[0].1);
    for &(x, y) in &points[1..] {
        cr.line_to(x, y);
    }
    cr.close_path();
    let _ = cr.fill();
}
fn paint(cr: &cairo::Context, icon: Icon) {
    cr.set_line_width(1.65);
    cr.set_line_cap(cairo::LineCap::Round);
    cr.set_line_join(cairo::LineJoin::Round);
    match icon {
        Icon::Play => triangle(cr, &[(8., 4.5), (20., 12.), (8., 19.5)]),
        Icon::Pause => {
            cr.rectangle(6.5, 5., 4., 14.);
            cr.rectangle(13.5, 5., 4., 14.);
            let _ = cr.fill();
        }
        Icon::Next | Icon::Previous => {
            let _ = cr.save();
            if matches!(icon, Icon::Previous) {
                cr.translate(24., 0.);
                cr.scale(-1., 1.);
            }
            triangle(cr, &[(4., 5.), (14., 12.), (4., 19.)]);
            cr.rectangle(17., 5., 2.4, 14.);
            let _ = cr.fill();
            let _ = cr.restore();
        }
        Icon::Heart => {
            cr.move_to(12., 20.);
            cr.curve_to(10., 18., 3., 13., 3., 8.);
            cr.curve_to(3., 2.5, 10., 2., 12., 7.);
            cr.curve_to(14., 2., 21., 2.5, 21., 8.);
            cr.curve_to(21., 13., 14., 18., 12., 20.);
            let _ = cr.stroke();
        }
        Icon::Search => {
            cr.arc(10., 10., 6.5, 0., std::f64::consts::TAU);
            let _ = cr.stroke();
            line(cr, &[(15., 15.), (21., 21.)]);
        }
        Icon::Home => {
            line(cr, &[(3., 10.), (12., 3.), (21., 10.)]);
            line(
                cr,
                &[
                    (5., 9.),
                    (5., 21.),
                    (10., 21.),
                    (10., 14.),
                    (14., 14.),
                    (14., 21.),
                    (19., 21.),
                    (19., 9.),
                ],
            );
        }
        Icon::History => {
            cr.arc(12., 12., 8.5, -2.5, 2.8);
            let _ = cr.stroke();
            line(cr, &[(3., 4.), (3., 9.), (8., 9.)]);
            line(cr, &[(12., 7.), (12., 12.), (16., 14.)]);
        }
        Icon::Queue => {
            for y in [5., 11., 17.] {
                line(cr, &[(4., y), (16., y)]);
            }
            triangle(cr, &[(18., 14.), (23., 18.), (18., 22.)]);
        }
        Icon::Cloud => {
            cr.move_to(6., 18.);
            cr.curve_to(0., 18., 0., 9., 6., 9.);
            cr.curve_to(8., 1., 18., 2., 19., 10.);
            cr.curve_to(25., 10., 25., 18., 19., 18.);
            cr.close_path();
            let _ = cr.stroke();
        }
        Icon::Settings => {
            cr.arc(12., 12., 3.5, 0., std::f64::consts::TAU);
            let _ = cr.stroke();
            cr.arc(12., 12., 7.5, 0., std::f64::consts::TAU);
            let _ = cr.stroke();
            for i in 0..8 {
                let a = i as f64 * std::f64::consts::TAU / 8.;
                line(
                    cr,
                    &[
                        (12. + 7.5 * a.cos(), 12. + 7.5 * a.sin()),
                        (12. + 10. * a.cos(), 12. + 10. * a.sin()),
                    ],
                );
            }
        }
        Icon::Plus => {
            line(cr, &[(12., 5.), (12., 19.)]);
            line(cr, &[(5., 12.), (19., 12.)]);
        }
    }
}
pub(super) fn widget(icon: Icon, size: i32, pink: bool) -> gtk::DrawingArea {
    let drawing = gtk::DrawingArea::new();
    drawing.set_content_width(size);
    drawing.set_content_height(size);
    drawing.set_halign(gtk::Align::Center);
    drawing.set_valign(gtk::Align::Center);
    drawing.set_draw_func(move |drawing, cr, w, h| {
        cr.translate((w - h.min(w)) as f64 / 2., (h - h.min(w)) as f64 / 2.);
        let scale = w.min(h) as f64 / 24.;
        cr.scale(scale, scale);
        if drawing
            .ancestor(gtk::ListBoxRow::static_type())
            .and_downcast::<gtk::ListBoxRow>()
            .is_some_and(|row| row.is_selected())
        {
            cr.set_source_rgb(0., 0.58, 0.79);
        } else if pink {
            cr.set_source_rgb(0.984, 0.447, 0.600);
        } else {
            cr.set_source_rgb(0.30, 0.30, 0.32);
        }
        paint(cr, icon);
    });
    drawing
}
pub(super) fn set_play(button: &gtk::Button, playing: bool) {
    let drawing = widget(if playing { Icon::Pause } else { Icon::Play }, 22, false);
    drawing.set_draw_func(move |_, cr, w, h| {
        cr.translate((w - 24) as f64 / 2., (h - 24) as f64 / 2.);
        cr.set_source_rgb(1., 1., 1.);
        paint(cr, if playing { Icon::Pause } else { Icon::Play });
    });
    button.set_child(Some(&drawing));
    button.set_tooltip_text(Some(if playing { "暂停" } else { "播放" }));
}
thread_local! {
    // Rasterized from the same SVG as the desktop and tray icons.
    static LOGO: gtk::gdk_pixbuf::Pixbuf = gtk::gdk_pixbuf::Pixbuf::from_read(
        std::io::Cursor::new(include_bytes!("../../assets/buisc-logo.png"))
    ).expect("embedded Buisc icon");
}

pub(super) fn artwork(size: i32) -> gtk::DrawingArea {
    let drawing = gtk::DrawingArea::new();
    drawing.set_content_width(size);
    drawing.set_content_height(size);
    drawing.set_valign(gtk::Align::Center);
    drawing.set_halign(gtk::Align::Center);
    drawing.set_draw_func(|_, cr, width, height| {
        let size = f64::from(width.min(height));
        LOGO.with(|pixbuf| {
            cr.translate(
                (f64::from(width) - size) / 2.,
                (f64::from(height) - size) / 2.,
            );
            cr.scale(
                size / f64::from(pixbuf.width()),
                size / f64::from(pixbuf.height()),
            );
            cr.set_source_pixbuf(pixbuf, 0., 0.);
            let _ = cr.paint();
        });
    });
    drawing
}
