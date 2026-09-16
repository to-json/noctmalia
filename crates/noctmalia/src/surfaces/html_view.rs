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
fn measure(text: &str, size_px: i32) -> i32 {
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

    fn render_with_measure(html: &str, width: i32, height: i32) -> litehtml_sys::Rendered {
        litehtml_sys::render_with_measure(html, width, height, &mut measure)
            .expect("litehtml should accept this document")
    }
}
