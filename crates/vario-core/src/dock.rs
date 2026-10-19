//! The magnetism between the gauge and the main window, as plain geometry in physical
//! pixels: where one window goes beside the other, and when a released window snaps.

/// A window's outer rectangle on the desktop.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl Rect {
    fn right(&self) -> i32 {
        self.x + self.width
    }

    fn bottom(&self) -> i32 {
        self.y + self.height
    }

    fn center_x(&self) -> i32 {
        self.x + self.width / 2
    }

    fn overlaps_vertically(&self, other: &Rect) -> bool {
        self.y < other.bottom() && other.y < self.bottom()
    }

    pub fn contains(&self, x: i32, y: i32) -> bool {
        (self.x..self.right()).contains(&x) && (self.y..self.bottom()).contains(&y)
    }
}

/// Which side of the gauge the main window sits on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}

/// Where the main window goes to sit on `side` of the gauge, tops aligned.
pub fn main_beside(gauge: Rect, main_width: i32, side: Side) -> (i32, i32) {
    match side {
        Side::Left => (gauge.x - main_width, gauge.y),
        Side::Right => (gauge.right(), gauge.y),
    }
}

/// Where the gauge goes so that the main window sits on `side` of it, tops aligned.
pub fn gauge_beside(main: Rect, gauge_width: i32, side: Side) -> (i32, i32) {
    match side {
        Side::Left => (main.right(), main.y),
        Side::Right => (main.x - gauge_width, main.y),
    }
}

/// The side with room for the main window: the left when the gauge stands on the
/// right half of its screen, else the right.
pub fn roomy_side(gauge: Rect, screen: Option<Rect>) -> Side {
    match screen {
        Some(screen) if gauge.center_x() > screen.center_x() => Side::Left,
        Some(_) => Side::Right,
        None => Side::Left,
    }
}

/// The side the main window snaps to when one of the two windows is released with
/// facing edges within `reach` of each other, the windows overlapping vertically.
pub fn snap(main: Rect, gauge: Rect, reach: i32) -> Option<Side> {
    if !main.overlaps_vertically(&gauge) {
        return None;
    }
    if (main.right() - gauge.x).abs() <= reach {
        Some(Side::Left)
    } else if (main.x - gauge.right()).abs() <= reach {
        Some(Side::Right)
    } else {
        None
    }
}

/// The gauge's first place: the top right corner of the screen, inside a margin.
pub fn corner(screen: Rect, gauge_width: i32, margin: i32) -> (i32, i32) {
    (screen.right() - gauge_width - margin, screen.y + margin)
}

#[cfg(test)]
mod tests {
    use super::*;

    const GAUGE: Rect = Rect {
        x: 1000,
        y: 100,
        width: 156,
        height: 494,
    };

    fn main_at(x: i32, y: i32) -> Rect {
        Rect {
            x,
            y,
            width: 640,
            height: 494,
        }
    }

    #[test]
    fn beside_and_back() {
        assert_eq!(main_beside(GAUGE, 640, Side::Left), (360, 100));
        assert_eq!(main_beside(GAUGE, 640, Side::Right), (1156, 100));
        assert_eq!(
            gauge_beside(main_at(360, 100), 156, Side::Left),
            (1000, 100)
        );
        assert_eq!(
            gauge_beside(main_at(1156, 100), 156, Side::Right),
            (1000, 100)
        );
    }

    #[test]
    fn snaps_only_near_and_facing() {
        assert_eq!(snap(main_at(380, 130), GAUGE, 24), Some(Side::Left));
        assert_eq!(snap(main_at(1140, 60), GAUGE, 24), Some(Side::Right));
        assert_eq!(snap(main_at(300, 100), GAUGE, 24), None, "too far");
        assert_eq!(snap(main_at(360, 700), GAUGE, 24), None, "below the gauge");
    }

    #[test]
    fn the_main_window_opens_where_there_is_room() {
        let screen = Rect {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
        };
        assert_eq!(roomy_side(GAUGE, Some(screen)), Side::Left);
        let left = Rect { x: 300, ..GAUGE };
        assert_eq!(roomy_side(left, Some(screen)), Side::Right);
        assert_eq!(corner(screen, 156, 24), (1740, 24));
        assert!(screen.contains(1740, 24) && !screen.contains(1920, 24));
    }
}
