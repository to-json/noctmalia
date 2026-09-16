//! Raw FFI over `shim.cpp`'s `litehtml_render` — the whole of Stream 0/1 in
//! `docs/html-mail-plan.md`. This crate does layout only: hand it sanitized HTML, get back a flat
//! list of paint primitives (background rects, border edges, positioned text runs) plus their text.
//! No image loading, no network, no anchor navigation — those need the fetch chokepoint and the
//! `Reader::on_link_click`-shaped wiring that only make sense one layer up, in the crate that
//! actually has a UI and a network policy.

use std::ffi::{CStr, CString, c_char, c_void};
use std::os::raw::c_int;

#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Background = 0,
    Border = 1,
    Text = 2,
}

impl Kind {
    fn from_raw(raw: i32) -> Kind {
        match raw {
            0 => Kind::Background,
            1 => Kind::Border,
            2 => Kind::Text,
            other => panic!("litehtml-sys: shim.cpp emitted an unknown primitive kind {other}"),
        }
    }
}

/// One paint instruction, in the coordinate space litehtml laid the document out in (origin
/// top-left, `y` growing downward, same as iced). `rgba` is packed `0xRRGGBBAA`. `text` is only
/// meaningful for [`Kind::Text`].
#[derive(Debug, Clone)]
pub struct Primitive {
    pub kind: Kind,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    pub rgba: u32,
    pub text: String,
}

impl Primitive {
    pub fn color(&self) -> [f32; 4] {
        [
            ((self.rgba >> 24) & 0xff) as f32 / 255.0,
            ((self.rgba >> 16) & 0xff) as f32 / 255.0,
            ((self.rgba >> 8) & 0xff) as f32 / 255.0,
            (self.rgba & 0xff) as f32 / 255.0,
        ]
    }
}

/// The document's total laid-out size, and every primitive litehtml drew at `viewport_width`.
#[derive(Debug, Clone)]
pub struct Rendered {
    pub primitives: Vec<Primitive>,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct RawPrimitive {
    kind: c_int,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    rgba: u32,
    text_offset: i32,
    text_len: i32,
}

type MeasureFn = unsafe extern "C" fn(ctx: *mut c_void, text: *const c_char, size_px: i32) -> i32;

unsafe extern "C" {
    fn litehtml_render(
        html: *const c_char,
        viewport_w: i32,
        viewport_h: i32,
        out: *mut RawPrimitive,
        max_out: i32,
        text_out: *mut u8,
        max_text: i32,
        text_len_out: *mut i32,
        measure: Option<MeasureFn>,
        measure_ctx: *mut c_void,
    ) -> i32;
}

/// Lays out `html` at `viewport_w` × `viewport_h` using a fixed per-character width estimate for
/// text (no real font available here — see [`render_with_measure`] for the version that asks a
/// real text shaper). Good enough for a rough size, wrong for anything meant to be looked at.
pub fn render(html: &str, viewport_w: i32, viewport_h: i32) -> Option<Rendered> {
    render_with(html, viewport_w, viewport_h, None, std::ptr::null_mut())
}

/// Same as [`render`], but `measure(text, size_px)` — real text-width measurement from whatever
/// font will actually paint this, e.g. iced's own shaper — decides where litehtml wraps a line.
/// Without this, line breaks are computed against a guess and rarely land where the text that is
/// actually painted will really break, which is visible as text overflowing or breaking mid-word.
///
/// `measure` is called synchronously and only for the duration of this call — nothing about it is
/// retained afterward.
pub fn render_with_measure(
    html: &str,
    viewport_w: i32,
    viewport_h: i32,
    measure: &mut dyn FnMut(&str, i32) -> i32,
) -> Option<Rendered> {
    // A trait object behind one thin pointer, so the trampoline below has a single, stable shape
    // regardless of which closure the caller passed — no per-closure monomorphized C symbol needed.
    let mut measure: &mut dyn FnMut(&str, i32) -> i32 = measure;
    let ctx = std::ptr::addr_of_mut!(measure).cast::<c_void>();

    unsafe extern "C" fn trampoline(ctx: *mut c_void, text: *const c_char, size_px: i32) -> i32 {
        let measure = unsafe { &mut *ctx.cast::<&mut dyn FnMut(&str, i32) -> i32>() };
        let text = unsafe { CStr::from_ptr(text) }.to_str().unwrap_or("");
        // `measure` runs arbitrary caller code (real font shaping, in practice) from inside a C++
        // call stack (litehtml is mid-layout when it asks for this). A Rust panic unwinding through
        // foreign frames is undefined behaviour — not a clean abort, an honest-to-god memory
        // corruption that can crash *elsewhere* in litehtml, which is exactly what a poisoned
        // font-system lock did here once, three times, only in the live app (a lock the caller
        // holds is a caller concern; not letting it become a segfault is this trampoline's job).
        // On panic, fall back to the same fixed per-character estimate `render` uses without a
        // measurer at all — wrong width, never a crash.
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| measure(text, size_px)))
            .unwrap_or_else(|_| text.chars().count() as i32 * (size_px * 3 / 5))
    }

    render_with(html, viewport_w, viewport_h, Some(trampoline), ctx)
}

/// A NUL byte in `html` — which a C string cannot carry, and which hostile mail can — is dropped
/// rather than refused: the caller is a reader, not a validator.
///
/// # Panics
/// If the shim reports a primitive kind this crate doesn't know about (would mean `shim.cpp` and
/// `lib.rs` have drifted — a bug here, not a caller error).
fn render_with(
    html: &str,
    viewport_w: i32,
    viewport_h: i32,
    measure: Option<MeasureFn>,
    measure_ctx: *mut c_void,
) -> Option<Rendered> {
    let c_html = match CString::new(html) {
        Ok(c_html) => c_html,
        Err(_) => CString::new(html.replace('\0', "")).expect("every NUL was just removed"),
    };

    // First pass: ask for nothing, just learn the true counts, then allocate exactly once. A
    // render pass is re-run rather than cached because litehtml doesn't expose a "how many
    // primitives would this produce" query on its own — this is the shim's substitute. `measure` is
    // still exercised on this pass — it's what decides line breaks, so skipping it here would
    // count primitives for a layout different from the one the second pass actually reports.
    let mut text_len: i32 = 0;
    let probe = unsafe {
        litehtml_render(
            c_html.as_ptr(),
            viewport_w,
            viewport_h,
            std::ptr::null_mut(),
            0,
            std::ptr::null_mut(),
            0,
            &mut text_len,
            measure,
            measure_ctx,
        )
    };
    if probe < 0 {
        return None; // litehtml::document::createFromString itself failed
    }

    let count = probe as usize;
    let mut raw = vec![
        RawPrimitive { kind: 0, x: 0.0, y: 0.0, w: 0.0, h: 0.0, rgba: 0, text_offset: 0, text_len: 0 };
        count
    ];
    let mut text_buf = vec![0u8; text_len as usize];

    let written = unsafe {
        litehtml_render(
            c_html.as_ptr(),
            viewport_w,
            viewport_h,
            raw.as_mut_ptr(),
            count as i32,
            text_buf.as_mut_ptr(),
            text_len,
            &mut 0,
            measure,
            measure_ctx,
        )
    };
    debug_assert_eq!(written, probe, "litehtml_render produced a different primitive count on the real pass");

    let primitives = raw
        .into_iter()
        .map(|p| {
            let text = if p.text_len > 0 {
                let start = p.text_offset as usize;
                let end = start + p.text_len as usize;
                String::from_utf8_lossy(&text_buf[start..end]).into_owned()
            } else {
                String::new()
            };
            Primitive { kind: Kind::from_raw(p.kind), x: p.x, y: p.y, w: p.w, h: p.h, rgba: p.rgba, text }
        })
        .collect();

    Some(Rendered { primitives })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A panic inside `measure` would otherwise unwind through litehtml's C++ call stack —
    /// undefined behaviour, observed in the wild as litehtml segfaulting deep in its own layout
    /// code on a real Gmail message, nowhere near where the actual panic happened. The trampoline
    /// must catch it and carry on rather than let a caller's bug become a crash bug here.
    #[test]
    fn a_panicking_measure_callback_does_not_crash_the_process() {
        let html = "<html><body><p>one two three four five six seven eight</p></body></html>";
        let mut always_panics = |_: &str, _: i32| -> i32 { panic!("simulated: a poisoned lock, or anything else") };
        let rendered = render_with_measure(html, 300, 600, &mut always_panics);
        assert!(rendered.is_some_and(|r| !r.primitives.is_empty()), "should still render, via the fallback width");
    }

    /// Stream 0's exit criterion, made permanent: litehtml links, lays real HTML out, and hands
    /// back at least one paintable primitive. If this stops passing, the FFI boundary broke, not
    /// some downstream rendering choice.
    #[test]
    fn a_coloured_box_and_a_paragraph_produce_primitives() {
        let html = r#"
            <html><body>
                <div style="background-color: #ff0000; width: 100px; height: 50px;"></div>
                <p>hello world, this is a paragraph long enough to wrap at a modest viewport width</p>
            </body></html>
        "#;

        let rendered = render(html, 400, 600).expect("litehtml should accept this document");
        assert!(!rendered.primitives.is_empty(), "expected at least one primitive, got none");

        let has_background = rendered.primitives.iter().any(|p| p.kind == Kind::Background && p.rgba == 0xff0000ff);
        assert!(has_background, "expected the red div's background, primitives: {:?}", rendered.primitives);

        let text: String = rendered
            .primitives
            .iter()
            .filter(|p| p.kind == Kind::Text)
            .map(|p| p.text.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(text.contains("hello"), "expected paragraph text among primitives, got: {text:?}");
    }

    /// The callback bridge itself: a deliberately huge measured width should change the layout
    /// litehtml settles on, and litehtml should actually have called into Rust to get there.
    #[test]
    fn a_measure_callback_changes_the_layout() {
        let html = "<html><body><p>one two three four five six seven eight</p></body></html>";

        let narrow = render(html, 200, 600).unwrap();
        let narrow_positions: Vec<(f64, f64)> = narrow.primitives.iter().map(|p| (p.x, p.y)).collect();

        let mut calls = 0;
        let mut huge = |text: &str, _size: i32| {
            calls += 1;
            text.chars().count() as i32 * 1000
        };
        let wrapped = render_with_measure(html, 200, 600, &mut huge).unwrap();
        let wrapped_positions: Vec<(f64, f64)> = wrapped.primitives.iter().map(|p| (p.x, p.y)).collect();

        assert!(calls > 0, "shim.cpp never called the measure callback");
        assert_ne!(
            narrow_positions, wrapped_positions,
            "an enormous measured width should lay these words out differently than the built-in guess"
        );
    }

    #[test]
    fn a_nul_byte_in_the_document_is_dropped_rather_than_a_panic() {
        let rendered = render("<html><body><p>be\0fore</p></body></html>", 400, 600).expect("still a document");
        let text: String = rendered.primitives.iter().filter(|p| p.kind == Kind::Text).map(|p| p.text.as_str()).collect();
        assert!(text.contains("before"), "{text:?}");
    }

    #[test]
    fn empty_document_still_renders_without_crashing() {
        let rendered = render("<html><body></body></html>", 400, 600).expect("empty doc is still a valid doc");
        assert!(rendered.primitives.is_empty());
    }
}
