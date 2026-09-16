//! A list's selection bar: which row it is travelling to, whether it is on screen at all, and the
//! scroll that keeps the row it marks in view.
//!
//! Mail and people each had one of these, written twice with different names. This is the one,
//! and a third list gets it by holding one rather than by copying either.

use iced::advanced::widget::Id;
use iced::widget::operation;
use iced::widget::scrollable::{AbsoluteOffset, Viewport};
use iced::{Animation, Task};
use noctalia_iced::{list, motion};
use std::time::Instant;

pub struct Cursor {
    /// Which row the bar is travelling to, counted from the top.
    row: Animation<f32>,
    /// Whether the bar is on screen at all. It fades rather than blinking out.
    shown: Animation<bool>,
}

impl Cursor {
    /// A bar with nothing to mark yet: parked on row 0, hidden.
    pub fn hidden() -> Cursor {
        Cursor { row: motion::spring_animation(0.0), shown: motion::glide_animation(false) }
    }

    /// A bar already on `row` and showing, for a rail that always has a selection.
    pub fn seated(row: f32) -> Cursor {
        Cursor { row: motion::spring_animation(row), shown: motion::glide_animation(true) }
    }

    /// Sends the bar to `row`, or fades it out when there is no row to go to. It travels, so a
    /// user who picks a third row before the bar reached the second sees it change course.
    pub fn aim(&mut self, row: Option<f32>, now: Instant) {
        if let Some(row) = row {
            self.row.go_mut(row, now);
        }
        self.shown.go_mut(row.is_some(), now);
    }

    /// Puts the bar on `row` without travelling: the rows underneath it just changed, and the
    /// distance from where it was means nothing.
    pub fn seat(&mut self, row: Option<f32>, now: Instant) {
        self.row = motion::spring_animation(row.unwrap_or(0.0));
        self.shown.go_mut(row.is_some(), now);
    }

    /// Where the bar is this frame, in rows.
    pub fn at(&self, now: Instant) -> f32 {
        self.row.interpolate_with(|row| row, now)
    }

    /// How much of the bar is on screen, from 0 to 1.
    pub fn presence(&self, now: Instant) -> f32 {
        self.shown.interpolate(0.0, 1.0, now)
    }

    pub fn animating(&self, now: Instant) -> bool {
        self.row.is_animating(now) || self.shown.is_animating(now)
    }
}

/// The scroll that brings `row` fully into view, and none at all if it already is. Before the list
/// has ever reported a viewport there is nothing to decide with, and short lists never need it.
pub fn follow<M: Send + 'static>(
    row: Option<usize>,
    pitch: f32,
    row_height: f32,
    view: Option<Viewport>,
    list: Id,
) -> Task<M> {
    let (Some(row), Some(view)) = (row, view) else { return Task::none() };
    match list::reveal(row, pitch, row_height, view.absolute_offset().y, view.bounds().height) {
        Some(y) => operation::scroll_to(list, AbsoluteOffset { x: 0.0, y }),
        None => Task::none(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn a_hidden_cursor_shows_once_aimed_and_hides_again_when_there_is_nothing_to_mark() {
        let now = Instant::now();
        let mut cursor = Cursor::hidden();
        assert_eq!(cursor.presence(now), 0.0);
        cursor.aim(Some(3.0), now);
        let later = now + Duration::from_secs(5);
        assert_eq!(cursor.presence(later), 1.0);
        assert!((cursor.at(later) - 3.0).abs() < f32::EPSILON);
        cursor.aim(None, later);
        assert_eq!(cursor.presence(later + Duration::from_secs(5)), 0.0);
    }

    #[test]
    fn seating_arrives_at_once_where_aiming_travels() {
        let now = Instant::now();
        let mut travelling = Cursor::seated(0.0);
        travelling.aim(Some(10.0), now);
        assert!(travelling.at(now) < 10.0, "aiming starts a journey");
        let mut seated = Cursor::seated(0.0);
        seated.seat(Some(10.0), now);
        assert!((seated.at(now) - 10.0).abs() < f32::EPSILON, "seating is already there");
    }
}
