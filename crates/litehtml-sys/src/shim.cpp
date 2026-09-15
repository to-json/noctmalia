// The FFI boundary between litehtml (a pure-virtual C++ callback interface,
// litehtml::document_container) and Rust. litehtml has no notion of a GPU or a widget toolkit — it
// asks its container to draw text/backgrounds/borders at specific positions, and the container
// decides what "drawing" means. Ours means "record a primitive," not "paint a pixel": each callback
// appends a `Primitive` to a per-render buffer, which `litehtml_render` copies out to the caller.
// The actual painting (into an iced `canvas`) happens on the Rust side — see crate docs.
//
// No image loading, no anchor navigation, no CSS import: those are Stream 1+ (see
// docs/html-mail-plan.md), gated behind the network chokepoint that doesn't exist yet. This shim's
// job is only to prove the FFI boundary and get real layout numbers out.

#include <litehtml/litehtml.h>
#include <litehtml/document_container.h>
#include <cstring>
#include <cstdint>
#include <vector>
#include <string>

extern "C" {

enum PrimitiveKind : int32_t {
    PRIMITIVE_BACKGROUND = 0,
    PRIMITIVE_BORDER = 1,
    PRIMITIVE_TEXT = 2,
};

struct Primitive {
    int32_t kind;
    double x, y, w, h;
    uint32_t rgba;
    // Only populated for PRIMITIVE_TEXT: an offset/length into the render's text blob, since a
    // fixed-size struct can't hold a variable-length string.
    int32_t text_offset;
    int32_t text_len;
};

} // extern "C"

namespace {

uint32_t pack_rgba(const litehtml::web_color& c) {
    return (uint32_t(c.red) << 24) | (uint32_t(c.green) << 16) | (uint32_t(c.blue) << 8) | uint32_t(c.alpha);
}

// Rust supplies real text measurement (iced's own font shaper) through this — see
// `litehtml_sys::render`. `ctx` is opaque to this file; Rust owns whatever it points to.
using MeasureFn = int32_t (*)(void* ctx, const char* text, int32_t size_px);

// One of these per litehtml_render call. Owns the text blob every PRIMITIVE_TEXT's
// text_offset/text_len points into, so the caller can copy it out after collecting primitives.
struct Sink {
    std::vector<Primitive> primitives;
    std::string text_blob;
    int viewport_w, viewport_h;
    MeasureFn measure;
    void* measure_ctx;

    int32_t stash_text(const char* text) {
        int32_t offset = static_cast<int32_t>(text_blob.size());
        text_blob.append(text);
        return offset;
    }
};

// A stub font: litehtml only ever asks us for the metrics and widths it needs to lay text out, and
// never inspects the handle itself, so any unique-enough value works. No real font is loaded on
// this side — `text_width` asks Rust instead (see `Sink::measure`); only line-height metrics
// (ascent/descent/height, below) are still an estimate.
struct StubFont {
    int size;
};

class Container : public litehtml::document_container {
public:
    explicit Container(Sink& sink) : sink_(sink) {}

    litehtml::uint_ptr create_font(const char* /*faceName*/, int size, int /*weight*/, litehtml::font_style /*italic*/,
                                    unsigned int /*decoration*/, litehtml::font_metrics* fm) override {
        if (fm) {
            fm->height = size + size / 4;
            fm->ascent = size;
            fm->descent = size / 4;
            fm->x_height = size / 2;
            fm->draw_spaces = true;
        }
        return reinterpret_cast<litehtml::uint_ptr>(new StubFont{size});
    }

    void delete_font(litehtml::uint_ptr hFont) override { delete reinterpret_cast<StubFont*>(hFont); }

    int text_width(const char* text, litehtml::uint_ptr hFont) override {
        auto* font = reinterpret_cast<StubFont*>(hFont);
        if (sink_.measure) return sink_.measure(sink_.measure_ctx, text, font->size);
        // No measurement callback given (e.g. a caller that only wants rough layout) — fall back to
        // a fixed advance-per-character estimate, wrong for any real typeface but self-consistent.
        return static_cast<int>(std::strlen(text)) * (font->size * 3 / 5);
    }

    void draw_text(litehtml::uint_ptr hdc, const char* text, litehtml::uint_ptr /*hFont*/,
                   litehtml::web_color color, const litehtml::position& pos) override {
        (void)hdc;
        Primitive p{};
        p.kind = PRIMITIVE_TEXT;
        p.x = pos.x;
        p.y = pos.y;
        p.w = pos.width;
        p.h = pos.height;
        p.rgba = pack_rgba(color);
        p.text_offset = sink_.stash_text(text);
        p.text_len = static_cast<int32_t>(std::strlen(text));
        sink_.primitives.push_back(p);
    }

    int pt_to_px(int pt) const override { return pt * 4 / 3; }
    int get_default_font_size() const override { return 16; }
    const char* get_default_font_name() const override { return "sans-serif"; }

    void draw_list_marker(litehtml::uint_ptr /*hdc*/, const litehtml::list_marker& /*marker*/) override {
        // Bullets/numbers: fine to skip for the spike, real support is Stream 1.
    }

    void load_image(const char* /*src*/, const char* /*baseurl*/, bool /*redraw_on_ready*/) override {
        // Deliberately a no-op: no network access exists anywhere in this shim. Stream 3 adds the
        // one fetch chokepoint this would call into — nothing here should grow its own.
    }

    void get_image_size(const char* /*src*/, const char* /*baseurl*/, litehtml::size& sz) override {
        sz.width = 0;
        sz.height = 0;
    }

    void draw_background(litehtml::uint_ptr hdc, const std::vector<litehtml::background_paint>& bg) override {
        (void)hdc;
        for (const auto& layer : bg) {
            if (layer.color.alpha == 0) continue;
            Primitive p{};
            p.kind = PRIMITIVE_BACKGROUND;
            p.x = layer.border_box.x;
            p.y = layer.border_box.y;
            p.w = layer.border_box.width;
            p.h = layer.border_box.height;
            p.rgba = pack_rgba(layer.color);
            sink_.primitives.push_back(p);
        }
    }

    void draw_borders(litehtml::uint_ptr hdc, const litehtml::borders& borders, const litehtml::position& draw_pos,
                       bool /*root*/) override {
        (void)hdc;
        auto emit = [&](const litehtml::border& b, double x, double y, double w, double h) {
            if (b.style == litehtml::border_style_none || b.width == 0) return;
            Primitive p{};
            p.kind = PRIMITIVE_BORDER;
            p.x = x;
            p.y = y;
            p.w = w;
            p.h = h;
            p.rgba = pack_rgba(b.color);
            sink_.primitives.push_back(p);
        };
        emit(borders.top, draw_pos.x, draw_pos.y, draw_pos.width, borders.top.width);
        emit(borders.bottom, draw_pos.x, draw_pos.y + draw_pos.height - borders.bottom.width, draw_pos.width,
             borders.bottom.width);
        emit(borders.left, draw_pos.x, draw_pos.y, borders.left.width, draw_pos.height);
        emit(borders.right, draw_pos.x + draw_pos.width - borders.right.width, draw_pos.y, borders.right.width,
             draw_pos.height);
    }

    void set_caption(const char* /*caption*/) override {}
    void set_base_url(const char* /*base_url*/) override {}
    void link(const std::shared_ptr<litehtml::document>& /*doc*/, const litehtml::element::ptr& /*el*/) override {}
    void on_anchor_click(const char* /*url*/, const litehtml::element::ptr& /*el*/) override {
        // Stream 2.3 wires this to Reader::on_link_click. Not yet.
    }
    void set_cursor(const char* /*cursor*/) override {}
    void transform_text(litehtml::string& /*text*/, litehtml::text_transform /*tt*/) override {}
    void import_css(litehtml::string& /*text*/, const litehtml::string& /*url*/, litehtml::string& /*baseurl*/) override {
        // No @import: nothing here should reach the network. Deliberate, matches load_image.
    }
    void set_clip(const litehtml::position& /*pos*/, const litehtml::border_radiuses& /*bdr_radius*/) override {}
    void del_clip() override {}

    void get_client_rect(litehtml::position& client) const override {
        client.x = 0;
        client.y = 0;
        client.width = sink_.viewport_w;
        client.height = sink_.viewport_h;
    }

    litehtml::element::ptr create_element(const char* /*tag_name*/, const litehtml::string_map& /*attributes*/,
                                           const std::shared_ptr<litehtml::document>& /*doc*/) override {
        return nullptr; // let litehtml fall back to its own built-in element for every tag
    }

    void get_media_features(litehtml::media_features& media) const override {
        media.type = litehtml::media_type_screen;
        media.width = sink_.viewport_w;
        media.height = sink_.viewport_h;
        media.device_width = sink_.viewport_w;
        media.device_height = sink_.viewport_h;
        media.color = 8;
        media.color_index = 256;
        media.monochrome = 0;
        media.resolution = 96;
    }

    void get_language(litehtml::string& language, litehtml::string& culture) const override {
        language = "en";
        culture = "US";
    }

private:
    Sink& sink_;
};

} // namespace

extern "C" {

// Lays out `html` at `viewport_w` and writes every background/border/text primitive litehtml
// produces into `out` (capacity `max_out`), plus the concatenated text of every PRIMITIVE_TEXT into
// `text_out` (capacity `max_text`). Returns the true primitive count (may exceed `max_out`, in
// which case the caller should re-run with a bigger buffer) and, via `text_len_out`, the true text
// blob length. Returns a negative count on failure (litehtml returned no document).
//
// `measure`/`measure_ctx`: real text-width measurement, supplied by the caller (see
// `litehtml_sys::render`) so line-wrapping matches the font that will actually paint this text
// rather than a fixed per-character guess. Pass `measure = nullptr` to fall back to that guess.
int32_t litehtml_render(const char* html, int32_t viewport_w, int32_t viewport_h, Primitive* out, int32_t max_out,
                         char* text_out, int32_t max_text, int32_t* text_len_out, MeasureFn measure,
                         void* measure_ctx) {
    Sink sink;
    sink.viewport_w = viewport_w;
    sink.viewport_h = viewport_h;
    sink.measure = measure;
    sink.measure_ctx = measure_ctx;
    Container container(sink);

    auto doc = litehtml::document::createFromString(html, &container);
    if (!doc) return -1;

    doc->render(viewport_w);
    doc->draw(0, 0, 0, nullptr);

    int32_t count = static_cast<int32_t>(sink.primitives.size());
    int32_t to_copy = std::min(count, max_out);
    if (to_copy > 0) std::memcpy(out, sink.primitives.data(), sizeof(Primitive) * to_copy);

    if (text_len_out) *text_len_out = static_cast<int32_t>(sink.text_blob.size());
    int32_t text_to_copy = std::min(static_cast<int32_t>(sink.text_blob.size()), max_text);
    if (text_to_copy > 0) std::memcpy(text_out, sink.text_blob.data(), text_to_copy);

    return count;
}

} // extern "C"
