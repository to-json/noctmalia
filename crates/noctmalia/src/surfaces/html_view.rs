//! "Original formatting" — real CSS layout via `litehtml-sys`, painted onto an iced canvas.
//!
//! This is the opt-in half of `docs/html-mail-plan.md`. The default reader (`Reader` in
//! `surfaces::mail`) stays Markdown; this module only ever renders the one letter the user
//! explicitly asked to see "as sent." litehtml itself never touches the network (`shim.cpp`'s
//! `load_image` is a no-op) — remote images are Stream 3's fetch chokepoint, not built here, so
//! today this mode shows layout, text and declared colors/backgrounds, and nothing an `<img>` ever
//! pointed at.
//!
//! [`responsive`] is what makes this work without a hand-written `Widget` impl: its closure runs
//! at layout time with the real available width, which is also the one litehtml needs to lay a
//! block-flow document out — a canvas can't sensibly report its own height until that layout has
//! already happened once. Layout time is every frame, though, and a litehtml pass is two walks of
//! the document with a font shaper in the loop, so [`Cache`] remembers the last one: the same
//! document at the same width is the same layout, and only a resize or another letter costs one.

use iced::advanced::text::Paragraph as _;
use iced::widget::canvas::{self, Frame, Geometry};
use iced::widget::{canvas as canvas_widget, responsive};
use iced::{Color, Element, Length, Pixels, Point, Rectangle, Size};
use iced_graphics::text::Paragraph;
use litehtml_sys::{Kind, Primitive};
use noctalia_iced::theme;
use std::cell::RefCell;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::rc::Rc;

/// The last layout: which document, at what width, and what came out. Owned by whoever holds
/// the letter and handed to [`view`] on every frame; interior mutability because `view` runs on
/// `&self` and the layout happens inside it.
#[derive(Default)]
pub struct Cache {
    laid: RefCell<Option<Laid>>,
}

struct Laid {
    document: u64,
    width: i32,
    primitives: Rc<Vec<Primitive>>,
}

impl Cache {
    /// The primitives for `html` at `width`, laid out now if the last layout was of something
    /// else.
    fn layout(&self, html: &str, width: i32) -> Rc<Vec<Primitive>> {
        let document = {
            let mut hasher = DefaultHasher::new();
            html.hash(&mut hasher);
            hasher.finish()
        };
        if let Some(laid) = self.laid.borrow().as_ref()
            && laid.document == document
            && laid.width == width
        {
            return Rc::clone(&laid.primitives);
        }
        // litehtml rejecting the document outright (see shim.cpp's negative-count path) is
        // nothing to paint, and a zero-height canvas is the honest reflection of that.
        let primitives = Rc::new(
            litehtml_sys::render_with_measure(html, width, i32::MAX, &mut measure).map_or(Vec::new(), |r| r.primitives),
        );
        *self.laid.borrow_mut() = Some(Laid { document, width, primitives: Rc::clone(&primitives) });
        primitives
    }
}

/// Measures `text` set at `size_px` in the app's own UI typeface, through iced's real shaper
/// (`iced_graphics::text::Paragraph`, the same one that will later actually paint it) — not a
/// per-character guess. This is what makes litehtml's line breaks land close to where the text it
/// hands back will really wrap once painted; see `litehtml_sys::render_with_measure`.
///
/// Wrapped in its own `catch_unwind`, independent of `litehtml_sys`'s trampoline (which also
/// catches this, so the panic never unwinds into litehtml's C++ stack — see that crate for why
/// that specifically is undefined behaviour). What only this call site can do is clean up after
/// itself: `Paragraph::with_text` takes iced's *global*, process-wide font-system lock
/// (`iced::advanced::graphics::text::font_system()`, a `std::sync::RwLock` shared with every
/// ordinary `iced::widget::text` in the window) and panics via `.expect("Write font system")` if
/// it is already poisoned. A panic anywhere while that lock is held poisons it — whether or not
/// the panic is caught — so leaving it poisoned after this call turns one bad HTML message into
/// every other screen in the app panicking on its very next redraw, unprotected, for the rest of
/// the process. That cascade, not the original panic, is what "crashes a lot" was: clearing the
/// poison here is what stops it from outliving this one call.
fn measure(text: &str, size_px: i32) -> i32 {
    let measured = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let paragraph = Paragraph::with_text(iced::advanced::text::Text {
            content: text,
            bounds: Size::INFINITE,
            size: Pixels(size_px as f32),
            line_height: iced::advanced::text::LineHeight::default(),
            font: theme::font(),
            align_x: iced::advanced::text::Alignment::Default,
            align_y: iced::alignment::Vertical::Top,
            shaping: iced::advanced::text::Shaping::Advanced,
            wrapping: iced::advanced::text::Wrapping::None,
        });
        paragraph.min_width().ceil() as i32
    }));
    measured.unwrap_or_else(|_| {
        iced::advanced::graphics::text::font_system().clear_poison();
        text.chars().count() as i32 * (size_px * 3 / 5)
    })
}

/// Renders `html` — already through `mime::html::restrict`; this module trusts its caller for
/// that — as a scrollable-height canvas filling whatever width its parent gives it.
pub fn view<'a, Message: 'a>(html: &'a str, cache: &'a Cache) -> Element<'a, Message> {
    responsive(move |size| {
        let available = (size.width.max(1.0) as i32).max(1);
        let primitives = cache.layout(html, available);
        let bottom = primitives.iter().fold(0.0_f32, |max, p| max.max((p.y + p.h) as f32));
        let height = if primitives.is_empty() { 0.0 } else { bottom + f32::from(PADDING_BOTTOM) };
        canvas_widget::Canvas::new(HtmlView { primitives }).width(Length::Fill).height(Length::Fixed(height)).into()
    })
    .into()
}

const PADDING_BOTTOM: u16 = 24;

struct HtmlView {
    primitives: Rc<Vec<Primitive>>,
}

impl<Message> canvas::Program<Message> for HtmlView {
    type State = ();

    fn draw(
        &self,
        _state: &(),
        renderer: &iced::Renderer,
        _theme: &iced::Theme,
        bounds: Rectangle,
        _cursor: iced::advanced::mouse::Cursor,
    ) -> Vec<Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());
        for primitive in self.primitives.iter() {
            let [r, g, b, a] = primitive.color();
            let color = Color::from_rgba(r, g, b, a);
            let top_left = Point::new(primitive.x as f32, primitive.y as f32);
            let size = Size::new(primitive.w as f32, primitive.h as f32);
            match primitive.kind {
                Kind::Background | Kind::Border => frame.fill_rectangle(top_left, size, color),
                Kind::Text => frame.fill_text(canvas::Text {
                    content: primitive.text.clone(),
                    position: top_left,
                    color,
                    // litehtml gives us the text run's laid-out box height (line height, not glyph
                    // size); shrinking it a little keeps ascenders/descenders from clipping against
                    // neighbouring lines until real font metrics replace the shim's fixed estimate.
                    size: Pixels((primitive.h as f32 * 0.82).max(9.0)),
                    // Deliberately NOT clamped to `primitive.w`: that box was sized by shim.cpp's
                    // rough `text_width` estimate, not by asking iced how wide this run will really
                    // be, so it is reliably narrower than the true shaped text — clamping to it
                    // wraps mid-run and produces broken-up text, worse than the rare word that
                    // overflows the reading pane at the current heuristic. Real font metrics
                    // (shim.cpp text_width calling back into iced's own shaper) fixes both; see
                    // docs/html-mail-plan.md Stream 1.2.
                    ..canvas::Text::default()
                }),
            }
        }
        vec![frame.into_geometry()]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real Google security-alert email (captured live, see `docs/html-mail-plan.md`) once segfaulted
    /// litehtml on this exact content — not through anything wrong with the HTML itself, but
    /// because `measure`'s font-system lock got contended in the live app and the resulting panic
    /// unwound through litehtml's C++ stack (undefined behaviour, fixed in litehtml-sys's
    /// trampoline with `catch_unwind`). This fixture stays as a real-world smoke test: actual
    /// Gmail markup, actual `<img>` tags, actual nested tables, at a realistic reading-pane width.
    #[test]
    fn a_real_captured_gmail_message_renders_without_crashing() {
        let html = include_str!("fixtures/security-alert.html");
        let rendered = render_with_measure(html, 380, i32::MAX);
        assert!(!rendered.primitives.is_empty());
    }

    /// The regression test above measures against `theme::font()`'s untouched default
    /// (`Font::DEFAULT`, since nothing in a `cargo test` process calls `main`'s startup sequence),
    /// which is not what a real window measures against: `main.rs` calls
    /// `font::adopt_system_families()` and `theme::set_font(font::ui())` before the first frame,
    /// pointing `sans-serif` at whatever fontconfig resolves on the machine (here, real Noto Sans),
    /// not iced's built-in fallback. If the live crash is a real shaping bug rather than lock
    /// contention, it should need the real font's real glyph data to trigger — reproducing it here
    /// is cheaper than reproducing it in the windowed app.
    #[test]
    fn the_real_system_font_against_the_same_fixture_does_not_crash() {
        crate::font::adopt_system_families();
        theme::set_font(crate::font::ui());
        let html = include_str!("fixtures/security-alert.html");
        for width in [60, 120, 200, 260, 320, 380, 440, 520, 600, 760, 900] {
            let rendered = render_with_measure(html, width, i32::MAX);
            assert!(!rendered.primitives.is_empty(), "width {width}");
        }
    }

    /// The actual mechanism behind "we crash a lot": `catch_unwind` in `litehtml_sys`'s trampoline
    /// stops a panic from unwinding into litehtml's C++ stack, but it does not — cannot — undo the
    /// poisoning a `std::sync::RwLock` does automatically when a panic unwinds through a held
    /// guard. Left alone, one bad HTML message poisons iced's *global* font-system lock, and every
    /// ordinary `iced::widget::text` in the rest of the window — nothing to do with litehtml —
    /// inherits that poison and panics on its own next redraw, unprotected. This proves `measure`
    /// clears the poison it finds, so the rest of the app survives past the one bad call.
    #[test]
    fn measure_clears_a_poisoned_font_system_lock_instead_of_leaving_it_for_everyone_else() {
        let lock = iced::advanced::graphics::text::font_system();
        let _ = std::thread::spawn(|| {
            let _guard = iced::advanced::graphics::text::font_system().write().expect("not poisoned yet");
            panic!("simulated: some shaping bug panicking mid-layout while holding the write lock");
        })
        .join();
        assert!(lock.is_poisoned(), "the setup should have poisoned it, or this test proves nothing");

        let width = measure("hello", 14);
        assert!(width > 0, "still returns a usable fallback width rather than propagating the panic");
        assert!(!lock.is_poisoned(), "measure must clear the poison, not just survive its own call");

        // The real proof: an ordinary widget elsewhere in the app, with no idea litehtml exists,
        // must not inherit the poison and panic on the very next redraw.
        let paragraph = Paragraph::with_text(iced::advanced::text::Text {
            content: "ordinary UI text, nothing to do with HTML mail",
            bounds: Size::INFINITE,
            size: Pixels(14.0),
            line_height: iced::advanced::text::LineHeight::default(),
            font: theme::font(),
            align_x: iced::advanced::text::Alignment::Default,
            align_y: iced::alignment::Vertical::Top,
            shaping: iced::advanced::text::Shaping::Basic,
            wrapping: iced::advanced::text::Wrapping::None,
        });
        assert!(paragraph.min_width() > 0.0);
    }

    /// `measure()` alone stops at shaping — text width, never a rasterized glyph. The two tests
    /// above prove the same fixture, at the same widths, through the real font, never panics
    /// there — but the live crash was in `draw_background`, deep inside a *paint*, and nothing
    /// above ever paints. `iced_tiny_skia` (the software backend `run.sh` forces on this machine —
    /// see the memory note on hardware rendering) rasterizes each glyph with
    /// `swash::get_image_uncached` and then `tiny_skia::PixmapRef::from_bytes(buffer, w, h)
    /// .expect("Create glyph pixel map")`: a panic site if swash ever hands back a buffer whose
    /// length doesn't match `w * h * 4`, which only a real `draw()` call can reach. `iced_test`'s
    /// `Simulator` drives that same real renderer headlessly (already used for `tests/shots.rs`'s
    /// PNGs), so routing `view()` through it — real font, same fixture, same width sweep — is the
    /// cheapest way to find out whether painting, not measuring, is where this actually breaks.
    #[test]
    fn the_real_renderer_paints_the_same_fixture_without_crashing() {
        crate::font::adopt_system_families();
        theme::set_font(crate::font::ui());
        let html = include_str!("fixtures/security-alert.html");
        let settings = iced::Settings { default_font: crate::font::ui(), ..iced::Settings::default() };
        for width in [60, 120, 200, 260, 320, 380, 440, 520, 600, 760, 900] {
            let cache = Cache::default();
            let element = view::<()>(html, &cache);
            let mut simulator = iced_test::Simulator::with_size(
                settings.clone(),
                Size::new(width as f32, 4000.0),
                element,
            );
            simulator.snapshot(&iced::Theme::Dark).unwrap_or_else(|e| panic!("width {width}: {e:?}"));
        }
    }

    fn render_with_measure(html: &str, width: i32, height: i32) -> litehtml_sys::Rendered {
        litehtml_sys::render_with_measure(html, width, height, &mut measure)
            .expect("litehtml should accept this document")
    }
}
