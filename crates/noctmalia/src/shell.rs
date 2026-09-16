//! What every surface shares: the bridge, the window's width, and the one place a message to the
//! user goes.
//!
//! Only one client may hold the bridge socket, so mail and contacts cannot be two programs — they
//! are two surfaces of one, and this is the part of it that neither of them owns. A surface is
//! handed a `&mut Shell` on every update and a `&Shell` on every view; it reads the width to decide
//! its layout, borrows the bridge to make a call, and calls [`Shell::announce`] or [`Shell::fail`]
//! rather than growing a banner of its own.

use crate::error::Error;
use crate::ui;
use iced::widget::{button, container, row, space, text};
use iced::{Alignment, Animation, Border, Element, Length, Padding};
use noctalia_iced::motion;
use noctalia_iced::theme;
use noctalia_iced::widgets;
use noctmalia_bridge::Bridge;
use std::time::Instant;

struct Notice {
    text: String,
    bad: bool,
}

pub struct Shell {
    bridge: Bridge,
    connected: bool,
    /// The window's width, which is what decides whether a surface's panes all fit.
    width: f32,
    notice: Option<Notice>,
    open: Animation<bool>,
}

impl Shell {
    pub fn new(bridge: Bridge, width: f32) -> Shell {
        Shell { bridge, connected: false, width, notice: None, open: motion::settle_animation(false) }
    }

    /// A handle to the bridge. Cheap to clone; every clone talks to the same socket.
    pub fn bridge(&self) -> Bridge {
        self.bridge.clone()
    }

    pub fn socket(&self) -> String {
        self.bridge.path().display().to_string()
    }

    pub fn connected(&self) -> bool {
        self.connected
    }

    pub fn set_connected(&mut self, connected: bool) {
        self.connected = connected;
    }

    pub fn width(&self) -> f32 {
        self.width
    }

    pub fn set_width(&mut self, width: f32) {
        self.width = width;
    }

    /// Shows the banner. The text stays put once set: [`hush`](Shell::hush) only closes it, so the
    /// words are still there to draw while it slides shut.
    pub fn announce(&mut self, text: impl Into<String>, now: Instant) {
        self.notice = Some(Notice { text: text.into(), bad: false });
        self.open.go_mut(true, now);
    }

    pub fn fail(&mut self, text: impl Into<String>, now: Instant) {
        self.notice = Some(Notice { text: text.into(), bad: true });
        self.open.go_mut(true, now);
    }

    /// A call over the bridge failed. Thunderbird not being there is not news — the window is
    /// already showing the waiting page, and the resync its next hello triggers redoes the call —
    /// so that case says nothing; everything else is a failure worth a banner.
    pub fn report(&mut self, error: &Error, now: Instant) {
        if error.is_disconnected() {
            return;
        }
        self.fail(error.to_string(), now);
    }

    /// [`report`](Shell::report), with what was being attempted in front of the reason.
    pub fn report_in(&mut self, what: &str, error: &Error, now: Instant) {
        if error.is_disconnected() {
            return;
        }
        self.fail(format!("{what}: {error}"), now);
    }

    /// What the banner is saying, whether or not it is still open.
    pub fn saying(&self) -> Option<&str> {
        self.notice.as_ref().map(|notice| notice.text.as_str())
    }

    pub fn hush(&mut self, now: Instant) {
        self.open.go_mut(false, now);
    }

    pub fn animating(&self, now: Instant) -> bool {
        self.open.is_animating(now)
    }

    /// The banner, which is nothing at all when there is nothing to say.
    pub fn notice<'a, M: Clone + 'a>(&'a self, now: Instant, dismiss: M) -> Element<'a, M> {
        let open = self.open.interpolate(0.0, 1.0, now);
        let Some(notice) = self.notice.as_ref().filter(|_| open > 0.001) else {
            return space().height(0).into();
        };
        let color = if notice.bad { theme::palette().error } else { theme::palette().tertiary };
        let glyph = if notice.bad { ui::icon::ALERT } else { theme::icon::CHECK };
        container(
            row![
                widgets::icon(glyph, theme::FONT_BODY).color(color),
                // The banner is one control-height row and clips to it, so the message stays on
                // one line rather than wrapping into a half-drawn second.
                text(notice.text.clone()).size(theme::FONT_CAPTION).wrapping(text::Wrapping::None),
                space().width(Length::Fill),
                button(widgets::icon(ui::icon::X, theme::FONT_CAPTION).color(theme::palette().on_surface_variant))
                    .style(theme::bare_button)
                    .padding(theme::SPACE_XS)
                    .on_press(dismiss),
            ]
            .spacing(theme::SPACE_SM)
            .align_y(Alignment::Center),
        )
        .width(Length::Fill)
        // Opening on height, clipped, so the row slides out from under the edge. `open` overshoots
        // a little on the way in, which is the whole point of SETTLE.
        //
        // The banner clips while it slides, so its full height has to be a number rather than
        // whatever the content measures. Taking the design language's control height makes it a
        // token instead of a guess, and leaves room to spare for the dismiss button inside it.
        .height(theme::CONTROL_HEIGHT * open)
        .clip(true)
        .padding(Padding::from([theme::SPACE_XS, theme::SPACE_MD]))
        .style(move |_| container::Style {
            background: Some(theme::palette().surface_variant.into()),
            border: Border { color, width: theme::BORDER, ..Border::default() },
            ..container::Style::default()
        })
        .into()
    }
}
