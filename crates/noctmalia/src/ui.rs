//! The pieces every surface is built out of.
//!
//! These were the rolodex's private helpers until there was a second surface to draw. Nothing here
//! knows what it is showing — each is generic over the application's message type — which is the
//! point: a row in the mail index and a row in the contact list are the same row, and a change to
//! how selection looks should land in both without being made twice.
//!
//! The motion convention this file assumes is worth stating once, because it explains the shape of
//! nearly every signature here. iced has no opacity and no transform that survives clipping, so
//! nothing fades or moves by being drawn differently: it fades by blending *towards the surface
//! colour* and moves by changing *padding*. So the arguments are fractions — `fill`, `arrived`,
//! `shown` — and the widget resolves them into colours and lengths that the renderer clips and
//! hit-tests exactly as it does a still frame.

use iced::widget::{button, container, pin, row, space, text, tooltip};
use iced::{Alignment, Border, Color, Element, Length, Padding, Theme, border};
use noctalia_iced::motion;
use noctalia_iced::theme;
use noctalia_iced::widgets;

pub mod cursor;

/// Tabler codepoints from the bundled icon font.
pub mod icon {
    pub const ADDRESS_BOOK: char = '\u{f021}';
    pub const ALERT: char = '\u{ea05}';
    pub const ALERT_TRIANGLE: char = '\u{ea06}';
    pub const ARCHIVE: char = '\u{ea0b}';
    pub const ARROW_BACK_UP: char = '\u{eb77}';
    pub const AT: char = '\u{ea2b}';
    pub const BUILDING: char = '\u{ea4f}';
    pub const CALENDAR: char = '\u{ea53}';
    pub const DOWNLOAD: char = '\u{ea96}';
    pub const CHECK: char = '\u{ea5e}';
    pub const CHEVRON_DOWN: char = '\u{ea5f}';
    pub const CHEVRON_RIGHT: char = '\u{ea61}';
    pub const CIRCLE_CHECK: char = '\u{ea67}';
    pub const CLOUD: char = '\u{ea76}';
    pub const CODE: char = '\u{ea77}';
    pub const CORNER_UP_LEFT: char = '\u{ea82}';
    pub const CORNER_UP_RIGHT: char = '\u{ea83}';
    pub const DOTS: char = '\u{ea95}';
    pub const EXTERNAL_LINK: char = '\u{ea99}';
    pub const EYE: char = '\u{ea9a}';
    pub const FILE: char = '\u{eaa4}';
    pub const FILTER: char = '\u{eaa5}';
    pub const FLAG: char = '\u{eaa6}';
    pub const FLAG_FILLED: char = '\u{f67a}';
    pub const FOLDER: char = '\u{eaad}';
    pub const INBOX: char = '\u{eac4}';
    pub const INFO: char = '\u{eac5}';
    pub const LINK: char = '\u{eade}';
    pub const LIST: char = '\u{eb6b}';
    pub const LOCK: char = '\u{eae2}';
    pub const MAIL: char = '\u{eae5}';
    pub const MAIL_OPENED: char = '\u{eae4}';
    pub const MAP_PIN: char = '\u{eae8}';
    pub const NOTE: char = '\u{eb6d}';
    pub const PAPERCLIP: char = '\u{eb02}';
    pub const PENCIL: char = '\u{eb04}';
    pub const PHONE: char = '\u{eb09}';
    pub const PHOTO: char = '\u{eb0a}';
    pub const PLUG: char = '\u{ea9e}';
    pub const PLUS: char = '\u{eb0b}';
    pub const POINT: char = '\u{f698}';
    pub const REFRESH: char = '\u{eb13}';
    pub const SEARCH: char = '\u{eb1c}';
    pub const SEND: char = '\u{eb1e}';
    pub const SHIELD: char = '\u{eb24}';
    pub const SHIELD_CHECK: char = '\u{eb22}';
    pub const SHIELD_X: char = '\u{eb23}';
    pub const TAG: char = '\u{10096}';
    pub const TRASH: char = '\u{eb41}';
    pub const USER: char = '\u{eb4d}';
    pub const WORLD: char = '\u{eb54}';
    pub const WRITING: char = '\u{ef08}';
    pub const X: char = '\u{eb55}';
}

/// The hairline between rows in a list, and so the difference between a row and a row pitch.
pub const ROW_GAP: f32 = 1.0;
/// The slot a row's icon or initial sits in, so two of them line up with each other.
pub const MARK: f32 = 16.0;
pub const AVATAR: f32 = 36.0;
pub const AVATAR_LARGE: f32 = 64.0;

/// Secondary text: section labels, counts, hints.
pub fn caption<'a, M: 'a>(label: impl text::IntoFragment<'a>) -> Element<'a, M> {
    text(label).size(theme::FONT_CAPTION).color(theme::palette().on_surface_variant).into()
}

pub fn hairline_y<'a, M: 'a>() -> Element<'a, M> {
    container(space().width(theme::BORDER).height(Length::Fill)).style(theme::rule).into()
}

pub fn hairline_x<'a, M: 'a>() -> Element<'a, M> {
    container(space().width(Length::Fill).height(theme::BORDER)).style(theme::rule).into()
}

/// The gap a row leaves for the selection bar to be drawn in.
pub fn bar_gutter<'a, M: 'a>() -> Element<'a, M> {
    space().width(theme::BORDER_EMPHASIZED).into()
}

/// How much of a row's fill the selection bar is currently giving it: 1 when the bar is on the
/// row, 0 once it is a whole row away. Two adjacent rows therefore cross-fade as the bar travels
/// between them, and a longer jump lights the rows it passes over for a frame or two.
pub fn nearness(at: f32, index: usize) -> f32 {
    (1.0 - (at - index as f32).abs()).clamp(0.0, 1.0)
}

/// List rows mark selection with a leading bar and a quiet fill. `ButtonVariant`'s own hover is a
/// full mint block and `Selected` a full accent block — at list length that is a wall of colour.
///
/// `fill` is how selected the row is, which is a fraction rather than a flag while the bar is on
/// its way.
///
/// Hover and selection share the one fill, because the palette has one quiet surface and not two:
/// `surface_variant` is it, and a weaker wash of it would be a colour Noctalia never named. What
/// says a row is *selected* rather than merely under the pointer is the accent — the bar down its
/// left and the accent in its avatar — which is the louder signal either way.
pub fn row_style(fill: f32) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| {
        let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
        // The only fraction left is time: the fill is that role arriving, not a shade of it.
        let strength = if hovered { 1.0 } else { fill };
        button::Style {
            background: (strength > 0.001).then(|| theme::alpha(theme::palette().surface_variant, strength).into()),
            text_color: theme::palette().on_surface,
            border: border::rounded(theme::RADIUS_MD),
            ..button::Style::default()
        }
    }
}

/// The accent bar down the left of the selected row, pinned over the rows so it can be anywhere
/// between them. It lives inside the scrollable with the rows, so it scrolls with them.
///
/// `y` is its top in pixels, `total` the height of the whole column of rows — the bar layer has to
/// be as tall as the rows layer or the stack would size itself to whichever is bigger and leave the
/// scrollable measuring the wrong content.
///
/// It can lie over the rows without stealing their clicks because `stack` only hides the cursor
/// from the layers beneath one that reports a `mouse_interaction`, and this is a `space` in a
/// container: neither implements one, so the rows go on receiving presses through it.
pub fn selection_bar<'a, M: 'a>(y: f32, row_height: f32, shown: f32, total: f32) -> Element<'a, M> {
    let height = (row_height - theme::SPACE_XS * 2.0).max(1.0);
    let mark = container(space().width(theme::BORDER_EMPHASIZED).height(height)).style(move |_| container::Style {
        background: Some(theme::alpha(theme::palette().primary, shown.clamp(0.0, 1.0)).into()),
        border: border::rounded(theme::BORDER_EMPHASIZED),
        ..container::Style::default()
    });
    pin(mark).x(theme::SPACE_SM).y(y + theme::SPACE_XS).width(Length::Fill).height(total.max(0.0)).into()
}

/// Initials in a circle. Only the selected row gets the accent; a column of accent-filled circles
/// is the single loudest thing a list can do.
///
/// `accent` is a fraction, so a row picking up the selection crossfades into the accent instead of
/// switching. `scale` grows the disc inside a box that keeps its size, so an avatar arriving does
/// not shove the name beside it sideways. `fade` blends the whole thing towards the surface, which
/// is this toolkit's stand-in for an opacity it does not have.
pub fn avatar<'a, M: 'a>(initials: &str, size: f32, accent: f32, scale: f32, fade: f32) -> Element<'a, M> {
    let palette = theme::palette();
    let accent = accent.clamp(0.0, 1.0);
    let blend = |quiet, loud| motion::mix(palette.surface, motion::mix(quiet, loud, accent), fade);
    let background = blend(palette.surface_variant, palette.primary);
    let foreground = blend(palette.on_surface_variant, palette.on_primary);
    // The outline is what the accent fill replaces. Rather than fading it to nothing, it becomes
    // the fill it sits on — same role, so it disappears without ever being a colour of its own.
    let outline = blend(palette.outline, palette.primary);

    let disc = size * scale;
    let circle = container(text(initials.to_string()).size(disc * 0.34).font(theme::semibold()).color(foreground))
        .width(disc)
        .height(disc)
        .center_x(disc)
        .center_y(disc)
        .style(move |_| container::Style {
            background: Some(background.into()),
            border: Border { color: outline, width: theme::BORDER, ..border::rounded(disc / 2.0) },
            ..container::Style::default()
        });
    container(circle).width(size).height(size).center_x(size).center_y(size).into()
}

/// The first character of a name, which is what identifies a row once a rail has collapsed.
pub fn initial(label: &str) -> String {
    label.chars().find(|c| c.is_alphanumeric()).map_or_else(|| "•".to_string(), |c| c.to_uppercase().to_string())
}

/// A button with a glyph before its label.
pub fn glyph_button<'a, M: Clone + 'a>(
    glyph: char,
    label: &'a str,
    height: f32,
    style: impl Fn(&Theme, button::Status) -> button::Style + 'a,
    message: M,
) -> Element<'a, M> {
    // `button` does not centre its content: `layout::padded` puts the child at the padding offset
    // and stops. A shrink-height row in a fixed-height button therefore sits against the top, which
    // is what `widgets::action` avoids by filling the height and centring inside it. Same here.
    let content = row![widgets::icon(glyph, theme::FONT_BODY), text(label).size(theme::FONT_BODY)]
        .spacing(theme::SPACE_XS)
        .height(Length::Fill)
        .align_y(Alignment::Center);
    button(content).height(height).padding(Padding::from([0.0, theme::SPACE_MD])).style(style).on_press(message).into()
}

/// Quiet until the pointer is on it, and then unmistakably the destructive one.
pub fn danger_button<'a, M: Clone + 'a>(label: &'a str, message: M) -> Element<'a, M> {
    button(text(label).size(theme::FONT_BODY))
        .height(theme::CONTROL_HEIGHT)
        .padding(Padding::from([0.0, theme::SPACE_MD]))
        // At rest it is the same quiet surface every other button sits on, saying what it is in
        // `error` text; under the pointer it becomes the role, and the text its pair.
        .style(|_, status| {
            let hot = matches!(status, button::Status::Hovered | button::Status::Pressed);
            let palette = theme::palette();
            button::Style {
                background: Some(if hot { palette.error } else { palette.surface_variant }.into()),
                text_color: if hot { palette.on_error } else { palette.error },
                border: border::rounded(theme::RADIUS_MD),
                ..button::Style::default()
            }
        })
        .on_press(message)
        .into()
}

/// An outline with nothing in it, which warms to the accent under the pointer.
///
/// It stops short of the full accent fill the real actions take — adding a row is not the same
/// order of thing as saving — but the edge and the text both move, so there is no doubt it is live.
pub fn ghost_button(_: &Theme, status: button::Status) -> button::Style {
    let palette = theme::palette();
    let hot = matches!(status, button::Status::Hovered | button::Status::Pressed);
    let edge = if hot { palette.primary } else { palette.outline };
    button::Style {
        background: hot.then(|| palette.surface_variant.into()),
        text_color: if hot { palette.primary } else { palette.on_surface_variant },
        border: border::rounded(theme::RADIUS_MD).color(edge).width(theme::BORDER),
        ..button::Style::default()
    }
}

/// A small icon button with no chrome until the pointer is on it: a toolbar's worth of verbs.
pub fn icon_button<'a, M: Clone + 'a>(glyph: char, hint: &'a str, active: bool, message: M) -> Element<'a, M> {
    let control =
        button(container(widgets::icon(glyph, theme::FONT_BODY)).center_x(Length::Fill).center_y(Length::Fill))
            .width(theme::CONTROL_HEIGHT_SM)
            .height(theme::CONTROL_HEIGHT_SM)
            .padding(0)
            .style(move |_, status| {
                let palette = theme::palette();
                let hot = matches!(status, button::Status::Hovered | button::Status::Pressed);
                let (background, text_color) = match (active, hot) {
                    (_, true) => theme::hover_pair(if active { palette.primary } else { palette.surface }),
                    (true, false) => (palette.primary, palette.on_primary),
                    (false, false) => (Color::TRANSPARENT, palette.on_surface_variant),
                };
                button::Style {
                    background: Some(background.into()),
                    text_color,
                    border: border::rounded(theme::RADIUS_MD),
                    ..button::Style::default()
                }
            })
            .on_press(message);
    tip(control.into(), hint)
}

/// Anything, with a word about it under the pointer.
pub fn tip<'a, M: 'a>(control: Element<'a, M>, hint: &'a str) -> Element<'a, M> {
    if hint.is_empty() {
        return control;
    }
    let label = container(text(hint).size(theme::FONT_CAPTION))
        .padding(Padding::from([theme::SPACE_XS, theme::SPACE_SM]))
        .style(theme::track);
    tooltip(control, label, tooltip::Position::Bottom).gap(theme::SPACE_XS).into()
}

/// A word on a quiet chip: a tag, a count, a flavour.
pub fn chip<'a, M: 'a>(label: impl text::IntoFragment<'a>, colour: Color) -> Element<'a, M> {
    container(text(label).size(theme::FONT_MINI).color(colour))
        .padding(Padding::from([1.0, theme::SPACE_XS]))
        .style(move |_| container::Style {
            border: border::rounded(theme::RADIUS_SM).color(colour).width(theme::BORDER),
            ..container::Style::default()
        })
        .into()
}

/// The page shown until Thunderbird is up: what is happening, in a sentence, and a bar while
/// something measurable is. Nothing about sockets is on it.
pub fn starting<'a, M: 'a>(glyph: char, title: &'a str, detail: String, progress: Option<f32>) -> Element<'a, M> {
    let mut lines = iced::widget::column![
        widgets::icon(glyph, 32.0).color(theme::palette().outline),
        text(title).size(theme::FONT_TITLE).font(theme::semibold()),
        text(detail)
            .size(theme::FONT_CAPTION)
            .color(theme::palette().on_surface_variant)
            .wrapping(text::Wrapping::WordOrGlyph),
    ]
    .spacing(theme::SPACE_SM)
    .align_x(Alignment::Center);
    if let Some(fraction) = progress {
        lines = lines.push(iced::widget::progress_bar(0.0..=1.0, fraction).length(240.0).girth(6.0));
    }
    container(lines).center_x(Length::Fill).center_y(Length::Fill).into()
}

/// An empty pane saying what would be in it.
pub fn placeholder<'a, M: 'a>(glyph: char, title: &'a str, hint: &'a str) -> Element<'a, M> {
    container(
        iced::widget::column![
            // Decoration at the weight of a rule, so it takes the role rules are drawn in.
            widgets::icon(glyph, 32.0).color(theme::palette().outline),
            text(title).size(theme::FONT_BODY).color(theme::palette().on_surface_variant),
            // One role for secondary text; the size is what separates the hint from the line above.
            text(hint).size(theme::FONT_MINI).color(theme::palette().on_surface_variant),
        ]
        .spacing(theme::SPACE_XS)
        .align_x(Alignment::Center),
    )
    .center_x(Length::Fill)
    .center_y(Length::Fill)
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_row_is_lit_by_how_near_the_bar_is_and_by_nothing_else() {
        assert_eq!(nearness(2.0, 2), 1.0);
        assert_eq!(nearness(2.0, 3), 0.0);
        // Mid-travel the two rows it is between share the fill.
        assert_eq!(nearness(2.5, 2), 0.5);
        assert_eq!(nearness(2.5, 3), 0.5);
        assert_eq!(nearness(2.0, 40), 0.0, "a row far away is not lit at all");
    }

    #[test]
    fn a_row_is_marked_by_the_first_letter_of_its_name() {
        assert_eq!(initial("Personal Address Book"), "P");
        assert_eq!(initial("work"), "W");
        assert_eq!(initial("  Collected Addresses"), "C");
        assert_eq!(initial("2nd account"), "2");
        assert_eq!(initial("École"), "É");
        // Nothing to take a letter from is still a row, and still needs a mark.
        assert_eq!(initial("— —"), "•");
        assert_eq!(initial(""), "•");
    }
}
