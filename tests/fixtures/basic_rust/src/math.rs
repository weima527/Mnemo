//! Small math helpers used by the fixture crate.

/// Add two integers.
pub fn add(a: i64, b: i64) -> i64 {
    a + b
}

/// Multiply two integers.
pub fn mul(a: i64, b: i64) -> i64 {
    a * b
}

/// Square an integer (same-file call into `mul`).
pub fn square(n: i64) -> i64 {
    mul(n, n)
}
