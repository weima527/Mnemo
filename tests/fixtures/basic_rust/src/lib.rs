//! A tiny fixture crate used by Mnemo's invariant and (later) parser tests.

pub mod math;

/// A 2-D point.
pub struct Point {
    /// Horizontal coordinate.
    pub x: i64,
    /// Vertical coordinate.
    pub y: i64,
}

impl Point {
    /// Manhattan distance from the origin (cross-file call into `math::add`).
    pub fn manhattan(&self) -> i64 {
        math::add(self.x.abs(), self.y.abs())
    }
}
