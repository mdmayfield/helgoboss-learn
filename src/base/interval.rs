use std::ops::Sub;

/// An interval which has an inclusive min and inclusive max value.
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub struct Interval<T: PartialOrd + Copy + Sub> {
    min: T,
    max: T,
}

impl<T: PartialOrd + Copy + Sub> Interval<T> {
    /// Creates an interval. Panics if `min` is greater than `max`.
    pub fn new(min: T, max: T) -> Interval<T> {
        assert!(min <= max);
        Interval { min, max }
    }

    /// Checks if this interval contains the given value.
    pub fn contains(&self, value: T) -> bool {
        self.min <= value && value <= self.max
    }

    /// Returns the low bound of this interval.
    pub fn min(&self) -> T {
        self.min
    }

    /// Returns a new interval containing the given minimum.
    ///
    /// If the given minimum is greater than the current maximum, the maximum will be set to given
    /// minimum.
    pub fn with_min(&self, min: T) -> Interval<T> {
        Interval::new(min, if min <= self.max { self.max } else { min })
    }
    /// Returns a new interval containing the given maxium.
    ///
    /// If the given maximum is lower than the current minimum, the minimum will be set to the given
    /// maximum.
    pub fn with_max(&self, max: T) -> Interval<T> {
        Interval::new(if self.min <= max { self.min } else { max }, max)
    }

    /// Returns the high bound of this interval.
    pub fn max(&self) -> T {
        self.max
    }

    /// Returns the distance between the low and high bound of this interval.
    pub fn span(&self) -> T::Output {
        self.max - self.min
    }
}
