use crate::{
    create_discrete_increment_interval, create_unit_value_interval, full_unit_interval,
    mode::feedback_util, negative_if, ControlType, ControlValue, DiscreteIncrement, DiscreteValue,
    Interval, MinIsMaxBehavior, OutOfRangeBehavior, PressDurationProcessor, Target, Transformation,
    UnitIncrement, UnitValue,
};
use derive_more::Display;
use enum_iterator::IntoEnumIterator;
use num_enum::{IntoPrimitive, TryFromPrimitive};
#[cfg(feature = "serde_repr")]
use serde_repr::{Deserialize_repr, Serialize_repr};

/// Settings for processing all kinds of control values.
///
/// ## How relative control values are processed (or button taps interpreted as increments).
///
/// Here's an overview in which cases step counts are used and in which step sizes.
/// This is the same, no matter if the source emits relative increments or absolute values
/// ("relative one-direction mode").
///   
/// - Target wants relative increments: __Step counts__
///     - Example: Action with invocation type "Relative"
///     - Displayed as: "{count} x"
/// - Target wants absolute values
///     - Target is continuous, optionally roundable: __Step sizes__
///         - Example: Track volume
///         - Displayed as: "{size} {unit}"
///     - Target is discrete: __Step counts__
///         - Example: FX preset, some FX params
///         - Displayed as: "{count} x" or "{count}" (former if source emits increments) TODO I
///           think now we have only the "x" variant
#[derive(Clone, Debug)]
pub struct Mode<T: Transformation> {
    pub absolute_mode: AbsoluteMode,
    pub source_value_interval: Interval<UnitValue>,
    pub target_value_interval: Interval<UnitValue>,
    /// Negative increments represent fractions (throttling), e.g. -2 fires an increment every
    /// 2nd time only.
    pub step_count_interval: Interval<DiscreteIncrement>,
    pub step_size_interval: Interval<UnitValue>,
    pub jump_interval: Interval<UnitValue>,
    // TODO-low Not cool to make this public. Maybe derive a builder for this beast.
    pub press_duration_processor: PressDurationProcessor,
    pub approach_target_value: bool,
    pub reverse: bool,
    pub rotate: bool,
    pub round_target_value: bool,
    pub out_of_range_behavior: OutOfRangeBehavior,
    pub control_transformation: Option<T>,
    pub feedback_transformation: Option<T>,
    /// Counter for implementing throttling.
    ///
    /// Throttling is implemented by spitting out control values only every nth time. The counter
    /// can take positive or negative values in order to detect direction changes. This is positive
    /// when the last change was a positive increment and negative when the last change was a
    /// negative increment.
    pub increment_counter: i32,
}

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, IntoEnumIterator, TryFromPrimitive, IntoPrimitive, Display,
)]
#[cfg_attr(feature = "serde_repr", derive(Serialize_repr, Deserialize_repr))]
#[repr(usize)]
pub enum AbsoluteMode {
    #[display(fmt = "Normal")]
    Normal = 0,
    #[display(fmt = "Incremental buttons")]
    IncrementalButtons = 1,
    #[display(fmt = "Toggle buttons")]
    ToggleButtons = 2,
}

impl Default for AbsoluteMode {
    fn default() -> Self {
        AbsoluteMode::Normal
    }
}

impl<T: Transformation> Default for Mode<T> {
    fn default() -> Self {
        Mode {
            absolute_mode: AbsoluteMode::Normal,
            source_value_interval: full_unit_interval(),
            target_value_interval: full_unit_interval(),
            // 0.01 has been chosen as default minimum step size because it corresponds to 1%.
            // 0.01 has also been chosen as default maximum step size because most users probably
            // want to start easy, that is without using the "press harder = more increments"
            // respectively "dial harder = more increments" features. Activating them right from
            // the start by choosing a higher step size maximum could lead to surprising results
            // such as ugly parameters jumps, especially if the source is not suited for that.
            step_size_interval: create_unit_value_interval(0.01, 0.01),
            // Same reasoning like with `step_size_interval`
            step_count_interval: create_discrete_increment_interval(1, 1),
            jump_interval: full_unit_interval(),
            press_duration_processor: Default::default(),
            approach_target_value: false,
            reverse: false,
            round_target_value: false,
            out_of_range_behavior: OutOfRangeBehavior::MinOrMax,
            control_transformation: None,
            feedback_transformation: None,
            rotate: false,
            increment_counter: 0,
        }
    }
}

impl<T: Transformation> Mode<T> {
    /// Processes the given control value and maybe returns an appropriate target control value.
    pub fn control(
        &mut self,
        control_value: ControlValue,
        target: &impl Target,
    ) -> Option<ControlValue> {
        match control_value {
            ControlValue::Relative(i) => self.control_relative(i, target),
            ControlValue::Absolute(v) => {
                use AbsoluteMode::*;
                match self.absolute_mode {
                    Normal => self
                        .control_absolute_normal(v, target)
                        .map(ControlValue::Absolute),
                    IncrementalButtons => self.control_absolute_incremental_buttons(v, target),
                    ToggleButtons => self
                        .control_absolute_toggle_buttons(v, target)
                        .map(ControlValue::Absolute),
                }
            }
        }
    }

    /// Takes a target value, interprets and transforms it conforming to mode rules and
    /// maybe returns an appropriate source value that should be sent to the source.
    pub fn feedback(&self, target_value: UnitValue) -> Option<UnitValue> {
        feedback_util::feedback(
            target_value,
            self.reverse,
            &self.feedback_transformation,
            &self.source_value_interval,
            &self.target_value_interval,
            self.out_of_range_behavior,
        )
    }

    /// Processes the given control value in absolute mode and maybe returns an appropriate target
    /// value.
    fn control_absolute_normal(
        &mut self,
        control_value: UnitValue,
        target: &impl Target,
    ) -> Option<UnitValue> {
        let control_value = self.press_duration_processor.process(control_value)?;
        let (source_bound_value, min_is_max_behavior) =
            if control_value.is_within_interval(&self.source_value_interval) {
                // Control value is within source value interval
                (control_value, MinIsMaxBehavior::PreferOne)
            } else {
                // Control value is outside source value interval
                use OutOfRangeBehavior::*;
                match self.out_of_range_behavior {
                    MinOrMax => {
                        if control_value < self.source_value_interval.min_val() {
                            (
                                self.source_value_interval.min_val(),
                                MinIsMaxBehavior::PreferZero,
                            )
                        } else {
                            (
                                self.source_value_interval.max_val(),
                                MinIsMaxBehavior::PreferOne,
                            )
                        }
                    }
                    Min => (
                        self.source_value_interval.min_val(),
                        MinIsMaxBehavior::PreferZero,
                    ),
                    Ignore => return None,
                }
            };
        let current_target_value = target.current_value();
        // Control value is within source value interval
        let control_type = target.control_type();
        let pepped_up_control_value = self.pep_up_control_value(
            source_bound_value,
            control_type,
            current_target_value,
            min_is_max_behavior,
        );
        self.hitting_target_considering_max_jump(
            pepped_up_control_value,
            current_target_value,
            control_type,
        )
    }

    /// Relative one-direction mode (convert absolute button presses to relative increments)
    fn control_absolute_incremental_buttons(
        &mut self,
        control_value: UnitValue,
        target: &impl Target,
    ) -> Option<ControlValue> {
        let control_value = self.press_duration_processor.process(control_value)?;
        if control_value.is_zero() || !control_value.is_within_interval(&self.source_value_interval)
        {
            return None;
        }
        use ControlType::*;
        match target.control_type() {
            AbsoluteContinuous
            | AbsoluteContinuousRoundable { .. }
            // TODO-low I think trigger and switch targets don't make sense at all here because
            //  instead of +/- n they need just "trigger!" or "on/off!". 
            | AbsoluteTrigger
            | AbsoluteSwitch => {
                // Continuous target
                //
                // Settings:
                // - Source value interval (for setting the input interval of relevant source
                //   values)
                // - Minimum target step size (enables accurate minimum increment, atomic)
                // - Maximum target step size (enables accurate maximum increment, clamped)
                // - Target value interval (absolute, important for rotation only, clamped)
                let step_size_value = control_value
                    .map_to_unit_interval_from(
                        &self.source_value_interval,
                        MinIsMaxBehavior::PreferOne,
                    )
                    .map_from_unit_interval_to(&self.step_size_interval);
                let step_size_increment =
                    step_size_value.to_increment(negative_if(self.reverse))?;
                self.hit_target_absolutely_with_unit_increment(
                    step_size_increment,
                    self.step_size_interval.min_val(),
                    target.current_value()?,
                )
            }
            AbsoluteDiscrete { atomic_step_size } => {
                // Discrete target
                //
                // Settings:
                // - Source value interval (for setting the input interval of relevant source
                //   values)
                // - Minimum target step count (enables accurate normal/minimum increment, atomic)
                // - Target value interval (absolute, important for rotation only, clamped)
                // - Maximum target step count (enables accurate maximum increment, clamped)
                let discrete_increment = self.convert_to_discrete_increment(control_value)?;
                self.hit_discrete_target_absolutely(discrete_increment, atomic_step_size, || {
                    target.current_value()
                })
            }
            Relative
            // This is cool! With this, we can make controllers without encoders simulate them
            // by assigning one - button and one + button to the same virtual multi target.
            // Of course, all we can deliver is increments/decrements since virtual targets 
            // don't provide a current target value. But we also don't need it because all we
            // want to do is simulate an encoder.
            | VirtualMulti => {
                // Target wants increments so we just generate them e.g. depending on how hard the
                // button has been pressed
                //
                // - Source value interval (for setting the input interval of relevant source
                //   values)
                // - Minimum target step count (enables accurate normal/minimum increment, atomic)
                // - Maximum target step count (enables accurate maximum increment, mapped)
                let discrete_increment = self.convert_to_discrete_increment(control_value)?;
                Some(ControlValue::Relative(discrete_increment))
            }
            VirtualButton => {
                // This doesn't make sense at all. Buttons just need to be triggered, not fed with
                // +/- n.
                None
            },
        }
    }

    fn control_absolute_toggle_buttons(
        &mut self,
        control_value: UnitValue,
        target: &impl Target,
    ) -> Option<UnitValue> {
        let control_value = self.press_duration_processor.process(control_value)?;
        if control_value.is_zero() {
            return None;
        }
        let center_target_value = self.target_value_interval.center();
        // Nothing we can do if we can't get the current target value. This shouldn't happen
        // usually because virtual targets are not supposed to be used with toggle mode.
        let current_target_value = target.current_value()?;
        let desired_target_value = if current_target_value > center_target_value {
            self.target_value_interval.min_val()
        } else {
            self.target_value_interval.max_val()
        };
        if desired_target_value == current_target_value {
            return None;
        }
        Some(desired_target_value)
    }

    // Classic relative mode: We are getting encoder increments from the source.
    // We don't need source min/max config in this case. At least I can't think of a use case
    // where one would like to totally ignore especially slow or especially fast encoder movements,
    // I guess that possibility would rather cause irritation.
    fn control_relative(
        &mut self,
        discrete_increment: DiscreteIncrement,
        target: &impl Target,
    ) -> Option<ControlValue> {
        use ControlType::*;
        match target.control_type() {
            AbsoluteContinuous
            | AbsoluteContinuousRoundable { .. }
            // TODO-low Controlling a switch/trigger target with +/- n doesn't make sense.
            | AbsoluteSwitch
            | AbsoluteTrigger => {
                // Continuous target
                //
                // Settings which are always necessary:
                // - Minimum target step size (enables accurate minimum increment, atomic)
                // - Target value interval (absolute, important for rotation only, clamped)
                //
                // Settings which are necessary in order to support >1-increments:
                // - Maximum target step size (enables accurate maximum increment, clamped)
                let potentially_reversed_increment = if self.reverse {
                    discrete_increment.inverse()
                } else {
                    discrete_increment
                };
                let unit_increment = potentially_reversed_increment
                    .to_unit_increment(self.step_size_interval.min_val())?;
                let clamped_unit_increment =
                    unit_increment.clamp_to_interval(&self.step_size_interval);
                self.hit_target_absolutely_with_unit_increment(
                    clamped_unit_increment,
                    self.step_size_interval.min_val(),
                    target.current_value()?,
                )
            }
            AbsoluteDiscrete { atomic_step_size } => {
                // Discrete target
                //
                // Settings which are always necessary:
                // - Minimum target step count (enables accurate normal/minimum increment, atomic)
                // - Target value interval (absolute, important for rotation only, clamped)
                //
                // Settings which are necessary in order to support >1-increments:
                // - Maximum target step count (enables accurate maximum increment, clamped)
                let pepped_up_increment = self.pep_up_discrete_increment(discrete_increment)?;
                self.hit_discrete_target_absolutely(pepped_up_increment, atomic_step_size, || {
                    target.current_value()
                })
            }
            Relative | VirtualMulti => {
                // Target wants increments so we just forward them after some preprocessing
                //
                // Settings which are always necessary:
                // - Minimum target step count (enables accurate normal/minimum increment, clamped)
                //
                // Settings which are necessary in order to support >1-increments:
                // - Maximum target step count (enables accurate maximum increment, clamped)
                let pepped_up_increment = self.pep_up_discrete_increment(discrete_increment)?;
                Some(ControlValue::Relative(pepped_up_increment))
            }
            VirtualButton => {
                // Controlling a button target with +/- n doesn't make sense.
                None
            }
        }
    }

    fn pep_up_control_value(
        &self,
        control_value: UnitValue,
        control_type: ControlType,
        current_target_value: Option<UnitValue>,
        min_is_max_behavior: MinIsMaxBehavior,
    ) -> UnitValue {
        // 1. Apply source interval
        let v1 = control_value
            .map_to_unit_interval_from(&self.source_value_interval, min_is_max_behavior);
        // 2. Apply transformation
        let v2 = self
            .control_transformation
            .as_ref()
            .and_then(|t| {
                t.transform(v1, current_target_value.unwrap_or(UnitValue::MIN))
                    .ok()
            })
            .unwrap_or(v1);
        // 3. Apply reverse
        let v3 = if self.reverse { v2.inverse() } else { v2 };
        // 4. Apply target interval
        let v4 = v3.map_from_unit_interval_to(&self.target_value_interval);
        // 5. Apply rounding
        let v5 = if self.round_target_value {
            round_to_nearest_discrete_value(control_type, v4)
        } else {
            v4
        };
        // Return
        v5
    }

    fn hitting_target_considering_max_jump(
        &self,
        control_value: UnitValue,
        current_target_value: Option<UnitValue>,
        control_type: ControlType,
    ) -> Option<UnitValue> {
        let current_target_value = match current_target_value {
            // No target value available ... just deliver! Virtual targets take this shortcut.
            None => return Some(control_value),
            Some(v) => v,
        };
        if self.jump_interval.is_full() {
            // No jump restrictions whatsoever
            return self.hit_if_changed(control_value, current_target_value, control_type);
        }
        let distance = control_value.calc_distance_from(current_target_value);
        if distance > self.jump_interval.max_val() {
            // Distance is too large
            if !self.approach_target_value {
                // Scaling not desired. Do nothing.
                return None;
            }
            // Scaling desired
            let approach_distance = distance.map_from_unit_interval_to(&self.jump_interval);
            let approach_increment = approach_distance
                .to_increment(negative_if(control_value < current_target_value))?;
            let final_target_value =
                current_target_value.add_clamping(approach_increment, &self.target_value_interval);
            return self.hit_if_changed(final_target_value, current_target_value, control_type);
        }
        // Distance is not too large
        if distance < self.jump_interval.min_val() {
            return None;
        }
        // Distance is also not too small
        self.hit_if_changed(control_value, current_target_value, control_type)
    }

    fn hit_if_changed(
        &self,
        desired_target_value: UnitValue,
        current_target_value: UnitValue,
        control_type: ControlType,
    ) -> Option<UnitValue> {
        if !control_type.is_trigger() && current_target_value == desired_target_value {
            return None;
        }
        Some(desired_target_value)
    }

    fn hit_discrete_target_absolutely(
        &self,
        discrete_increment: DiscreteIncrement,
        target_step_size: UnitValue,
        current_value: impl Fn() -> Option<UnitValue>,
    ) -> Option<ControlValue> {
        let unit_increment = discrete_increment.to_unit_increment(target_step_size)?;
        self.hit_target_absolutely_with_unit_increment(
            unit_increment,
            target_step_size,
            current_value()?,
        )
    }

    fn hit_target_absolutely_with_unit_increment(
        &self,
        increment: UnitIncrement,
        grid_interval_size: UnitValue,
        current_target_value: UnitValue,
    ) -> Option<ControlValue> {
        let snapped_target_value_interval = Interval::new(
            self.target_value_interval
                .min_val()
                .snap_to_grid_by_interval_size(grid_interval_size),
            self.target_value_interval
                .max_val()
                .snap_to_grid_by_interval_size(grid_interval_size),
        );
        // The add functions don't add anything if the current target value is not within the target
        // interval in the first place. Instead they return one of the interval bounds. One issue
        // that might occur is that the current target value only *appears* out-of-range
        // because of numerical inaccuracies. That could lead to frustrating "it doesn't move"
        // experiences. Therefore we snap the current target value to grid first in that case.
        let snapped_current_target_value =
            if current_target_value.is_within_interval(&snapped_target_value_interval) {
                current_target_value
            } else {
                current_target_value.snap_to_grid_by_interval_size(grid_interval_size)
            };
        let desired_target_value = if self.rotate {
            snapped_current_target_value.add_rotating(increment, &snapped_target_value_interval)
        } else {
            snapped_current_target_value.add_clamping(increment, &snapped_target_value_interval)
        };
        if desired_target_value == current_target_value {
            return None;
        }
        Some(ControlValue::Absolute(desired_target_value))
    }

    fn pep_up_discrete_increment(
        &mut self,
        increment: DiscreteIncrement,
    ) -> Option<DiscreteIncrement> {
        let factor = increment.clamp_to_interval(&self.step_count_interval);
        let actual_increment = if factor.is_positive() {
            factor
        } else {
            let nth = factor.get().abs() as u32;
            let (fire, new_counter_value) = self.its_time_to_fire(nth, increment.signum());
            self.increment_counter = new_counter_value;
            if !fire {
                return None;
            }
            DiscreteIncrement::new(1)
        };
        let clamped_increment = actual_increment.with_direction(increment.signum());
        let result = if self.reverse {
            clamped_increment.inverse()
        } else {
            clamped_increment
        };
        Some(result)
    }

    /// `nth` stands for "fire every nth time". `direction_signum` is either +1 or -1.
    fn its_time_to_fire(&self, nth: u32, direction_signum: i32) -> (bool, i32) {
        if self.increment_counter == 0 {
            // Initial fire
            return (true, direction_signum);
        }
        if self.increment_counter.signum() != direction_signum {
            // Change of direction. In this case always fire.
            return (true, direction_signum);
        }
        let positive_increment_counter = self.increment_counter.abs() as u32;
        if positive_increment_counter >= nth {
            // After having waited for a few increments, fire again.
            return (true, direction_signum);
        }
        (false, self.increment_counter + direction_signum)
    }

    fn convert_to_discrete_increment(
        &mut self,
        control_value: UnitValue,
    ) -> Option<DiscreteIncrement> {
        let factor = control_value
            .map_to_unit_interval_from(&self.source_value_interval, MinIsMaxBehavior::PreferOne)
            .map_from_unit_interval_to_discrete_increment(&self.step_count_interval);
        // This mode supports positive increment only.
        let discrete_value = if factor.is_positive() {
            factor.to_value()
        } else {
            let nth = factor.get().abs() as u32;
            let (fire, new_counter_value) = self.its_time_to_fire(nth, 1);
            self.increment_counter = new_counter_value;
            if !fire {
                return None;
            }
            DiscreteValue::new(1)
        };
        discrete_value.to_increment(negative_if(self.reverse))
    }
}

fn round_to_nearest_discrete_value(
    control_type: ControlType,
    approximate_control_value: UnitValue,
) -> UnitValue {
    // round() is the right choice here vs. floor() because we don't want slight numerical
    // inaccuracies lead to surprising jumps
    use ControlType::*;
    let step_size = match control_type {
        AbsoluteContinuousRoundable { rounding_step_size } => rounding_step_size,
        AbsoluteDiscrete { atomic_step_size } => atomic_step_size,
        AbsoluteTrigger | AbsoluteSwitch | AbsoluteContinuous | Relative | VirtualMulti
        | VirtualButton => return approximate_control_value,
    };
    approximate_control_value.snap_to_grid_by_interval_size(step_size)
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::mode::test_util::{TestTarget, TestTransformation};
    use crate::{create_unit_value_interval, ControlType};
    use approx::*;

    mod absolute_normal {
        use super::*;

        #[test]
        fn default() {
            // Given
            let mut mode: Mode<TestTransformation> = Mode {
                ..Default::default()
            };
            let target = TestTarget {
                current_value: Some(UnitValue::new(0.777)),
                control_type: ControlType::AbsoluteContinuous,
            };
            // When
            // Then
            assert_abs_diff_eq!(mode.control(abs(0.0), &target).unwrap(), abs(0.0));
            assert_abs_diff_eq!(mode.control(abs(0.5), &target).unwrap(), abs(0.5));
            assert!(mode.control(abs(0.777), &target).is_none());
            assert_abs_diff_eq!(mode.control(abs(1.0), &target).unwrap(), abs(1.0));
        }

        #[test]
        fn default_target_is_trigger() {
            // Given
            let mut mode: Mode<TestTransformation> = Mode {
                ..Default::default()
            };
            let target = TestTarget {
                current_value: Some(UnitValue::new(0.777)),
                control_type: ControlType::AbsoluteTrigger,
            };
            // When
            // Then
            assert_abs_diff_eq!(mode.control(abs(0.0), &target).unwrap(), abs(0.0));
            assert_abs_diff_eq!(mode.control(abs(0.5), &target).unwrap(), abs(0.5));
            assert_abs_diff_eq!(mode.control(abs(0.777), &target).unwrap(), abs(0.777));
            assert_abs_diff_eq!(mode.control(abs(1.0), &target).unwrap(), abs(1.0));
        }

        #[test]
        fn relative_target() {
            // Given
            let mut mode: Mode<TestTransformation> = Mode {
                ..Default::default()
            };
            let target = TestTarget {
                current_value: Some(UnitValue::new(0.777)),
                control_type: ControlType::Relative,
            };
            // When
            // Then
            assert_abs_diff_eq!(mode.control(abs(0.0), &target).unwrap(), abs(0.0));
            assert_abs_diff_eq!(mode.control(abs(0.5), &target).unwrap(), abs(0.5));
            assert!(mode.control(abs(0.777), &target).is_none());
            assert_abs_diff_eq!(mode.control(abs(1.0), &target).unwrap(), abs(1.0));
        }

        #[test]
        fn source_interval() {
            // Given
            let mut mode: Mode<TestTransformation> = Mode {
                source_value_interval: create_unit_value_interval(0.2, 0.6),
                ..Default::default()
            };
            let target = TestTarget {
                current_value: Some(UnitValue::new(0.777)),
                control_type: ControlType::AbsoluteContinuous,
            };
            // When
            // Then
            assert_abs_diff_eq!(mode.control(abs(0.0), &target).unwrap(), abs(0.0));
            assert_abs_diff_eq!(mode.control(abs(0.1), &target).unwrap(), abs(0.0));
            assert_abs_diff_eq!(mode.control(abs(0.2), &target).unwrap(), abs(0.0));
            assert_abs_diff_eq!(mode.control(abs(0.4), &target).unwrap(), abs(0.5));
            assert_abs_diff_eq!(mode.control(abs(0.6), &target).unwrap(), abs(1.0));
            assert_abs_diff_eq!(mode.control(abs(0.8), &target).unwrap(), abs(1.0));
            assert_abs_diff_eq!(mode.control(abs(1.0), &target).unwrap(), abs(1.0));
        }

        #[test]
        fn source_interval_out_of_range_ignore() {
            // Given
            let mut mode: Mode<TestTransformation> = Mode {
                source_value_interval: create_unit_value_interval(0.2, 0.6),
                out_of_range_behavior: OutOfRangeBehavior::Ignore,
                ..Default::default()
            };
            let target = TestTarget {
                current_value: Some(UnitValue::new(0.777)),
                control_type: ControlType::AbsoluteContinuous,
            };
            // When
            // Then
            assert!(mode.control(abs(0.0), &target).is_none());
            assert!(mode.control(abs(0.1), &target).is_none());
            assert_abs_diff_eq!(mode.control(abs(0.2), &target).unwrap(), abs(0.0));
            assert_abs_diff_eq!(mode.control(abs(0.4), &target).unwrap(), abs(0.5));
            assert_abs_diff_eq!(mode.control(abs(0.6), &target).unwrap(), abs(1.0));
            assert!(mode.control(abs(0.8), &target).is_none());
            assert!(mode.control(abs(1.0), &target).is_none());
        }

        #[test]
        fn source_interval_out_of_range_min() {
            // Given
            let mut mode: Mode<TestTransformation> = Mode {
                source_value_interval: create_unit_value_interval(0.2, 0.6),
                out_of_range_behavior: OutOfRangeBehavior::Min,
                ..Default::default()
            };
            let target = TestTarget {
                current_value: Some(UnitValue::new(0.777)),
                control_type: ControlType::AbsoluteContinuous,
            };
            // When
            // Then
            assert_abs_diff_eq!(mode.control(abs(0.0), &target).unwrap(), abs(0.0));
            assert_abs_diff_eq!(mode.control(abs(0.1), &target).unwrap(), abs(0.0));
            assert_abs_diff_eq!(mode.control(abs(0.2), &target).unwrap(), abs(0.0));
            assert_abs_diff_eq!(mode.control(abs(0.4), &target).unwrap(), abs(0.5));
            assert_abs_diff_eq!(mode.control(abs(0.6), &target).unwrap(), abs(1.0));
            assert_abs_diff_eq!(mode.control(abs(0.8), &target).unwrap(), abs(0.0));
            assert_abs_diff_eq!(mode.control(abs(1.0), &target).unwrap(), abs(0.0));
        }

        #[test]
        fn source_interval_out_of_range_ignore_source_one_value() {
            // Given
            let mut mode: Mode<TestTransformation> = Mode {
                source_value_interval: create_unit_value_interval(0.5, 0.5),
                out_of_range_behavior: OutOfRangeBehavior::Ignore,
                ..Default::default()
            };
            let target = TestTarget {
                current_value: Some(UnitValue::new(0.777)),
                control_type: ControlType::AbsoluteContinuous,
            };
            // When
            // Then
            assert!(mode.control(abs(0.0), &target).is_none());
            assert!(mode.control(abs(0.4), &target).is_none());
            assert_abs_diff_eq!(mode.control(abs(0.5), &target).unwrap(), abs(1.0));
            assert!(mode.control(abs(0.6), &target).is_none());
            assert!(mode.control(abs(1.0), &target).is_none());
        }

        #[test]
        fn source_interval_out_of_range_min_source_one_value() {
            // Given
            let mut mode: Mode<TestTransformation> = Mode {
                source_value_interval: create_unit_value_interval(0.5, 0.5),
                out_of_range_behavior: OutOfRangeBehavior::Min,
                ..Default::default()
            };
            let target = TestTarget {
                current_value: Some(UnitValue::new(0.777)),
                control_type: ControlType::AbsoluteContinuous,
            };
            // When
            // Then
            assert_abs_diff_eq!(mode.control(abs(0.0), &target).unwrap(), abs(0.0));
            assert_abs_diff_eq!(mode.control(abs(0.4), &target).unwrap(), abs(0.0));
            assert_abs_diff_eq!(mode.control(abs(0.5), &target).unwrap(), abs(1.0));
            assert_abs_diff_eq!(mode.control(abs(0.6), &target).unwrap(), abs(0.0));
            assert_abs_diff_eq!(mode.control(abs(1.0), &target).unwrap(), abs(0.0));
        }

        #[test]
        fn source_interval_out_of_range_min_max_source_one_value() {
            // Given
            let mut mode: Mode<TestTransformation> = Mode {
                source_value_interval: create_unit_value_interval(0.5, 0.5),
                out_of_range_behavior: OutOfRangeBehavior::MinOrMax,
                ..Default::default()
            };
            let target = TestTarget {
                current_value: Some(UnitValue::new(0.777)),
                control_type: ControlType::AbsoluteContinuous,
            };
            // When
            // Then
            assert_abs_diff_eq!(mode.control(abs(0.0), &target).unwrap(), abs(0.0));
            assert_abs_diff_eq!(mode.control(abs(0.4), &target).unwrap(), abs(0.0));
            assert_abs_diff_eq!(mode.control(abs(0.5), &target).unwrap(), abs(1.0));
            assert_abs_diff_eq!(mode.control(abs(0.6), &target).unwrap(), abs(1.0));
            assert_abs_diff_eq!(mode.control(abs(1.0), &target).unwrap(), abs(1.0));
        }

        #[test]
        fn target_interval() {
            // Given
            let mut mode: Mode<TestTransformation> = Mode {
                target_value_interval: create_unit_value_interval(0.2, 0.6),
                ..Default::default()
            };
            let target = TestTarget {
                current_value: Some(UnitValue::new(0.777)),
                control_type: ControlType::AbsoluteContinuous,
            };
            // When
            // Then
            assert_abs_diff_eq!(mode.control(abs(0.0), &target).unwrap(), abs(0.2));
            assert_abs_diff_eq!(mode.control(abs(0.2), &target).unwrap(), abs(0.28));
            assert_abs_diff_eq!(mode.control(abs(0.25), &target).unwrap(), abs(0.3));
            assert_abs_diff_eq!(mode.control(abs(0.5), &target).unwrap(), abs(0.4));
            assert_abs_diff_eq!(mode.control(abs(0.75), &target).unwrap(), abs(0.5));
            assert_abs_diff_eq!(mode.control(abs(1.0), &target).unwrap(), abs(0.6));
        }

        #[test]
        fn target_interval_reverse() {
            // Given
            let mut mode: Mode<TestTransformation> = Mode {
                target_value_interval: create_unit_value_interval(0.6, 1.0),
                reverse: true,
                ..Default::default()
            };
            let target = TestTarget {
                current_value: Some(UnitValue::new(0.777)),
                control_type: ControlType::AbsoluteContinuous,
            };
            // When
            // Then
            assert_abs_diff_eq!(mode.control(abs(0.0), &target).unwrap(), abs(1.0));
            assert_abs_diff_eq!(mode.control(abs(0.25), &target).unwrap(), abs(0.9));
            assert_abs_diff_eq!(mode.control(abs(0.5), &target).unwrap(), abs(0.8));
            assert_abs_diff_eq!(mode.control(abs(0.75), &target).unwrap(), abs(0.7));
            assert_abs_diff_eq!(mode.control(abs(1.0), &target).unwrap(), abs(0.6));
        }

        #[test]
        fn source_and_target_interval() {
            // Given
            let mut mode: Mode<TestTransformation> = Mode {
                source_value_interval: create_unit_value_interval(0.2, 0.6),
                target_value_interval: create_unit_value_interval(0.2, 0.6),
                ..Default::default()
            };
            let target = TestTarget {
                current_value: Some(UnitValue::new(0.777)),
                control_type: ControlType::AbsoluteContinuous,
            };
            // When
            // Then
            assert_abs_diff_eq!(mode.control(abs(0.0), &target).unwrap(), abs(0.2));
            assert_abs_diff_eq!(mode.control(abs(0.2), &target).unwrap(), abs(0.2));
            assert_abs_diff_eq!(mode.control(abs(0.4), &target).unwrap(), abs(0.4));
            assert_abs_diff_eq!(mode.control(abs(0.6), &target).unwrap(), abs(0.6));
            assert_abs_diff_eq!(mode.control(abs(0.8), &target).unwrap(), abs(0.6));
            assert_abs_diff_eq!(mode.control(abs(1.0), &target).unwrap(), abs(0.6));
        }

        #[test]
        fn source_and_target_interval_shifted() {
            // Given
            let mut mode: Mode<TestTransformation> = Mode {
                source_value_interval: create_unit_value_interval(0.2, 0.6),
                target_value_interval: create_unit_value_interval(0.4, 0.8),
                ..Default::default()
            };
            let target = TestTarget {
                current_value: Some(UnitValue::new(0.777)),
                control_type: ControlType::AbsoluteContinuous,
            };
            // When
            // Then
            assert_abs_diff_eq!(mode.control(abs(0.0), &target).unwrap(), abs(0.4));
            assert_abs_diff_eq!(mode.control(abs(0.2), &target).unwrap(), abs(0.4));
            assert_abs_diff_eq!(mode.control(abs(0.4), &target).unwrap(), abs(0.6));
            assert_abs_diff_eq!(mode.control(abs(0.6), &target).unwrap(), abs(0.8));
            assert_abs_diff_eq!(mode.control(abs(0.8), &target).unwrap(), abs(0.8));
            assert_abs_diff_eq!(mode.control(abs(1.0), &target).unwrap(), abs(0.8));
        }

        #[test]
        fn reverse() {
            // Given
            let mut mode: Mode<TestTransformation> = Mode {
                reverse: true,
                ..Default::default()
            };
            let target = TestTarget {
                current_value: Some(UnitValue::new(0.777)),
                control_type: ControlType::AbsoluteContinuous,
            };
            // When
            // Then
            assert_abs_diff_eq!(mode.control(abs(0.0), &target).unwrap(), abs(1.0));
            assert_abs_diff_eq!(mode.control(abs(0.5), &target).unwrap(), abs(0.5));
            assert_abs_diff_eq!(mode.control(abs(1.0), &target).unwrap(), abs(0.0));
        }

        #[test]
        fn round() {
            // Given
            let mut mode: Mode<TestTransformation> = Mode {
                round_target_value: true,
                ..Default::default()
            };
            let target = TestTarget {
                current_value: Some(UnitValue::new(0.777)),
                control_type: ControlType::AbsoluteDiscrete {
                    atomic_step_size: UnitValue::new(0.2),
                },
            };
            // When
            // Then
            assert_abs_diff_eq!(mode.control(abs(0.0), &target).unwrap(), abs(0.0));
            assert_abs_diff_eq!(mode.control(abs(0.11), &target).unwrap(), abs(0.2));
            assert_abs_diff_eq!(mode.control(abs(0.19), &target).unwrap(), abs(0.2));
            assert_abs_diff_eq!(mode.control(abs(0.2), &target).unwrap(), abs(0.2));
            assert_abs_diff_eq!(mode.control(abs(0.35), &target).unwrap(), abs(0.4));
            assert_abs_diff_eq!(mode.control(abs(0.49), &target).unwrap(), abs(0.4));
            assert_abs_diff_eq!(mode.control(abs(1.0), &target).unwrap(), abs(1.0));
        }

        #[test]
        fn jump_interval() {
            // Given
            let mut mode: Mode<TestTransformation> = Mode {
                jump_interval: create_unit_value_interval(0.0, 0.2),
                ..Default::default()
            };
            let target = TestTarget {
                current_value: Some(UnitValue::new(0.5)),
                control_type: ControlType::AbsoluteContinuous,
            };
            // When
            // Then
            assert!(mode.control(abs(0.0), &target).is_none());
            assert!(mode.control(abs(0.1), &target).is_none());
            assert_abs_diff_eq!(mode.control(abs(0.4), &target).unwrap(), abs(0.4));
            assert_abs_diff_eq!(mode.control(abs(0.6), &target).unwrap(), abs(0.6));
            assert_abs_diff_eq!(mode.control(abs(0.7), &target).unwrap(), abs(0.7));
            assert!(mode.control(abs(0.8), &target).is_none());
            assert!(mode.control(abs(0.9), &target).is_none());
            assert!(mode.control(abs(1.0), &target).is_none());
        }

        #[test]
        fn jump_interval_min() {
            // Given
            let mut mode: Mode<TestTransformation> = Mode {
                jump_interval: create_unit_value_interval(0.1, 1.0),
                ..Default::default()
            };
            let target = TestTarget {
                current_value: Some(UnitValue::new(0.5)),
                control_type: ControlType::AbsoluteContinuous,
            };
            // When
            // Then
            assert_abs_diff_eq!(mode.control(abs(0.1), &target).unwrap(), abs(0.1));
            assert!(mode.control(abs(0.4), &target).is_none());
            assert!(mode.control(abs(0.5), &target).is_none());
            assert!(mode.control(abs(0.6), &target).is_none());
            assert_abs_diff_eq!(mode.control(abs(1.0), &target).unwrap(), abs(1.0));
        }

        #[test]
        fn jump_interval_approach() {
            // Given
            let mut mode: Mode<TestTransformation> = Mode {
                jump_interval: create_unit_value_interval(0.0, 0.2),
                approach_target_value: true,
                ..Default::default()
            };
            let target = TestTarget {
                current_value: Some(UnitValue::new(0.5)),
                control_type: ControlType::AbsoluteContinuous,
            };
            // When
            // Then
            assert_abs_diff_eq!(mode.control(abs(0.0), &target).unwrap(), abs(0.4));
            assert_abs_diff_eq!(mode.control(abs(0.1), &target).unwrap(), abs(0.42));
            assert_abs_diff_eq!(mode.control(abs(0.4), &target).unwrap(), abs(0.4));
            assert_abs_diff_eq!(mode.control(abs(0.6), &target).unwrap(), abs(0.6));
            assert_abs_diff_eq!(mode.control(abs(0.7), &target).unwrap(), abs(0.7));
            assert_abs_diff_eq!(mode.control(abs(0.8), &target).unwrap(), abs(0.56));
            assert_abs_diff_eq!(mode.control(abs(1.0), &target).unwrap(), abs(0.6));
        }

        #[test]
        fn transformation_ok() {
            // Given
            let mut mode: Mode<TestTransformation> = Mode {
                control_transformation: Some(TestTransformation::new(|input| Ok(input.inverse()))),
                ..Default::default()
            };
            let target = TestTarget {
                current_value: Some(UnitValue::new(0.777)),
                control_type: ControlType::AbsoluteContinuous,
            };
            // When
            // Then
            assert_abs_diff_eq!(mode.control(abs(0.0), &target).unwrap(), abs(1.0));
            assert_abs_diff_eq!(mode.control(abs(0.5), &target).unwrap(), abs(0.5));
            assert_abs_diff_eq!(mode.control(abs(1.0), &target).unwrap(), abs(0.0));
        }

        #[test]
        fn transformation_err() {
            // Given
            let mut mode: Mode<TestTransformation> = Mode {
                control_transformation: Some(TestTransformation::new(|_| Err("oh no!"))),
                ..Default::default()
            };
            let target = TestTarget {
                current_value: Some(UnitValue::new(0.777)),
                control_type: ControlType::AbsoluteContinuous,
            };
            // When
            // Then
            assert_abs_diff_eq!(mode.control(abs(0.0), &target).unwrap(), abs(0.0));
            assert_abs_diff_eq!(mode.control(abs(0.5), &target).unwrap(), abs(0.5));
            assert_abs_diff_eq!(mode.control(abs(1.0), &target).unwrap(), abs(1.0));
        }

        #[test]
        fn feedback() {
            // Given
            let mode: Mode<TestTransformation> = Mode {
                ..Default::default()
            };
            // When
            // Then
            assert_abs_diff_eq!(mode.feedback(uv(0.0)).unwrap(), uv(0.0));
            assert_abs_diff_eq!(mode.feedback(uv(0.5)).unwrap(), uv(0.5));
            assert_abs_diff_eq!(mode.feedback(uv(1.0)).unwrap(), uv(1.0));
        }

        #[test]
        fn feedback_reverse() {
            // Given
            let mode: Mode<TestTransformation> = Mode {
                reverse: true,
                ..Default::default()
            };
            // When
            // Then
            assert_abs_diff_eq!(mode.feedback(uv(0.0)).unwrap(), uv(1.0));
            assert_abs_diff_eq!(mode.feedback(uv(0.5)).unwrap(), uv(0.5));
            assert_abs_diff_eq!(mode.feedback(uv(1.0)).unwrap(), uv(0.0));
        }

        #[test]
        fn feedback_target_interval() {
            // Given
            let mode: Mode<TestTransformation> = Mode {
                target_value_interval: create_unit_value_interval(0.2, 1.0),
                ..Default::default()
            };
            // When
            // Then
            assert_abs_diff_eq!(mode.feedback(uv(0.0)).unwrap(), uv(0.0));
            assert_abs_diff_eq!(mode.feedback(uv(0.2)).unwrap(), uv(0.0));
            assert_abs_diff_eq!(mode.feedback(uv(0.4)).unwrap(), uv(0.25));
            assert_abs_diff_eq!(mode.feedback(uv(0.6)).unwrap(), uv(0.5));
            assert_abs_diff_eq!(mode.feedback(uv(0.8)).unwrap(), uv(0.75));
            assert_abs_diff_eq!(mode.feedback(uv(1.0)).unwrap(), uv(1.0));
        }

        #[test]
        fn feedback_target_interval_reverse() {
            // Given
            let mode: Mode<TestTransformation> = Mode {
                target_value_interval: create_unit_value_interval(0.2, 1.0),
                reverse: true,
                ..Default::default()
            };
            // When
            // Then
            assert_abs_diff_eq!(mode.feedback(uv(0.0)).unwrap(), uv(1.0));
            assert_abs_diff_eq!(mode.feedback(uv(0.2)).unwrap(), uv(1.0));
            assert_abs_diff_eq!(mode.feedback(uv(0.4)).unwrap(), uv(0.75));
            assert_abs_diff_eq!(mode.feedback(uv(0.6)).unwrap(), uv(0.5));
            assert_abs_diff_eq!(mode.feedback(uv(0.8)).unwrap(), uv(0.25));
            assert_abs_diff_eq!(mode.feedback(uv(1.0)).unwrap(), uv(0.0));
        }

        #[test]
        fn feedback_source_and_target_interval() {
            // Given
            let mode: Mode<TestTransformation> = Mode {
                source_value_interval: create_unit_value_interval(0.2, 0.8),
                target_value_interval: create_unit_value_interval(0.4, 1.0),
                ..Default::default()
            };
            // When
            // Then
            assert_abs_diff_eq!(mode.feedback(uv(0.0)).unwrap(), uv(0.2));
            assert_abs_diff_eq!(mode.feedback(uv(0.4)).unwrap(), uv(0.2));
            assert_abs_diff_eq!(mode.feedback(uv(0.7)).unwrap(), uv(0.5));
            assert_abs_diff_eq!(mode.feedback(uv(1.0)).unwrap(), uv(0.8));
        }

        #[test]
        fn feedback_out_of_range_ignore() {
            // Given
            let mode: Mode<TestTransformation> = Mode {
                target_value_interval: create_unit_value_interval(0.2, 0.8),
                out_of_range_behavior: OutOfRangeBehavior::Ignore,
                ..Default::default()
            };
            // When
            // Then
            assert!(mode.feedback(uv(0.0)).is_none());
            assert_abs_diff_eq!(mode.feedback(uv(0.5)).unwrap(), uv(0.5));
            assert!(mode.feedback(uv(1.0)).is_none());
        }

        #[test]
        fn feedback_out_of_range_min() {
            // Given
            let mode: Mode<TestTransformation> = Mode {
                target_value_interval: create_unit_value_interval(0.2, 0.8),
                out_of_range_behavior: OutOfRangeBehavior::Min,
                ..Default::default()
            };
            // When
            // Then
            assert_abs_diff_eq!(mode.feedback(uv(0.0)).unwrap(), uv(0.0));
            assert_abs_diff_eq!(mode.feedback(uv(0.1)).unwrap(), uv(0.0));
            assert_abs_diff_eq!(mode.feedback(uv(0.5)).unwrap(), uv(0.5));
            assert_abs_diff_eq!(mode.feedback(uv(0.9)).unwrap(), uv(0.0));
            assert_abs_diff_eq!(mode.feedback(uv(1.0)).unwrap(), uv(0.0));
        }

        #[test]
        fn feedback_out_of_range_min_target_one_value() {
            // Given
            let mode: Mode<TestTransformation> = Mode {
                target_value_interval: create_unit_value_interval(0.5, 0.5),
                out_of_range_behavior: OutOfRangeBehavior::Min,
                ..Default::default()
            };
            // When
            // Then
            assert_abs_diff_eq!(mode.feedback(uv(0.0)).unwrap(), uv(0.0));
            assert_abs_diff_eq!(mode.feedback(uv(0.1)).unwrap(), uv(0.0));
            assert_abs_diff_eq!(mode.feedback(uv(0.5)).unwrap(), uv(1.0));
            assert_abs_diff_eq!(mode.feedback(uv(0.9)).unwrap(), uv(0.0));
            assert_abs_diff_eq!(mode.feedback(uv(1.0)).unwrap(), uv(0.0));
        }

        #[test]
        fn feedback_out_of_range_min_max_target_one_value() {
            // Given
            let mode: Mode<TestTransformation> = Mode {
                target_value_interval: create_unit_value_interval(0.5, 0.5),
                ..Default::default()
            };
            // When
            // Then
            assert_abs_diff_eq!(mode.feedback(uv(0.0)).unwrap(), uv(0.0));
            assert_abs_diff_eq!(mode.feedback(uv(0.1)).unwrap(), uv(0.0));
            assert_abs_diff_eq!(mode.feedback(uv(0.5)).unwrap(), uv(1.0));
            assert_abs_diff_eq!(mode.feedback(uv(0.9)).unwrap(), uv(1.0));
            assert_abs_diff_eq!(mode.feedback(uv(1.0)).unwrap(), uv(1.0));
        }

        #[test]
        fn feedback_out_of_range_ignore_target_one_value() {
            // Given
            let mode: Mode<TestTransformation> = Mode {
                target_value_interval: create_unit_value_interval(0.5, 0.5),
                out_of_range_behavior: OutOfRangeBehavior::Ignore,
                ..Default::default()
            };
            // When
            // Then
            assert!(mode.feedback(uv(0.0)).is_none());
            assert!(mode.feedback(uv(0.1)).is_none());
            assert_abs_diff_eq!(mode.feedback(uv(0.5)).unwrap(), uv(1.0));
            assert!(mode.feedback(uv(0.9)).is_none());
            assert!(mode.feedback(uv(1.0)).is_none());
        }

        #[test]
        fn feedback_transformation() {
            // Given
            let mode: Mode<TestTransformation> = Mode {
                feedback_transformation: Some(TestTransformation::new(|input| Ok(input.inverse()))),
                ..Default::default()
            };
            // When
            // Then
            assert_abs_diff_eq!(mode.feedback(uv(0.0)).unwrap(), uv(1.0));
            assert_abs_diff_eq!(mode.feedback(uv(0.5)).unwrap(), uv(0.5));
            assert_abs_diff_eq!(mode.feedback(uv(1.0)).unwrap(), uv(0.0));
        }
    }

    mod absolute_toggle {

        use super::*;

        #[test]
        fn absolute_value_target_off() {
            // Given
            let mut mode: Mode<TestTransformation> = Mode {
                absolute_mode: AbsoluteMode::ToggleButtons,
                ..Default::default()
            };
            let target = TestTarget {
                current_value: Some(UnitValue::MIN),
                control_type: ControlType::AbsoluteContinuous,
            };
            // When
            // Then
            assert!(mode.control(abs(0.0), &target).is_none());
            assert_abs_diff_eq!(mode.control(abs(0.1), &target).unwrap(), abs(1.0));
            assert_abs_diff_eq!(mode.control(abs(0.5), &target).unwrap(), abs(1.0));
            assert_abs_diff_eq!(mode.control(abs(1.0), &target).unwrap(), abs(1.0));
        }

        #[test]
        fn absolute_value_target_on() {
            // Given
            let mut mode: Mode<TestTransformation> = Mode {
                absolute_mode: AbsoluteMode::ToggleButtons,
                ..Default::default()
            };
            let target = TestTarget {
                current_value: Some(UnitValue::MAX),
                control_type: ControlType::AbsoluteContinuous,
            };
            // When
            // Then
            assert!(mode.control(abs(0.0), &target).is_none());
            assert_abs_diff_eq!(mode.control(abs(0.1), &target).unwrap(), abs(0.0));
            assert_abs_diff_eq!(mode.control(abs(0.5), &target).unwrap(), abs(0.0));
            assert_abs_diff_eq!(mode.control(abs(1.0), &target).unwrap(), abs(0.0));
        }

        #[test]
        fn absolute_value_target_rather_off() {
            // Given
            let mut mode: Mode<TestTransformation> = Mode {
                absolute_mode: AbsoluteMode::ToggleButtons,
                ..Default::default()
            };
            let target = TestTarget {
                current_value: Some(UnitValue::new(0.333)),
                control_type: ControlType::AbsoluteContinuous,
            };
            // When
            // Then
            assert!(mode.control(abs(0.0), &target).is_none());
            assert_abs_diff_eq!(mode.control(abs(0.1), &target).unwrap(), abs(1.0));
            assert_abs_diff_eq!(mode.control(abs(0.5), &target).unwrap(), abs(1.0));
            assert_abs_diff_eq!(mode.control(abs(1.0), &target).unwrap(), abs(1.0));
        }

        #[test]
        fn absolute_value_target_rather_on() {
            // Given
            let mut mode: Mode<TestTransformation> = Mode {
                absolute_mode: AbsoluteMode::ToggleButtons,
                ..Default::default()
            };
            let target = TestTarget {
                current_value: Some(UnitValue::new(0.777)),
                control_type: ControlType::AbsoluteContinuous,
            };
            // When
            // Then
            assert!(mode.control(abs(0.0), &target).is_none());
            assert_abs_diff_eq!(mode.control(abs(0.1), &target).unwrap(), abs(0.0));
            assert_abs_diff_eq!(mode.control(abs(0.5), &target).unwrap(), abs(0.0));
            assert_abs_diff_eq!(mode.control(abs(1.0), &target).unwrap(), abs(0.0));
        }

        #[test]
        fn absolute_value_target_interval_target_off() {
            // Given
            let mut mode: Mode<TestTransformation> = Mode {
                absolute_mode: AbsoluteMode::ToggleButtons,
                target_value_interval: create_unit_value_interval(0.3, 0.7),
                ..Default::default()
            };
            let target = TestTarget {
                current_value: Some(UnitValue::new(0.3)),
                control_type: ControlType::AbsoluteContinuous,
            };
            // When
            // Then
            assert!(mode.control(abs(0.0), &target).is_none());
            assert_abs_diff_eq!(mode.control(abs(0.1), &target).unwrap(), abs(0.7));
            assert_abs_diff_eq!(mode.control(abs(0.5), &target).unwrap(), abs(0.7));
            assert_abs_diff_eq!(mode.control(abs(1.0), &target).unwrap(), abs(0.7));
        }

        #[test]
        fn absolute_value_target_interval_target_on() {
            // Given
            let mut mode: Mode<TestTransformation> = Mode {
                absolute_mode: AbsoluteMode::ToggleButtons,
                target_value_interval: create_unit_value_interval(0.3, 0.7),
                ..Default::default()
            };
            let target = TestTarget {
                current_value: Some(UnitValue::new(0.7)),
                control_type: ControlType::AbsoluteContinuous,
            };
            // When
            // Then
            assert!(mode.control(abs(0.0), &target).is_none());
            assert_abs_diff_eq!(mode.control(abs(0.1), &target).unwrap(), abs(0.3));
            assert_abs_diff_eq!(mode.control(abs(0.5), &target).unwrap(), abs(0.3));
            assert_abs_diff_eq!(mode.control(abs(1.0), &target).unwrap(), abs(0.3));
        }

        #[test]
        fn absolute_value_target_interval_target_rather_off() {
            // Given
            let mut mode: Mode<TestTransformation> = Mode {
                absolute_mode: AbsoluteMode::ToggleButtons,
                target_value_interval: create_unit_value_interval(0.3, 0.7),
                ..Default::default()
            };
            let target = TestTarget {
                current_value: Some(UnitValue::new(0.4)),
                control_type: ControlType::AbsoluteContinuous,
            };
            // When
            // Then
            assert!(mode.control(abs(0.0), &target).is_none());
            assert_abs_diff_eq!(mode.control(abs(0.1), &target).unwrap(), abs(0.7));
            assert_abs_diff_eq!(mode.control(abs(0.5), &target).unwrap(), abs(0.7));
            assert_abs_diff_eq!(mode.control(abs(1.0), &target).unwrap(), abs(0.7));
        }

        #[test]
        fn absolute_value_target_interval_target_rather_on() {
            // Given
            let mut mode: Mode<TestTransformation> = Mode {
                absolute_mode: AbsoluteMode::ToggleButtons,
                target_value_interval: create_unit_value_interval(0.3, 0.7),
                ..Default::default()
            };
            let target = TestTarget {
                current_value: Some(UnitValue::new(0.6)),
                control_type: ControlType::AbsoluteContinuous,
            };
            // When
            // Then
            assert!(mode.control(abs(0.0), &target).is_none());
            assert_abs_diff_eq!(mode.control(abs(0.1), &target).unwrap(), abs(0.3));
            assert_abs_diff_eq!(mode.control(abs(0.5), &target).unwrap(), abs(0.3));
            assert_abs_diff_eq!(mode.control(abs(1.0), &target).unwrap(), abs(0.3));
        }

        #[test]
        fn absolute_value_target_interval_target_too_off() {
            // Given
            let mut mode: Mode<TestTransformation> = Mode {
                absolute_mode: AbsoluteMode::ToggleButtons,
                target_value_interval: create_unit_value_interval(0.3, 0.7),
                ..Default::default()
            };
            let target = TestTarget {
                current_value: Some(UnitValue::MIN),
                control_type: ControlType::AbsoluteContinuous,
            };
            // When
            // Then
            assert!(mode.control(abs(0.0), &target).is_none());
            assert_abs_diff_eq!(mode.control(abs(0.1), &target).unwrap(), abs(0.7));
            assert_abs_diff_eq!(mode.control(abs(0.5), &target).unwrap(), abs(0.7));
            assert_abs_diff_eq!(mode.control(abs(1.0), &target).unwrap(), abs(0.7));
        }

        #[test]
        fn absolute_value_target_interval_target_too_on() {
            // Given
            let mut mode: Mode<TestTransformation> = Mode {
                absolute_mode: AbsoluteMode::ToggleButtons,
                target_value_interval: create_unit_value_interval(0.3, 0.7),
                ..Default::default()
            };
            let target = TestTarget {
                current_value: Some(UnitValue::MAX),
                control_type: ControlType::AbsoluteContinuous,
            };
            // When
            // Then
            assert!(mode.control(abs(0.0), &target).is_none());
            assert_abs_diff_eq!(mode.control(abs(0.1), &target).unwrap(), abs(0.3));
            assert_abs_diff_eq!(mode.control(abs(0.5), &target).unwrap(), abs(0.3));
            assert_abs_diff_eq!(mode.control(abs(1.0), &target).unwrap(), abs(0.3));
        }

        #[test]
        fn feedback() {
            // Given
            let mode: Mode<TestTransformation> = Mode {
                absolute_mode: AbsoluteMode::ToggleButtons,
                ..Default::default()
            };
            // When
            // Then
            assert_abs_diff_eq!(mode.feedback(uv(0.0)).unwrap(), uv(0.0));
            assert_abs_diff_eq!(mode.feedback(uv(0.5)).unwrap(), uv(0.5));
            assert_abs_diff_eq!(mode.feedback(uv(1.0)).unwrap(), uv(1.0));
        }

        #[test]
        fn feedback_target_interval() {
            // Given
            let mode: Mode<TestTransformation> = Mode {
                absolute_mode: AbsoluteMode::ToggleButtons,
                target_value_interval: create_unit_value_interval(0.3, 0.7),
                ..Default::default()
            };
            // When
            // Then
            assert_abs_diff_eq!(mode.feedback(uv(0.0)).unwrap(), uv(0.0));
            assert_abs_diff_eq!(mode.feedback(uv(0.4)).unwrap(), uv(0.25));
            assert_abs_diff_eq!(mode.feedback(uv(0.7)).unwrap(), uv(1.0));
            assert_abs_diff_eq!(mode.feedback(uv(1.0)).unwrap(), uv(1.0));
        }
    }

    mod relative {
        use super::*;

        mod absolute_continuous_target {
            use super::*;

            #[test]
            fn default_1() {
                // Given
                let mut mode: Mode<TestTransformation> = Mode {
                    ..Default::default()
                };
                let target = TestTarget {
                    current_value: Some(UnitValue::MIN),
                    control_type: ControlType::AbsoluteContinuous,
                };
                // When
                // Then
                assert!(mode.control(rel(-10), &target).is_none());
                assert!(mode.control(rel(-2), &target).is_none());
                assert!(mode.control(rel(-1), &target).is_none());
                assert_abs_diff_eq!(mode.control(rel(1), &target).unwrap(), abs(0.01));
                assert_abs_diff_eq!(mode.control(rel(2), &target).unwrap(), abs(0.01));
                assert_abs_diff_eq!(mode.control(rel(10), &target).unwrap(), abs(0.01));
            }

            #[test]
            fn default_2() {
                // Given
                let mut mode: Mode<TestTransformation> = Mode {
                    ..Default::default()
                };
                let target = TestTarget {
                    current_value: Some(UnitValue::MAX),
                    control_type: ControlType::AbsoluteContinuous,
                };
                // When
                // Then
                assert_abs_diff_eq!(mode.control(rel(-10), &target).unwrap(), abs(0.99));
                assert_abs_diff_eq!(mode.control(rel(-2), &target).unwrap(), abs(0.99));
                assert_abs_diff_eq!(mode.control(rel(-1), &target).unwrap(), abs(0.99));
                assert!(mode.control(rel(1), &target).is_none());
                assert!(mode.control(rel(2), &target).is_none());
                assert!(mode.control(rel(10), &target).is_none());
            }

            #[test]
            fn min_step_size_1() {
                // Given
                let mut mode: Mode<TestTransformation> = Mode {
                    step_size_interval: create_unit_value_interval(0.2, 1.0),
                    ..Default::default()
                };
                let target = TestTarget {
                    current_value: Some(UnitValue::MIN),
                    control_type: ControlType::AbsoluteContinuous,
                };
                // When
                // Then
                assert!(mode.control(rel(-10), &target).is_none());
                assert!(mode.control(rel(-2), &target).is_none());
                assert!(mode.control(rel(-1), &target).is_none());
                assert_abs_diff_eq!(mode.control(rel(1), &target).unwrap(), abs(0.2));
                assert_abs_diff_eq!(mode.control(rel(2), &target).unwrap(), abs(0.4));
                assert_abs_diff_eq!(mode.control(rel(10), &target).unwrap(), abs(1.0));
            }

            #[test]
            fn min_step_size_2() {
                // Given
                let mut mode: Mode<TestTransformation> = Mode {
                    step_size_interval: create_unit_value_interval(0.2, 1.0),
                    ..Default::default()
                };
                let target = TestTarget {
                    current_value: Some(UnitValue::MAX),
                    control_type: ControlType::AbsoluteContinuous,
                };
                // When
                // Then
                assert_abs_diff_eq!(mode.control(rel(-10), &target).unwrap(), abs(0.0));
                assert_abs_diff_eq!(mode.control(rel(-2), &target).unwrap(), abs(0.6));
                assert_abs_diff_eq!(mode.control(rel(-1), &target).unwrap(), abs(0.8));
                assert!(mode.control(rel(1), &target).is_none());
                assert!(mode.control(rel(2), &target).is_none());
                assert!(mode.control(rel(10), &target).is_none());
            }

            #[test]
            fn max_step_size_1() {
                // Given
                let mut mode: Mode<TestTransformation> = Mode {
                    step_size_interval: create_unit_value_interval(0.01, 0.09),
                    ..Default::default()
                };
                let target = TestTarget {
                    current_value: Some(UnitValue::MIN),
                    control_type: ControlType::AbsoluteContinuous,
                };
                // When
                // Then
                assert!(mode.control(rel(-10), &target).is_none());
                assert!(mode.control(rel(-2), &target).is_none());
                assert!(mode.control(rel(-1), &target).is_none());
                assert_abs_diff_eq!(mode.control(rel(1), &target).unwrap(), abs(0.01));
                assert_abs_diff_eq!(mode.control(rel(2), &target).unwrap(), abs(0.02));
                assert_abs_diff_eq!(mode.control(rel(10), &target).unwrap(), abs(0.09));
            }

            #[test]
            fn max_step_size_2() {
                // Given
                let mut mode: Mode<TestTransformation> = Mode {
                    step_size_interval: create_unit_value_interval(0.01, 0.09),
                    ..Default::default()
                };
                let target = TestTarget {
                    current_value: Some(UnitValue::MAX),
                    control_type: ControlType::AbsoluteContinuous,
                };
                // When
                // Then
                assert_abs_diff_eq!(mode.control(rel(-10), &target).unwrap(), abs(0.91));
                assert_abs_diff_eq!(mode.control(rel(-2), &target).unwrap(), abs(0.98));
                assert_abs_diff_eq!(mode.control(rel(-1), &target).unwrap(), abs(0.99));
                assert!(mode.control(rel(1), &target).is_none());
                assert!(mode.control(rel(2), &target).is_none());
                assert!(mode.control(rel(10), &target).is_none());
            }

            #[test]
            fn reverse() {
                // Given
                let mut mode: Mode<TestTransformation> = Mode {
                    reverse: true,
                    ..Default::default()
                };
                let target = TestTarget {
                    current_value: Some(UnitValue::MIN),
                    control_type: ControlType::AbsoluteContinuous,
                };
                // When
                // Then
                assert_abs_diff_eq!(mode.control(rel(-10), &target).unwrap(), abs(0.01));
                assert_abs_diff_eq!(mode.control(rel(-2), &target).unwrap(), abs(0.01));
                assert_abs_diff_eq!(mode.control(rel(-1), &target).unwrap(), abs(0.01));
                assert!(mode.control(rel(1), &target).is_none());
                assert!(mode.control(rel(2), &target).is_none());
                assert!(mode.control(rel(10), &target).is_none());
            }

            #[test]
            fn rotate_1() {
                // Given
                let mut mode: Mode<TestTransformation> = Mode {
                    rotate: true,
                    ..Default::default()
                };
                let target = TestTarget {
                    current_value: Some(UnitValue::MIN),
                    control_type: ControlType::AbsoluteContinuous,
                };
                // When
                // Then
                assert_abs_diff_eq!(mode.control(rel(-10), &target).unwrap(), abs(1.0));
                assert_abs_diff_eq!(mode.control(rel(-2), &target).unwrap(), abs(1.0));
                assert_abs_diff_eq!(mode.control(rel(-1), &target).unwrap(), abs(1.0));
                assert_abs_diff_eq!(mode.control(rel(1), &target).unwrap(), abs(0.01));
                assert_abs_diff_eq!(mode.control(rel(2), &target).unwrap(), abs(0.01));
                assert_abs_diff_eq!(mode.control(rel(10), &target).unwrap(), abs(0.01));
            }

            #[test]
            fn rotate_2() {
                // Given
                let mut mode: Mode<TestTransformation> = Mode {
                    rotate: true,
                    ..Default::default()
                };
                let target = TestTarget {
                    current_value: Some(UnitValue::MAX),
                    control_type: ControlType::AbsoluteContinuous,
                };
                // When
                // Then
                assert_abs_diff_eq!(mode.control(rel(-10), &target).unwrap(), abs(0.99));
                assert_abs_diff_eq!(mode.control(rel(-2), &target).unwrap(), abs(0.99));
                assert_abs_diff_eq!(mode.control(rel(-1), &target).unwrap(), abs(0.99));
                assert_abs_diff_eq!(mode.control(rel(1), &target).unwrap(), abs(0.0));
                assert_abs_diff_eq!(mode.control(rel(2), &target).unwrap(), abs(0.0));
                assert_abs_diff_eq!(mode.control(rel(10), &target).unwrap(), abs(0.0));
            }

            #[test]
            fn target_interval_min() {
                // Given
                let mut mode: Mode<TestTransformation> = Mode {
                    target_value_interval: create_unit_value_interval(0.2, 0.8),
                    ..Default::default()
                };
                let target = TestTarget {
                    current_value: Some(UnitValue::new(0.2)),
                    control_type: ControlType::AbsoluteContinuous,
                };
                // When
                // Then
                assert!(mode.control(rel(-10), &target).is_none());
                assert!(mode.control(rel(-2), &target).is_none());
                assert!(mode.control(rel(-1), &target).is_none());
                assert_abs_diff_eq!(mode.control(rel(1), &target).unwrap(), abs(0.21));
                assert_abs_diff_eq!(mode.control(rel(2), &target).unwrap(), abs(0.21));
                assert_abs_diff_eq!(mode.control(rel(10), &target).unwrap(), abs(0.21));
            }

            #[test]
            fn target_interval_max() {
                // Given
                let mut mode: Mode<TestTransformation> = Mode {
                    target_value_interval: create_unit_value_interval(0.2, 0.8),
                    ..Default::default()
                };
                let target = TestTarget {
                    current_value: Some(UnitValue::new(0.8)),
                    control_type: ControlType::AbsoluteContinuous,
                };
                // When
                // Then
                assert_abs_diff_eq!(mode.control(rel(-10), &target).unwrap(), abs(0.79));
                assert_abs_diff_eq!(mode.control(rel(-2), &target).unwrap(), abs(0.79));
                assert_abs_diff_eq!(mode.control(rel(-1), &target).unwrap(), abs(0.79));
                assert!(mode.control(rel(1), &target).is_none());
                assert!(mode.control(rel(2), &target).is_none());
                assert!(mode.control(rel(10), &target).is_none());
            }

            #[test]
            fn target_interval_current_target_value_out_of_range() {
                // Given
                let mut mode: Mode<TestTransformation> = Mode {
                    target_value_interval: create_unit_value_interval(0.2, 0.8),
                    ..Default::default()
                };
                let target = TestTarget {
                    current_value: Some(UnitValue::MIN),
                    control_type: ControlType::AbsoluteContinuous,
                };
                // When
                // Then
                assert_abs_diff_eq!(mode.control(rel(-10), &target).unwrap(), abs(0.2));
                assert_abs_diff_eq!(mode.control(rel(-2), &target).unwrap(), abs(0.2));
                assert_abs_diff_eq!(mode.control(rel(-1), &target).unwrap(), abs(0.2));
                assert_abs_diff_eq!(mode.control(rel(1), &target).unwrap(), abs(0.2));
                assert_abs_diff_eq!(mode.control(rel(2), &target).unwrap(), abs(0.2));
                assert_abs_diff_eq!(mode.control(rel(10), &target).unwrap(), abs(0.2));
            }

            #[test]
            fn target_interval_current_target_value_just_appearing_out_of_range() {
                // Given
                let mut mode: Mode<TestTransformation> = Mode {
                    target_value_interval: create_unit_value_interval(0.2, 0.8),
                    ..Default::default()
                };
                let target = TestTarget {
                    current_value: Some(UnitValue::new(0.199999999999)),
                    control_type: ControlType::AbsoluteContinuous,
                };
                // When
                // Then
                assert_abs_diff_eq!(mode.control(rel(-10), &target).unwrap(), abs(0.2));
                assert_abs_diff_eq!(mode.control(rel(-2), &target).unwrap(), abs(0.2));
                assert_abs_diff_eq!(mode.control(rel(-1), &target).unwrap(), abs(0.2));
                assert_abs_diff_eq!(mode.control(rel(1), &target).unwrap(), abs(0.21));
                assert_abs_diff_eq!(mode.control(rel(2), &target).unwrap(), abs(0.21));
                assert_abs_diff_eq!(mode.control(rel(10), &target).unwrap(), abs(0.21));
            }

            /// See https://github.com/helgoboss/realearn/issues/100.
            #[test]
            fn not_get_stuck() {
                // Given
                let mut mode: Mode<TestTransformation> = Mode {
                    target_value_interval: full_unit_interval(),
                    step_size_interval: create_unit_value_interval(0.01, 0.01),
                    ..Default::default()
                };
                let target = TestTarget {
                    current_value: Some(UnitValue::new(0.875)),
                    control_type: ControlType::AbsoluteContinuous,
                };
                // When
                // Then
                assert_abs_diff_eq!(mode.control(rel(-1), &target).unwrap(), abs(0.865));
            }

            #[test]
            fn target_interval_min_rotate() {
                // Given
                let mut mode: Mode<TestTransformation> = Mode {
                    target_value_interval: create_unit_value_interval(0.2, 0.8),
                    rotate: true,
                    ..Default::default()
                };
                let target = TestTarget {
                    current_value: Some(UnitValue::new(0.2)),
                    control_type: ControlType::AbsoluteContinuous,
                };
                // When
                // Then
                assert_abs_diff_eq!(mode.control(rel(-10), &target).unwrap(), abs(0.8));
                assert_abs_diff_eq!(mode.control(rel(-2), &target).unwrap(), abs(0.8));
                assert_abs_diff_eq!(mode.control(rel(-1), &target).unwrap(), abs(0.8));
                assert_abs_diff_eq!(mode.control(rel(1), &target).unwrap(), abs(0.21));
                assert_abs_diff_eq!(mode.control(rel(2), &target).unwrap(), abs(0.21));
                assert_abs_diff_eq!(mode.control(rel(10), &target).unwrap(), abs(0.21));
            }

            #[test]
            fn target_interval_max_rotate() {
                // Given
                let mut mode: Mode<TestTransformation> = Mode {
                    target_value_interval: create_unit_value_interval(0.2, 0.8),
                    rotate: true,
                    ..Default::default()
                };
                let target = TestTarget {
                    current_value: Some(UnitValue::new(0.8)),
                    control_type: ControlType::AbsoluteContinuous,
                };
                // When
                // Then
                assert_abs_diff_eq!(mode.control(rel(-10), &target).unwrap(), abs(0.79));
                assert_abs_diff_eq!(mode.control(rel(-2), &target).unwrap(), abs(0.79));
                assert_abs_diff_eq!(mode.control(rel(-1), &target).unwrap(), abs(0.79));
                assert_abs_diff_eq!(mode.control(rel(1), &target).unwrap(), abs(0.2));
                assert_abs_diff_eq!(mode.control(rel(2), &target).unwrap(), abs(0.2));
                assert_abs_diff_eq!(mode.control(rel(10), &target).unwrap(), abs(0.2));
            }

            #[test]
            fn target_interval_rotate_current_target_value_out_of_range() {
                // Given
                let mut mode: Mode<TestTransformation> = Mode {
                    target_value_interval: create_unit_value_interval(0.2, 0.8),
                    rotate: true,
                    ..Default::default()
                };
                let target = TestTarget {
                    current_value: Some(UnitValue::MIN),
                    control_type: ControlType::AbsoluteContinuous,
                };
                // When
                // Then
                assert_abs_diff_eq!(mode.control(rel(-10), &target).unwrap(), abs(0.8));
                assert_abs_diff_eq!(mode.control(rel(-2), &target).unwrap(), abs(0.8));
                assert_abs_diff_eq!(mode.control(rel(-1), &target).unwrap(), abs(0.8));
                assert_abs_diff_eq!(mode.control(rel(1), &target).unwrap(), abs(0.2));
                assert_abs_diff_eq!(mode.control(rel(2), &target).unwrap(), abs(0.2));
                assert_abs_diff_eq!(mode.control(rel(10), &target).unwrap(), abs(0.2));
            }
        }

        mod absolute_discrete_target {
            use super::*;

            #[test]
            fn default_1() {
                // Given
                let mut mode: Mode<TestTransformation> = Mode {
                    ..Default::default()
                };
                let target = TestTarget {
                    current_value: Some(UnitValue::MIN),
                    control_type: ControlType::AbsoluteDiscrete {
                        atomic_step_size: UnitValue::new(0.05),
                    },
                };
                // When
                // Then
                assert!(mode.control(rel(-10), &target).is_none());
                assert!(mode.control(rel(-2), &target).is_none());
                assert!(mode.control(rel(-1), &target).is_none());
                assert_abs_diff_eq!(mode.control(rel(1), &target).unwrap(), abs(0.05));
                assert_abs_diff_eq!(mode.control(rel(2), &target).unwrap(), abs(0.05));
                assert_abs_diff_eq!(mode.control(rel(10), &target).unwrap(), abs(0.05));
            }

            #[test]
            fn default_2() {
                // Given
                let mut mode: Mode<TestTransformation> = Mode {
                    ..Default::default()
                };
                let target = TestTarget {
                    current_value: Some(UnitValue::MAX),
                    control_type: ControlType::AbsoluteDiscrete {
                        atomic_step_size: UnitValue::new(0.05),
                    },
                };
                // When
                // Then
                assert_abs_diff_eq!(mode.control(rel(-10), &target).unwrap(), abs(0.95));
                assert_abs_diff_eq!(mode.control(rel(-2), &target).unwrap(), abs(0.95));
                assert_abs_diff_eq!(mode.control(rel(-1), &target).unwrap(), abs(0.95));
                assert!(mode.control(rel(1), &target).is_none());
                assert!(mode.control(rel(2), &target).is_none());
                assert!(mode.control(rel(10), &target).is_none());
            }

            #[test]
            fn min_step_count_1() {
                // Given
                let mut mode: Mode<TestTransformation> = Mode {
                    step_count_interval: create_discrete_increment_interval(4, 100),
                    ..Default::default()
                };
                let target = TestTarget {
                    current_value: Some(UnitValue::MIN),
                    control_type: ControlType::AbsoluteDiscrete {
                        atomic_step_size: UnitValue::new(0.05),
                    },
                };
                // When
                // Then
                assert!(mode.control(rel(-10), &target).is_none());
                assert!(mode.control(rel(-2), &target).is_none());
                assert!(mode.control(rel(-1), &target).is_none());
                assert_abs_diff_eq!(mode.control(rel(1), &target).unwrap(), abs(0.20)); // 4x
                assert_abs_diff_eq!(mode.control(rel(2), &target).unwrap(), abs(0.25)); // 5x
                assert_abs_diff_eq!(mode.control(rel(4), &target).unwrap(), abs(0.35)); // 7x
                assert_abs_diff_eq!(mode.control(rel(10), &target).unwrap(), abs(0.65)); // 13x
                assert_abs_diff_eq!(mode.control(rel(100), &target).unwrap(), abs(1.00)); // 100x
            }

            #[test]
            fn min_step_count_2() {
                // Given
                let mut mode: Mode<TestTransformation> = Mode {
                    step_count_interval: create_discrete_increment_interval(4, 100),
                    ..Default::default()
                };
                let target = TestTarget {
                    current_value: Some(UnitValue::MAX),
                    control_type: ControlType::AbsoluteDiscrete {
                        atomic_step_size: UnitValue::new(0.05),
                    },
                };
                // When
                // Then
                assert_abs_diff_eq!(mode.control(rel(-10), &target).unwrap(), abs(0.35)); // 13x
                assert_abs_diff_eq!(mode.control(rel(-2), &target).unwrap(), abs(0.75)); // 5x
                assert_abs_diff_eq!(mode.control(rel(-1), &target).unwrap(), abs(0.8)); // 4x
                assert!(mode.control(rel(1), &target).is_none());
                assert!(mode.control(rel(2), &target).is_none());
                assert!(mode.control(rel(10), &target).is_none());
            }

            #[test]
            fn max_step_count_1() {
                // Given
                let mut mode: Mode<TestTransformation> = Mode {
                    step_count_interval: create_discrete_increment_interval(1, 2),
                    ..Default::default()
                };
                let target = TestTarget {
                    current_value: Some(UnitValue::MIN),
                    control_type: ControlType::AbsoluteDiscrete {
                        atomic_step_size: UnitValue::new(0.05),
                    },
                };
                // When
                // Then
                assert!(mode.control(rel(-10), &target).is_none());
                assert!(mode.control(rel(-2), &target).is_none());
                assert!(mode.control(rel(-1), &target).is_none());
                assert_abs_diff_eq!(mode.control(rel(1), &target).unwrap(), abs(0.05));
                assert_abs_diff_eq!(mode.control(rel(2), &target).unwrap(), abs(0.10));
                assert_abs_diff_eq!(mode.control(rel(10), &target).unwrap(), abs(0.10));
            }

            #[test]
            fn max_step_count_throttle() {
                // Given
                let mut mode: Mode<TestTransformation> = Mode {
                    step_count_interval: create_discrete_increment_interval(-2, -2),
                    ..Default::default()
                };
                let target = TestTarget {
                    current_value: Some(UnitValue::MIN),
                    control_type: ControlType::AbsoluteDiscrete {
                        atomic_step_size: UnitValue::new(0.05),
                    },
                };
                // When
                // Then
                // No effect because already min
                assert!(mode.control(rel(-10), &target).is_none());
                assert!(mode.control(rel(-10), &target).is_none());
                assert!(mode.control(rel(-10), &target).is_none());
                assert!(mode.control(rel(-10), &target).is_none());
                // Every 2nd time
                assert_abs_diff_eq!(mode.control(rel(1), &target).unwrap(), abs(0.05));
                assert!(mode.control(rel(1), &target).is_none());
                assert_abs_diff_eq!(mode.control(rel(1), &target).unwrap(), abs(0.05));
                assert!(mode.control(rel(2), &target).is_none());
                assert_abs_diff_eq!(mode.control(rel(2), &target).unwrap(), abs(0.05));
            }

            #[test]
            fn max_step_count_2() {
                // Given
                let mut mode: Mode<TestTransformation> = Mode {
                    step_count_interval: create_discrete_increment_interval(1, 2),
                    ..Default::default()
                };
                let target = TestTarget {
                    current_value: Some(UnitValue::MAX),
                    control_type: ControlType::AbsoluteDiscrete {
                        atomic_step_size: UnitValue::new(0.05),
                    },
                };
                // When
                // Then
                assert_abs_diff_eq!(mode.control(rel(-10), &target).unwrap(), abs(0.90));
                assert_abs_diff_eq!(mode.control(rel(-2), &target).unwrap(), abs(0.90));
                assert_abs_diff_eq!(mode.control(rel(-1), &target).unwrap(), abs(0.95));
                assert!(mode.control(rel(1), &target).is_none());
                assert!(mode.control(rel(2), &target).is_none());
                assert!(mode.control(rel(10), &target).is_none());
            }

            #[test]
            fn reverse() {
                // Given
                let mut mode: Mode<TestTransformation> = Mode {
                    reverse: true,
                    ..Default::default()
                };
                let target = TestTarget {
                    current_value: Some(UnitValue::MIN),
                    control_type: ControlType::AbsoluteDiscrete {
                        atomic_step_size: UnitValue::new(0.05),
                    },
                };
                // When
                // Then
                assert_abs_diff_eq!(mode.control(rel(-10), &target).unwrap(), abs(0.05));
                assert_abs_diff_eq!(mode.control(rel(-2), &target).unwrap(), abs(0.05));
                assert_abs_diff_eq!(mode.control(rel(-1), &target).unwrap(), abs(0.05));
                assert!(mode.control(rel(1), &target).is_none());
                assert!(mode.control(rel(2), &target).is_none());
                assert!(mode.control(rel(10), &target).is_none());
            }

            #[test]
            fn rotate_1() {
                // Given
                let mut mode: Mode<TestTransformation> = Mode {
                    rotate: true,
                    ..Default::default()
                };
                let target = TestTarget {
                    current_value: Some(UnitValue::MIN),
                    control_type: ControlType::AbsoluteDiscrete {
                        atomic_step_size: UnitValue::new(0.05),
                    },
                };
                // When
                // Then
                assert_abs_diff_eq!(mode.control(rel(-10), &target).unwrap(), abs(1.0));
                assert_abs_diff_eq!(mode.control(rel(-2), &target).unwrap(), abs(1.0));
                assert_abs_diff_eq!(mode.control(rel(-1), &target).unwrap(), abs(1.0));
                assert_abs_diff_eq!(mode.control(rel(1), &target).unwrap(), abs(0.05));
                assert_abs_diff_eq!(mode.control(rel(2), &target).unwrap(), abs(0.05));
                assert_abs_diff_eq!(mode.control(rel(10), &target).unwrap(), abs(0.05));
            }

            #[test]
            fn rotate_2() {
                // Given
                let mut mode: Mode<TestTransformation> = Mode {
                    rotate: true,
                    ..Default::default()
                };
                let target = TestTarget {
                    current_value: Some(UnitValue::MAX),
                    control_type: ControlType::AbsoluteDiscrete {
                        atomic_step_size: UnitValue::new(0.05),
                    },
                };
                // When
                // Then
                assert_abs_diff_eq!(mode.control(rel(-10), &target).unwrap(), abs(0.95));
                assert_abs_diff_eq!(mode.control(rel(-2), &target).unwrap(), abs(0.95));
                assert_abs_diff_eq!(mode.control(rel(-1), &target).unwrap(), abs(0.95));
                assert_abs_diff_eq!(mode.control(rel(1), &target).unwrap(), abs(0.0));
                assert_abs_diff_eq!(mode.control(rel(2), &target).unwrap(), abs(0.0));
                assert_abs_diff_eq!(mode.control(rel(10), &target).unwrap(), abs(0.0));
            }

            #[test]
            fn target_interval_min() {
                // Given
                let mut mode: Mode<TestTransformation> = Mode {
                    target_value_interval: create_unit_value_interval(0.2, 0.8),
                    ..Default::default()
                };
                let target = TestTarget {
                    current_value: Some(UnitValue::new(0.2)),
                    control_type: ControlType::AbsoluteDiscrete {
                        atomic_step_size: UnitValue::new(0.05),
                    },
                };
                // When
                // Then
                assert!(mode.control(rel(-10), &target).is_none());
                assert!(mode.control(rel(-2), &target).is_none());
                assert!(mode.control(rel(-1), &target).is_none());
                assert_abs_diff_eq!(mode.control(rel(1), &target).unwrap(), abs(0.25));
                assert_abs_diff_eq!(mode.control(rel(2), &target).unwrap(), abs(0.25));
                assert_abs_diff_eq!(mode.control(rel(10), &target).unwrap(), abs(0.25));
            }

            #[test]
            fn target_interval_max() {
                // Given
                let mut mode: Mode<TestTransformation> = Mode {
                    target_value_interval: create_unit_value_interval(0.2, 0.8),
                    ..Default::default()
                };
                let target = TestTarget {
                    current_value: Some(UnitValue::new(0.8)),
                    control_type: ControlType::AbsoluteDiscrete {
                        atomic_step_size: UnitValue::new(0.05),
                    },
                };
                // When
                // Then
                assert_abs_diff_eq!(mode.control(rel(-10), &target).unwrap(), abs(0.75));
                assert_abs_diff_eq!(mode.control(rel(-2), &target).unwrap(), abs(0.75));
                assert_abs_diff_eq!(mode.control(rel(-1), &target).unwrap(), abs(0.75));
                assert!(mode.control(rel(1), &target).is_none());
                assert!(mode.control(rel(2), &target).is_none());
                assert!(mode.control(rel(10), &target).is_none());
            }

            #[test]
            fn target_interval_current_target_value_out_of_range() {
                // Given
                let mut mode: Mode<TestTransformation> = Mode {
                    target_value_interval: create_unit_value_interval(0.2, 0.8),
                    ..Default::default()
                };
                let target = TestTarget {
                    current_value: Some(UnitValue::MIN),
                    control_type: ControlType::AbsoluteDiscrete {
                        atomic_step_size: UnitValue::new(0.05),
                    },
                };
                // When
                // Then
                assert_abs_diff_eq!(mode.control(rel(-10), &target).unwrap(), abs(0.2));
                assert_abs_diff_eq!(mode.control(rel(-2), &target).unwrap(), abs(0.2));
                assert_abs_diff_eq!(mode.control(rel(-1), &target).unwrap(), abs(0.2));
                assert_abs_diff_eq!(mode.control(rel(1), &target).unwrap(), abs(0.2));
                assert_abs_diff_eq!(mode.control(rel(2), &target).unwrap(), abs(0.2));
                assert_abs_diff_eq!(mode.control(rel(10), &target).unwrap(), abs(0.2));
            }

            #[test]
            fn target_interval_step_interval_current_target_value_out_of_range() {
                // Given
                let mut mode: Mode<TestTransformation> = Mode {
                    step_count_interval: create_discrete_increment_interval(1, 100),
                    target_value_interval: create_unit_value_interval(0.2, 0.8),
                    ..Default::default()
                };
                let target = TestTarget {
                    current_value: Some(UnitValue::MIN),
                    control_type: ControlType::AbsoluteDiscrete {
                        atomic_step_size: UnitValue::new(0.05),
                    },
                };
                // When
                // Then
                assert_abs_diff_eq!(mode.control(rel(-10), &target).unwrap(), abs(0.2));
                assert_abs_diff_eq!(mode.control(rel(-2), &target).unwrap(), abs(0.2));
                assert_abs_diff_eq!(mode.control(rel(-1), &target).unwrap(), abs(0.2));
                assert_abs_diff_eq!(mode.control(rel(1), &target).unwrap(), abs(0.2));
                assert_abs_diff_eq!(mode.control(rel(2), &target).unwrap(), abs(0.2));
                assert_abs_diff_eq!(mode.control(rel(10), &target).unwrap(), abs(0.2));
            }

            #[test]
            fn target_interval_min_rotate() {
                // Given
                let mut mode: Mode<TestTransformation> = Mode {
                    target_value_interval: create_unit_value_interval(0.2, 0.8),
                    rotate: true,
                    ..Default::default()
                };
                let target = TestTarget {
                    current_value: Some(UnitValue::new(0.2)),
                    control_type: ControlType::AbsoluteDiscrete {
                        atomic_step_size: UnitValue::new(0.05),
                    },
                };
                // When
                // Then
                assert_abs_diff_eq!(mode.control(rel(-10), &target).unwrap(), abs(0.8));
                assert_abs_diff_eq!(mode.control(rel(-2), &target).unwrap(), abs(0.8));
                assert_abs_diff_eq!(mode.control(rel(-1), &target).unwrap(), abs(0.8));
                assert_abs_diff_eq!(mode.control(rel(1), &target).unwrap(), abs(0.25));
                assert_abs_diff_eq!(mode.control(rel(2), &target).unwrap(), abs(0.25));
                assert_abs_diff_eq!(mode.control(rel(10), &target).unwrap(), abs(0.25));
            }

            #[test]
            fn target_interval_max_rotate() {
                // Given
                let mut mode: Mode<TestTransformation> = Mode {
                    target_value_interval: create_unit_value_interval(0.2, 0.8),
                    rotate: true,
                    ..Default::default()
                };
                let target = TestTarget {
                    current_value: Some(UnitValue::new(0.8)),
                    control_type: ControlType::AbsoluteDiscrete {
                        atomic_step_size: UnitValue::new(0.05),
                    },
                };
                // When
                // Then
                assert_abs_diff_eq!(mode.control(rel(-10), &target).unwrap(), abs(0.75));
                assert_abs_diff_eq!(mode.control(rel(-2), &target).unwrap(), abs(0.75));
                assert_abs_diff_eq!(mode.control(rel(-1), &target).unwrap(), abs(0.75));
                assert_abs_diff_eq!(mode.control(rel(1), &target).unwrap(), abs(0.2));
                assert_abs_diff_eq!(mode.control(rel(2), &target).unwrap(), abs(0.2));
                assert_abs_diff_eq!(mode.control(rel(10), &target).unwrap(), abs(0.2));
            }

            #[test]
            fn target_interval_rotate_current_target_value_out_of_range() {
                // Given
                let mut mode: Mode<TestTransformation> = Mode {
                    target_value_interval: create_unit_value_interval(0.2, 0.8),
                    rotate: true,
                    ..Default::default()
                };
                let target = TestTarget {
                    current_value: Some(UnitValue::MIN),
                    control_type: ControlType::AbsoluteDiscrete {
                        atomic_step_size: UnitValue::new(0.05),
                    },
                };
                // When
                // Then
                assert_abs_diff_eq!(mode.control(rel(-10), &target).unwrap(), abs(0.8));
                assert_abs_diff_eq!(mode.control(rel(-2), &target).unwrap(), abs(0.8));
                assert_abs_diff_eq!(mode.control(rel(-1), &target).unwrap(), abs(0.8));
                assert_abs_diff_eq!(mode.control(rel(1), &target).unwrap(), abs(0.2));
                assert_abs_diff_eq!(mode.control(rel(2), &target).unwrap(), abs(0.2));
                assert_abs_diff_eq!(mode.control(rel(10), &target).unwrap(), abs(0.2));
            }
        }

        mod relative_target {
            use super::*;

            #[test]
            fn default() {
                // Given
                let mut mode: Mode<TestTransformation> = Mode {
                    ..Default::default()
                };
                let target = TestTarget {
                    current_value: Some(UnitValue::MIN),
                    control_type: ControlType::Relative,
                };
                // When
                // Then
                assert_eq!(mode.control(rel(-10), &target), Some(rel(-1)));
                assert_eq!(mode.control(rel(-2), &target), Some(rel(-1)));
                assert_eq!(mode.control(rel(-1), &target), Some(rel(-1)));
                assert_eq!(mode.control(rel(1), &target), Some(rel(1)));
                assert_eq!(mode.control(rel(2), &target), Some(rel(1)));
                assert_eq!(mode.control(rel(10), &target), Some(rel(1)));
            }

            #[test]
            fn min_step_count() {
                // Given
                let mut mode: Mode<TestTransformation> = Mode {
                    step_count_interval: create_discrete_increment_interval(2, 100),
                    ..Default::default()
                };
                let target = TestTarget {
                    current_value: Some(UnitValue::MIN),
                    control_type: ControlType::Relative,
                };
                // When
                // Then
                assert_eq!(mode.control(rel(-10), &target), Some(rel(-11)));
                assert_eq!(mode.control(rel(-2), &target), Some(rel(-3)));
                assert_eq!(mode.control(rel(-1), &target), Some(rel(-2)));
                assert_eq!(mode.control(rel(1), &target), Some(rel(2)));
                assert_eq!(mode.control(rel(2), &target), Some(rel(3)));
                assert_eq!(mode.control(rel(10), &target), Some(rel(11)));
            }

            #[test]
            fn min_step_count_throttle() {
                // Given
                let mut mode: Mode<TestTransformation> = Mode {
                    step_count_interval: create_discrete_increment_interval(-4, 100),
                    ..Default::default()
                };
                let target = TestTarget {
                    current_value: Some(UnitValue::MIN),
                    control_type: ControlType::Relative,
                };
                // When
                // Then
                // So intense that reaching speedup area
                assert_eq!(mode.control(rel(-10), &target), Some(rel(-6)));
                // Every 3rd time
                assert_eq!(mode.control(rel(-2), &target), Some(rel(-1)));
                assert_eq!(mode.control(rel(-2), &target), None);
                assert_eq!(mode.control(rel(-2), &target), None);
                assert_eq!(mode.control(rel(-2), &target), Some(rel(-1)));
                // Every 4th time (but fired before)
                assert_eq!(mode.control(rel(-1), &target), None);
                assert_eq!(mode.control(rel(-1), &target), None);
                assert_eq!(mode.control(rel(-1), &target), None);
                assert_eq!(mode.control(rel(-1), &target), Some(rel(-1)));
                // Direction change
                assert_eq!(mode.control(rel(1), &target), Some(rel(1)));
                // Every 3rd time (but fired before)
                assert_eq!(mode.control(rel(2), &target), None);
                assert_eq!(mode.control(rel(2), &target), None);
                assert_eq!(mode.control(rel(2), &target), Some(rel(1)));
                // So intense that reaching speedup area
                assert_eq!(mode.control(rel(10), &target), Some(rel(6)));
            }

            #[test]
            fn max_step_count() {
                // Given
                let mut mode: Mode<TestTransformation> = Mode {
                    step_count_interval: create_discrete_increment_interval(1, 2),
                    ..Default::default()
                };
                let target = TestTarget {
                    current_value: Some(UnitValue::MIN),
                    control_type: ControlType::Relative,
                };
                // When
                // Then
                assert_eq!(mode.control(rel(-10), &target), Some(rel(-2)));
                assert_eq!(mode.control(rel(-2), &target), Some(rel(-2)));
                assert_eq!(mode.control(rel(-1), &target), Some(rel(-1)));
                assert_eq!(mode.control(rel(1), &target), Some(rel(1)));
                assert_eq!(mode.control(rel(2), &target), Some(rel(2)));
                assert_eq!(mode.control(rel(10), &target), Some(rel(2)));
            }

            #[test]
            fn max_step_count_throttle() {
                // Given
                let mut mode: Mode<TestTransformation> = Mode {
                    step_count_interval: create_discrete_increment_interval(-10, -4),
                    ..Default::default()
                };
                let target = TestTarget {
                    current_value: Some(UnitValue::MIN),
                    control_type: ControlType::Relative,
                };
                // When
                // Then
                // Every 4th time
                assert_eq!(mode.control(rel(-10), &target), Some(rel(-1)));
                assert_eq!(mode.control(rel(-10), &target), None);
                assert_eq!(mode.control(rel(-10), &target), None);
                assert_eq!(mode.control(rel(-10), &target), None);
                assert_eq!(mode.control(rel(-10), &target), Some(rel(-1)));
                assert_eq!(mode.control(rel(-10), &target), None);
                assert_eq!(mode.control(rel(-10), &target), None);
                assert_eq!(mode.control(rel(-10), &target), None);
                // Every 10th time
                assert_eq!(mode.control(rel(1), &target), Some(rel(1)));
                assert_eq!(mode.control(rel(1), &target), None);
                assert_eq!(mode.control(rel(1), &target), None);
                assert_eq!(mode.control(rel(1), &target), None);
                assert_eq!(mode.control(rel(1), &target), None);
                assert_eq!(mode.control(rel(1), &target), None);
                assert_eq!(mode.control(rel(1), &target), None);
                assert_eq!(mode.control(rel(1), &target), None);
                assert_eq!(mode.control(rel(1), &target), None);
                assert_eq!(mode.control(rel(1), &target), None);
                assert_eq!(mode.control(rel(1), &target), Some(rel(1)));
            }

            #[test]
            fn reverse() {
                // Given
                let mut mode: Mode<TestTransformation> = Mode {
                    reverse: true,
                    ..Default::default()
                };
                let target = TestTarget {
                    current_value: Some(UnitValue::MIN),
                    control_type: ControlType::Relative,
                };
                // When
                // Then
                assert_eq!(mode.control(rel(-10), &target), Some(rel(1)));
                assert_eq!(mode.control(rel(-2), &target), Some(rel(1)));
                assert_eq!(mode.control(rel(-1), &target), Some(rel(1)));
                assert_eq!(mode.control(rel(1), &target), Some(rel(-1)));
                assert_eq!(mode.control(rel(2), &target), Some(rel(-1)));
                assert_eq!(mode.control(rel(10), &target), Some(rel(-1)));
            }
        }
    }

    mod absolute_to_relative {
        use super::*;

        mod absolute_continuous_target {
            use super::*;

            #[test]
            fn default_1() {
                // Given
                let mut mode: Mode<TestTransformation> = Mode {
                    absolute_mode: AbsoluteMode::IncrementalButtons,
                    ..Default::default()
                };
                let target = TestTarget {
                    current_value: Some(UnitValue::MIN),
                    control_type: ControlType::AbsoluteContinuous,
                };
                // When
                // Then
                assert!(mode.control(abs(0.0), &target).is_none());
                assert_abs_diff_eq!(mode.control(abs(0.5), &target).unwrap(), abs(0.01));
                assert_abs_diff_eq!(mode.control(abs(1.0), &target).unwrap(), abs(0.01));
            }

            #[test]
            fn default_2() {
                // Given
                let mut mode: Mode<TestTransformation> = Mode {
                    absolute_mode: AbsoluteMode::IncrementalButtons,
                    ..Default::default()
                };
                let target = TestTarget {
                    current_value: Some(UnitValue::MAX),
                    control_type: ControlType::AbsoluteContinuous,
                };
                // When
                // Then
                assert!(mode.control(abs(0.0), &target).is_none());
                assert!(mode.control(abs(0.5), &target).is_none());
                assert!(mode.control(abs(1.0), &target).is_none());
            }

            #[test]
            fn min_step_size_1() {
                // Given
                let mut mode: Mode<TestTransformation> = Mode {
                    absolute_mode: AbsoluteMode::IncrementalButtons,
                    step_size_interval: create_unit_value_interval(0.2, 1.0),
                    ..Default::default()
                };
                let target = TestTarget {
                    current_value: Some(UnitValue::MIN),
                    control_type: ControlType::AbsoluteContinuous,
                };
                // When
                // Then
                assert!(mode.control(abs(0.0), &target).is_none());
                assert_abs_diff_eq!(mode.control(abs(0.1), &target).unwrap(), abs(0.28));
                assert_abs_diff_eq!(mode.control(abs(0.5), &target).unwrap(), abs(0.6));
                assert_abs_diff_eq!(mode.control(abs(1.0), &target).unwrap(), abs(1.0));
            }

            #[test]
            fn min_step_size_2() {
                // Given
                let mut mode: Mode<TestTransformation> = Mode {
                    absolute_mode: AbsoluteMode::IncrementalButtons,
                    step_size_interval: create_unit_value_interval(0.2, 1.0),
                    ..Default::default()
                };
                let target = TestTarget {
                    current_value: Some(UnitValue::MAX),
                    control_type: ControlType::AbsoluteContinuous,
                };
                // When
                // Then
                assert!(mode.control(abs(0.0), &target).is_none());
                assert!(mode.control(abs(0.5), &target).is_none());
                assert!(mode.control(abs(1.0), &target).is_none());
            }

            #[test]
            fn max_step_size_1() {
                // Given
                let mut mode: Mode<TestTransformation> = Mode {
                    absolute_mode: AbsoluteMode::IncrementalButtons,
                    step_size_interval: create_unit_value_interval(0.01, 0.09),
                    ..Default::default()
                };
                let target = TestTarget {
                    current_value: Some(UnitValue::MIN),
                    control_type: ControlType::AbsoluteContinuous,
                };
                // When
                // Then
                assert!(mode.control(abs(0.0), &target).is_none());
                assert_abs_diff_eq!(mode.control(abs(0.1), &target).unwrap(), abs(0.018));
                assert_abs_diff_eq!(mode.control(abs(0.5), &target).unwrap(), abs(0.05));
                assert_abs_diff_eq!(mode.control(abs(0.75), &target).unwrap(), abs(0.07));
                assert_abs_diff_eq!(mode.control(abs(1.0), &target).unwrap(), abs(0.09));
            }

            #[test]
            fn max_step_size_2() {
                // Given
                let mut mode: Mode<TestTransformation> = Mode {
                    absolute_mode: AbsoluteMode::IncrementalButtons,
                    step_size_interval: create_unit_value_interval(0.01, 0.09),
                    ..Default::default()
                };
                let target = TestTarget {
                    current_value: Some(UnitValue::MAX),
                    control_type: ControlType::AbsoluteContinuous,
                };
                // When
                // Then
                assert!(mode.control(abs(0.0), &target).is_none());
                assert!(mode.control(abs(0.5), &target).is_none());
                assert!(mode.control(abs(1.0), &target).is_none());
            }

            #[test]
            fn source_interval() {
                // Given
                let mut mode: Mode<TestTransformation> = Mode {
                    absolute_mode: AbsoluteMode::IncrementalButtons,
                    source_value_interval: create_unit_value_interval(0.5, 1.0),
                    ..Default::default()
                };
                let target = TestTarget {
                    current_value: Some(UnitValue::MIN),
                    control_type: ControlType::AbsoluteContinuous,
                };
                // When
                // Then
                assert!(mode.control(abs(0.0), &target).is_none());
                assert!(mode.control(abs(0.25), &target).is_none());
                assert_abs_diff_eq!(mode.control(abs(0.5), &target).unwrap(), abs(0.01));
                assert_abs_diff_eq!(mode.control(abs(0.75), &target).unwrap(), abs(0.01));
                assert_abs_diff_eq!(mode.control(abs(1.0), &target).unwrap(), abs(0.01));
            }

            #[test]
            fn source_interval_step_interval() {
                // Given
                let mut mode: Mode<TestTransformation> = Mode {
                    absolute_mode: AbsoluteMode::IncrementalButtons,
                    source_value_interval: create_unit_value_interval(0.5, 1.0),
                    step_size_interval: create_unit_value_interval(0.5, 1.0),
                    ..Default::default()
                };
                let target = TestTarget {
                    current_value: Some(UnitValue::MIN),
                    control_type: ControlType::AbsoluteContinuous,
                };
                // When
                // Then
                assert!(mode.control(abs(0.0), &target).is_none());
                assert!(mode.control(abs(0.25), &target).is_none());
                assert_abs_diff_eq!(mode.control(abs(0.5), &target).unwrap(), abs(0.5));
                assert_abs_diff_eq!(mode.control(abs(0.75), &target).unwrap(), abs(0.75));
                assert_abs_diff_eq!(mode.control(abs(1.0), &target).unwrap(), abs(1.0));
            }

            #[test]
            fn reverse_1() {
                // Given
                let mut mode: Mode<TestTransformation> = Mode {
                    absolute_mode: AbsoluteMode::IncrementalButtons,
                    reverse: true,
                    ..Default::default()
                };
                let target = TestTarget {
                    current_value: Some(UnitValue::MIN),
                    control_type: ControlType::AbsoluteContinuous,
                };
                // When
                // Then
                assert!(mode.control(abs(0.0), &target).is_none());
                assert!(mode.control(abs(0.5), &target).is_none());
                assert!(mode.control(abs(1.0), &target).is_none());
            }

            #[test]
            fn reverse_2() {
                // Given
                let mut mode: Mode<TestTransformation> = Mode {
                    absolute_mode: AbsoluteMode::IncrementalButtons,
                    reverse: true,
                    ..Default::default()
                };
                let target = TestTarget {
                    current_value: Some(UnitValue::MAX),
                    control_type: ControlType::AbsoluteContinuous,
                };
                // When
                // Then
                assert!(mode.control(abs(0.0), &target).is_none());
                assert_abs_diff_eq!(mode.control(abs(0.1), &target).unwrap(), abs(0.99));
                assert_abs_diff_eq!(mode.control(abs(0.5), &target).unwrap(), abs(0.99));
                assert_abs_diff_eq!(mode.control(abs(1.0), &target).unwrap(), abs(0.99));
            }

            #[test]
            fn rotate_1() {
                // Given
                let mut mode: Mode<TestTransformation> = Mode {
                    absolute_mode: AbsoluteMode::IncrementalButtons,
                    rotate: true,
                    ..Default::default()
                };
                let target = TestTarget {
                    current_value: Some(UnitValue::MIN),
                    control_type: ControlType::AbsoluteContinuous,
                };
                // When
                // Then
                assert!(mode.control(abs(0.0), &target).is_none());
                assert_abs_diff_eq!(mode.control(abs(0.1), &target).unwrap(), abs(0.01));
                assert_abs_diff_eq!(mode.control(abs(0.5), &target).unwrap(), abs(0.01));
                assert_abs_diff_eq!(mode.control(abs(1.0), &target).unwrap(), abs(0.01));
            }

            #[test]
            fn rotate_2() {
                // Given
                let mut mode: Mode<TestTransformation> = Mode {
                    absolute_mode: AbsoluteMode::IncrementalButtons,
                    rotate: true,
                    ..Default::default()
                };
                let target = TestTarget {
                    current_value: Some(UnitValue::MAX),
                    control_type: ControlType::AbsoluteContinuous,
                };
                // When
                // Then
                assert!(mode.control(abs(0.0), &target).is_none());
                assert_abs_diff_eq!(mode.control(abs(0.1), &target).unwrap(), abs(0.0));
                assert_abs_diff_eq!(mode.control(abs(0.5), &target).unwrap(), abs(0.0));
                assert_abs_diff_eq!(mode.control(abs(1.0), &target).unwrap(), abs(0.0));
            }

            #[test]
            fn target_interval_min() {
                // Given
                let mut mode: Mode<TestTransformation> = Mode {
                    absolute_mode: AbsoluteMode::IncrementalButtons,
                    target_value_interval: create_unit_value_interval(0.2, 0.8),
                    ..Default::default()
                };
                let target = TestTarget {
                    current_value: Some(UnitValue::new(0.2)),
                    control_type: ControlType::AbsoluteContinuous,
                };
                // When
                // Then
                assert!(mode.control(abs(0.0), &target).is_none());
                assert_abs_diff_eq!(mode.control(abs(0.1), &target).unwrap(), abs(0.21));
                assert_abs_diff_eq!(mode.control(abs(0.5), &target).unwrap(), abs(0.21));
                assert_abs_diff_eq!(mode.control(abs(1.0), &target).unwrap(), abs(0.21));
            }

            #[test]
            fn target_interval_max() {
                // Given
                let mut mode: Mode<TestTransformation> = Mode {
                    absolute_mode: AbsoluteMode::IncrementalButtons,
                    target_value_interval: create_unit_value_interval(0.2, 0.8),
                    ..Default::default()
                };
                let target = TestTarget {
                    current_value: Some(UnitValue::new(0.8)),
                    control_type: ControlType::AbsoluteContinuous,
                };
                // When
                // Then
                assert!(mode.control(abs(0.0), &target).is_none());
                assert!(mode.control(abs(0.1), &target).is_none());
                assert!(mode.control(abs(0.5), &target).is_none());
                assert!(mode.control(abs(1.0), &target).is_none());
            }

            #[test]
            fn target_interval_current_target_value_out_of_range() {
                // Given
                let mut mode: Mode<TestTransformation> = Mode {
                    absolute_mode: AbsoluteMode::IncrementalButtons,
                    target_value_interval: create_unit_value_interval(0.2, 0.8),
                    ..Default::default()
                };
                let target = TestTarget {
                    current_value: Some(UnitValue::MIN),
                    control_type: ControlType::AbsoluteContinuous,
                };
                // When
                // Then
                assert!(mode.control(abs(0.0), &target).is_none());
                assert_abs_diff_eq!(mode.control(abs(0.1), &target).unwrap(), abs(0.2));
                assert_abs_diff_eq!(mode.control(abs(0.5), &target).unwrap(), abs(0.2));
                assert_abs_diff_eq!(mode.control(abs(1.0), &target).unwrap(), abs(0.2));
            }

            #[test]
            fn target_interval_min_rotate() {
                // Given
                let mut mode: Mode<TestTransformation> = Mode {
                    absolute_mode: AbsoluteMode::IncrementalButtons,
                    target_value_interval: create_unit_value_interval(0.2, 0.8),
                    rotate: true,
                    ..Default::default()
                };
                let target = TestTarget {
                    current_value: Some(UnitValue::new(0.2)),
                    control_type: ControlType::AbsoluteContinuous,
                };
                // When
                // Then
                assert!(mode.control(abs(0.0), &target).is_none());
                assert_abs_diff_eq!(mode.control(abs(0.1), &target).unwrap(), abs(0.21));
                assert_abs_diff_eq!(mode.control(abs(0.5), &target).unwrap(), abs(0.21));
                assert_abs_diff_eq!(mode.control(abs(1.0), &target).unwrap(), abs(0.21));
            }

            #[test]
            fn target_interval_max_rotate() {
                // Given
                let mut mode: Mode<TestTransformation> = Mode {
                    absolute_mode: AbsoluteMode::IncrementalButtons,
                    target_value_interval: create_unit_value_interval(0.2, 0.8),
                    rotate: true,
                    ..Default::default()
                };
                let target = TestTarget {
                    current_value: Some(UnitValue::new(0.8)),
                    control_type: ControlType::AbsoluteContinuous,
                };
                // When
                // Then
                assert!(mode.control(abs(0.0), &target).is_none());
                assert_abs_diff_eq!(mode.control(abs(0.1), &target).unwrap(), abs(0.2));
                assert_abs_diff_eq!(mode.control(abs(0.5), &target).unwrap(), abs(0.2));
                assert_abs_diff_eq!(mode.control(abs(1.0), &target).unwrap(), abs(0.2));
            }

            #[test]
            fn target_interval_rotate_current_target_value_out_of_range() {
                // Given
                let mut mode: Mode<TestTransformation> = Mode {
                    absolute_mode: AbsoluteMode::IncrementalButtons,
                    target_value_interval: create_unit_value_interval(0.2, 0.8),
                    rotate: true,
                    ..Default::default()
                };
                let target = TestTarget {
                    current_value: Some(UnitValue::MIN),
                    control_type: ControlType::AbsoluteContinuous,
                };
                // When
                // Then
                assert!(mode.control(abs(0.0), &target).is_none());
                assert_abs_diff_eq!(mode.control(abs(0.1), &target).unwrap(), abs(0.2));
                assert_abs_diff_eq!(mode.control(abs(0.5), &target).unwrap(), abs(0.2));
                assert_abs_diff_eq!(mode.control(abs(1.0), &target).unwrap(), abs(0.2));
            }

            #[test]
            fn target_interval_rotate_reverse_current_target_value_out_of_range() {
                // Given
                let mut mode: Mode<TestTransformation> = Mode {
                    absolute_mode: AbsoluteMode::IncrementalButtons,
                    target_value_interval: create_unit_value_interval(0.2, 0.8),
                    reverse: true,
                    rotate: true,
                    ..Default::default()
                };
                let target = TestTarget {
                    current_value: Some(UnitValue::MIN),
                    control_type: ControlType::AbsoluteContinuous,
                };
                // When
                // Then
                assert!(mode.control(abs(0.0), &target).is_none());
                assert_abs_diff_eq!(mode.control(abs(0.1), &target).unwrap(), abs(0.8));
                assert_abs_diff_eq!(mode.control(abs(0.5), &target).unwrap(), abs(0.8));
                assert_abs_diff_eq!(mode.control(abs(1.0), &target).unwrap(), abs(0.8));
            }
        }

        mod absolute_discrete_target {
            use super::*;

            #[test]
            fn default_1() {
                // Given
                let mut mode: Mode<TestTransformation> = Mode {
                    absolute_mode: AbsoluteMode::IncrementalButtons,
                    ..Default::default()
                };
                let target = TestTarget {
                    current_value: Some(UnitValue::MIN),
                    control_type: ControlType::AbsoluteDiscrete {
                        atomic_step_size: UnitValue::new(0.05),
                    },
                };
                // When
                // Then
                assert!(mode.control(abs(0.0), &target).is_none());
                assert_abs_diff_eq!(mode.control(abs(0.1), &target).unwrap(), abs(0.05));
                assert_abs_diff_eq!(mode.control(abs(0.5), &target).unwrap(), abs(0.05));
                assert_abs_diff_eq!(mode.control(abs(1.0), &target).unwrap(), abs(0.05));
            }

            #[test]
            fn default_2() {
                // Given
                let mut mode: Mode<TestTransformation> = Mode {
                    absolute_mode: AbsoluteMode::IncrementalButtons,
                    ..Default::default()
                };
                let target = TestTarget {
                    current_value: Some(UnitValue::MAX),
                    control_type: ControlType::AbsoluteDiscrete {
                        atomic_step_size: UnitValue::new(0.05),
                    },
                };
                // When
                // Then
                assert!(mode.control(abs(0.0), &target).is_none());
                assert!(mode.control(abs(0.1), &target).is_none());
                assert!(mode.control(abs(0.5), &target).is_none());
                assert!(mode.control(abs(1.0), &target).is_none());
            }

            #[test]
            fn min_step_count_1() {
                // Given
                let mut mode: Mode<TestTransformation> = Mode {
                    absolute_mode: AbsoluteMode::IncrementalButtons,
                    step_count_interval: create_discrete_increment_interval(4, 8),
                    ..Default::default()
                };
                let target = TestTarget {
                    current_value: Some(UnitValue::MIN),
                    control_type: ControlType::AbsoluteDiscrete {
                        atomic_step_size: UnitValue::new(0.05),
                    },
                };
                // When
                // Then
                assert!(mode.control(abs(0.0), &target).is_none());
                assert_abs_diff_eq!(mode.control(abs(0.1), &target).unwrap(), abs(0.2));
                assert_abs_diff_eq!(mode.control(abs(0.5), &target).unwrap(), abs(0.3));
                assert_abs_diff_eq!(mode.control(abs(1.0), &target).unwrap(), abs(0.4));
            }

            #[test]
            fn min_step_count_throttle() {
                // Given
                let mut mode: Mode<TestTransformation> = Mode {
                    absolute_mode: AbsoluteMode::IncrementalButtons,
                    step_count_interval: create_discrete_increment_interval(-4, -4),
                    ..Default::default()
                };
                let target = TestTarget {
                    current_value: Some(UnitValue::MIN),
                    control_type: ControlType::AbsoluteDiscrete {
                        atomic_step_size: UnitValue::new(0.05),
                    },
                };
                // When
                // Then
                assert!(mode.control(abs(0.0), &target).is_none());
                // Every 4th time
                assert_abs_diff_eq!(mode.control(abs(0.1), &target).unwrap(), abs(0.05));
                assert!(mode.control(abs(0.1), &target).is_none());
                assert!(mode.control(abs(0.1), &target).is_none());
                assert!(mode.control(abs(0.1), &target).is_none());
                assert_abs_diff_eq!(mode.control(abs(0.1), &target).unwrap(), abs(0.05));
            }

            #[test]
            fn min_step_count_2() {
                // Given
                let mut mode: Mode<TestTransformation> = Mode {
                    absolute_mode: AbsoluteMode::IncrementalButtons,
                    step_count_interval: create_discrete_increment_interval(4, 8),
                    ..Default::default()
                };
                let target = TestTarget {
                    current_value: Some(UnitValue::MAX),
                    control_type: ControlType::AbsoluteDiscrete {
                        atomic_step_size: UnitValue::new(0.05),
                    },
                };
                // When
                // Then
                assert!(mode.control(abs(0.0), &target).is_none());
                assert!(mode.control(abs(0.1), &target).is_none());
                assert!(mode.control(abs(0.5), &target).is_none());
                assert!(mode.control(abs(1.0), &target).is_none());
            }

            #[test]
            fn max_step_count_1() {
                // Given
                let mut mode: Mode<TestTransformation> = Mode {
                    absolute_mode: AbsoluteMode::IncrementalButtons,
                    step_count_interval: create_discrete_increment_interval(1, 8),
                    ..Default::default()
                };
                let target = TestTarget {
                    current_value: Some(UnitValue::MIN),
                    control_type: ControlType::AbsoluteDiscrete {
                        atomic_step_size: UnitValue::new(0.05),
                    },
                };
                // When
                // Then
                assert!(mode.control(abs(0.0), &target).is_none());
                assert_abs_diff_eq!(mode.control(abs(0.1), &target).unwrap(), abs(0.1));
                assert_abs_diff_eq!(mode.control(abs(0.5), &target).unwrap(), abs(0.25));
                assert_abs_diff_eq!(mode.control(abs(1.0), &target).unwrap(), abs(0.4));
            }

            #[test]
            fn max_step_count_2() {
                // Given
                let mut mode: Mode<TestTransformation> = Mode {
                    absolute_mode: AbsoluteMode::IncrementalButtons,
                    step_count_interval: create_discrete_increment_interval(1, 2),
                    ..Default::default()
                };
                let target = TestTarget {
                    current_value: Some(UnitValue::MAX),
                    control_type: ControlType::AbsoluteDiscrete {
                        atomic_step_size: UnitValue::new(0.05),
                    },
                };
                // When
                // Then
                assert_abs_diff_eq!(mode.control(rel(-10), &target).unwrap(), abs(0.90));
                assert_abs_diff_eq!(mode.control(rel(-2), &target).unwrap(), abs(0.90));
                assert_abs_diff_eq!(mode.control(rel(-1), &target).unwrap(), abs(0.95));
                assert!(mode.control(rel(1), &target).is_none());
                assert!(mode.control(rel(2), &target).is_none());
                assert!(mode.control(rel(10), &target).is_none());
            }

            #[test]
            fn source_interval() {
                // Given
                let mut mode: Mode<TestTransformation> = Mode {
                    absolute_mode: AbsoluteMode::IncrementalButtons,
                    source_value_interval: create_unit_value_interval(0.5, 1.0),
                    ..Default::default()
                };
                let target = TestTarget {
                    current_value: Some(UnitValue::MIN),
                    control_type: ControlType::AbsoluteDiscrete {
                        atomic_step_size: UnitValue::new(0.05),
                    },
                };
                // When
                // Then
                assert!(mode.control(abs(0.0), &target).is_none());
                assert!(mode.control(abs(0.25), &target).is_none());
                assert_abs_diff_eq!(mode.control(abs(0.5), &target).unwrap(), abs(0.05));
                assert_abs_diff_eq!(mode.control(abs(0.75), &target).unwrap(), abs(0.05));
                assert_abs_diff_eq!(mode.control(abs(1.0), &target).unwrap(), abs(0.05));
            }

            #[test]
            fn source_interval_step_interval() {
                // Given
                let mut mode: Mode<TestTransformation> = Mode {
                    absolute_mode: AbsoluteMode::IncrementalButtons,
                    source_value_interval: create_unit_value_interval(0.5, 1.0),
                    step_count_interval: create_discrete_increment_interval(4, 8),
                    ..Default::default()
                };
                let target = TestTarget {
                    current_value: Some(UnitValue::MIN),
                    control_type: ControlType::AbsoluteDiscrete {
                        atomic_step_size: UnitValue::new(0.05),
                    },
                };
                // When
                // Then
                assert!(mode.control(abs(0.0), &target).is_none());
                assert!(mode.control(abs(0.25), &target).is_none());
                assert_abs_diff_eq!(mode.control(abs(0.5), &target).unwrap(), abs(0.2));
                assert_abs_diff_eq!(mode.control(abs(0.75), &target).unwrap(), abs(0.3));
                assert_abs_diff_eq!(mode.control(abs(1.0), &target).unwrap(), abs(0.4));
            }

            #[test]
            fn reverse() {
                // Given
                let mut mode: Mode<TestTransformation> = Mode {
                    absolute_mode: AbsoluteMode::IncrementalButtons,
                    reverse: true,
                    ..Default::default()
                };
                let target = TestTarget {
                    current_value: Some(UnitValue::MIN),
                    control_type: ControlType::AbsoluteDiscrete {
                        atomic_step_size: UnitValue::new(0.05),
                    },
                };
                // When
                // Then
                assert!(mode.control(abs(0.0), &target).is_none());
                assert!(mode.control(abs(0.1), &target).is_none());
                assert!(mode.control(abs(0.5), &target).is_none());
                assert!(mode.control(abs(1.0), &target).is_none());
            }

            #[test]
            fn rotate_1() {
                // Given
                let mut mode: Mode<TestTransformation> = Mode {
                    absolute_mode: AbsoluteMode::IncrementalButtons,
                    rotate: true,
                    ..Default::default()
                };
                let target = TestTarget {
                    current_value: Some(UnitValue::MIN),
                    control_type: ControlType::AbsoluteDiscrete {
                        atomic_step_size: UnitValue::new(0.05),
                    },
                };
                // When
                // Then
                assert!(mode.control(abs(0.0), &target).is_none());
                assert_abs_diff_eq!(mode.control(abs(0.1), &target).unwrap(), abs(0.05));
                assert_abs_diff_eq!(mode.control(abs(0.5), &target).unwrap(), abs(0.05));
                assert_abs_diff_eq!(mode.control(abs(1.0), &target).unwrap(), abs(0.05));
            }

            #[test]
            fn rotate_2() {
                // Given
                let mut mode: Mode<TestTransformation> = Mode {
                    absolute_mode: AbsoluteMode::IncrementalButtons,
                    rotate: true,
                    ..Default::default()
                };
                let target = TestTarget {
                    current_value: Some(UnitValue::MAX),
                    control_type: ControlType::AbsoluteDiscrete {
                        atomic_step_size: UnitValue::new(0.05),
                    },
                };
                // When
                // Then
                assert!(mode.control(abs(0.0), &target).is_none());
                assert_abs_diff_eq!(mode.control(abs(0.1), &target).unwrap(), abs(0.0));
                assert_abs_diff_eq!(mode.control(abs(0.5), &target).unwrap(), abs(0.0));
                assert_abs_diff_eq!(mode.control(abs(1.0), &target).unwrap(), abs(0.0));
            }

            #[test]
            fn target_interval_min() {
                // Given
                let mut mode: Mode<TestTransformation> = Mode {
                    absolute_mode: AbsoluteMode::IncrementalButtons,
                    target_value_interval: create_unit_value_interval(0.2, 0.8),
                    ..Default::default()
                };
                let target = TestTarget {
                    current_value: Some(UnitValue::new(0.2)),
                    control_type: ControlType::AbsoluteDiscrete {
                        atomic_step_size: UnitValue::new(0.05),
                    },
                };
                // When
                // Then
                assert!(mode.control(abs(0.0), &target).is_none());
                assert_abs_diff_eq!(mode.control(abs(0.1), &target).unwrap(), abs(0.25));
                assert_abs_diff_eq!(mode.control(abs(0.5), &target).unwrap(), abs(0.25));
                assert_abs_diff_eq!(mode.control(abs(1.0), &target).unwrap(), abs(0.25));
            }

            #[test]
            fn target_interval_max() {
                // Given
                let mut mode: Mode<TestTransformation> = Mode {
                    absolute_mode: AbsoluteMode::IncrementalButtons,
                    target_value_interval: create_unit_value_interval(0.2, 0.8),
                    ..Default::default()
                };
                let target = TestTarget {
                    current_value: Some(UnitValue::new(0.8)),
                    control_type: ControlType::AbsoluteDiscrete {
                        atomic_step_size: UnitValue::new(0.05),
                    },
                };
                // When
                // Then
                assert!(mode.control(abs(0.0), &target).is_none());
                assert!(mode.control(abs(0.1), &target).is_none());
                assert!(mode.control(abs(0.5), &target).is_none());
                assert!(mode.control(abs(1.0), &target).is_none());
            }

            #[test]
            fn target_interval_current_target_value_out_of_range() {
                // Given
                let mut mode: Mode<TestTransformation> = Mode {
                    absolute_mode: AbsoluteMode::IncrementalButtons,
                    target_value_interval: create_unit_value_interval(0.2, 0.8),
                    ..Default::default()
                };
                let target = TestTarget {
                    current_value: Some(UnitValue::MIN),
                    control_type: ControlType::AbsoluteDiscrete {
                        atomic_step_size: UnitValue::new(0.05),
                    },
                };
                // When
                // Then
                assert!(mode.control(abs(0.0), &target).is_none());
                assert_abs_diff_eq!(mode.control(abs(0.1), &target).unwrap(), abs(0.2));
                assert_abs_diff_eq!(mode.control(abs(0.5), &target).unwrap(), abs(0.2));
                assert_abs_diff_eq!(mode.control(abs(1.0), &target).unwrap(), abs(0.2));
            }

            #[test]
            fn step_count_interval_exceeded() {
                // Given
                let mut mode: Mode<TestTransformation> = Mode {
                    absolute_mode: AbsoluteMode::IncrementalButtons,
                    step_count_interval: create_discrete_increment_interval(1, 100),
                    ..Default::default()
                };
                let target = TestTarget {
                    current_value: Some(UnitValue::MIN),
                    control_type: ControlType::AbsoluteDiscrete {
                        atomic_step_size: UnitValue::new(0.05),
                    },
                };
                // When
                // Then
                assert!(mode.control(abs(0.0), &target).is_none());
                assert_abs_diff_eq!(mode.control(abs(0.1), &target).unwrap(), abs(0.55));
                assert_abs_diff_eq!(mode.control(abs(0.5), &target).unwrap(), abs(1.0));
                assert_abs_diff_eq!(mode.control(abs(1.0), &target).unwrap(), abs(1.0));
            }

            #[test]
            fn target_interval_step_interval_current_target_value_out_of_range() {
                // Given
                let mut mode: Mode<TestTransformation> = Mode {
                    absolute_mode: AbsoluteMode::IncrementalButtons,
                    step_count_interval: create_discrete_increment_interval(1, 100),
                    target_value_interval: create_unit_value_interval(0.2, 0.8),
                    ..Default::default()
                };
                let target = TestTarget {
                    current_value: Some(UnitValue::MIN),
                    control_type: ControlType::AbsoluteDiscrete {
                        atomic_step_size: UnitValue::new(0.05),
                    },
                };
                // When
                // Then
                assert!(mode.control(abs(0.0), &target).is_none());
                assert_abs_diff_eq!(mode.control(abs(0.1), &target).unwrap(), abs(0.2));
                assert_abs_diff_eq!(mode.control(abs(0.5), &target).unwrap(), abs(0.2));
                assert_abs_diff_eq!(mode.control(abs(1.0), &target).unwrap(), abs(0.2));
            }

            #[test]
            fn target_interval_min_rotate() {
                // Given
                let mut mode: Mode<TestTransformation> = Mode {
                    absolute_mode: AbsoluteMode::IncrementalButtons,
                    target_value_interval: create_unit_value_interval(0.2, 0.8),
                    rotate: true,
                    ..Default::default()
                };
                let target = TestTarget {
                    current_value: Some(UnitValue::new(0.2)),
                    control_type: ControlType::AbsoluteDiscrete {
                        atomic_step_size: UnitValue::new(0.05),
                    },
                };
                // When
                // Then
                assert!(mode.control(abs(0.0), &target).is_none());
                assert_abs_diff_eq!(mode.control(abs(0.1), &target).unwrap(), abs(0.25));
                assert_abs_diff_eq!(mode.control(abs(0.5), &target).unwrap(), abs(0.25));
                assert_abs_diff_eq!(mode.control(abs(1.0), &target).unwrap(), abs(0.25));
            }

            #[test]
            fn target_interval_max_rotate() {
                // Given
                let mut mode: Mode<TestTransformation> = Mode {
                    absolute_mode: AbsoluteMode::IncrementalButtons,
                    target_value_interval: create_unit_value_interval(0.2, 0.8),
                    rotate: true,
                    ..Default::default()
                };
                let target = TestTarget {
                    current_value: Some(UnitValue::new(0.8)),
                    control_type: ControlType::AbsoluteDiscrete {
                        atomic_step_size: UnitValue::new(0.05),
                    },
                };
                // When
                // Then
                assert!(mode.control(abs(0.0), &target).is_none());
                assert_abs_diff_eq!(mode.control(abs(0.1), &target).unwrap(), abs(0.2));
                assert_abs_diff_eq!(mode.control(abs(0.5), &target).unwrap(), abs(0.2));
                assert_abs_diff_eq!(mode.control(abs(1.0), &target).unwrap(), abs(0.2));
            }

            #[test]
            fn target_interval_rotate_current_target_value_out_of_range() {
                // Given
                let mut mode: Mode<TestTransformation> = Mode {
                    absolute_mode: AbsoluteMode::IncrementalButtons,
                    target_value_interval: create_unit_value_interval(0.2, 0.8),
                    rotate: true,
                    ..Default::default()
                };
                let target = TestTarget {
                    current_value: Some(UnitValue::MIN),
                    control_type: ControlType::AbsoluteDiscrete {
                        atomic_step_size: UnitValue::new(0.05),
                    },
                };
                // When
                // Then
                assert!(mode.control(abs(0.0), &target).is_none());
                assert_abs_diff_eq!(mode.control(abs(0.1), &target).unwrap(), abs(0.2));
                assert_abs_diff_eq!(mode.control(abs(0.5), &target).unwrap(), abs(0.2));
                assert_abs_diff_eq!(mode.control(abs(1.0), &target).unwrap(), abs(0.2));
            }

            #[test]
            fn target_interval_rotate_reverse_current_target_value_out_of_range() {
                // Given
                let mut mode: Mode<TestTransformation> = Mode {
                    absolute_mode: AbsoluteMode::IncrementalButtons,
                    target_value_interval: create_unit_value_interval(0.2, 0.8),
                    reverse: true,
                    rotate: true,
                    ..Default::default()
                };
                let target = TestTarget {
                    current_value: Some(UnitValue::MIN),
                    control_type: ControlType::AbsoluteDiscrete {
                        atomic_step_size: UnitValue::new(0.05),
                    },
                };
                // When
                // Then
                assert!(mode.control(abs(0.0), &target).is_none());
                assert_abs_diff_eq!(mode.control(abs(0.1), &target).unwrap(), abs(0.8));
                assert_abs_diff_eq!(mode.control(abs(0.5), &target).unwrap(), abs(0.8));
                assert_abs_diff_eq!(mode.control(abs(1.0), &target).unwrap(), abs(0.8));
            }
        }

        mod relative_target {
            use super::*;

            #[test]
            fn default() {
                // Given
                let mut mode: Mode<TestTransformation> = Mode {
                    absolute_mode: AbsoluteMode::IncrementalButtons,
                    ..Default::default()
                };
                let target = TestTarget {
                    current_value: Some(UnitValue::MIN),
                    control_type: ControlType::Relative,
                };
                // When
                // Then
                assert!(mode.control(abs(0.0), &target).is_none());
                assert_abs_diff_eq!(mode.control(abs(0.1), &target).unwrap(), rel(1));
                assert_abs_diff_eq!(mode.control(abs(0.5), &target).unwrap(), rel(1));
                assert_abs_diff_eq!(mode.control(abs(1.0), &target).unwrap(), rel(1));
            }

            #[test]
            fn min_step_count() {
                // Given
                let mut mode: Mode<TestTransformation> = Mode {
                    absolute_mode: AbsoluteMode::IncrementalButtons,
                    step_count_interval: create_discrete_increment_interval(2, 8),
                    ..Default::default()
                };
                let target = TestTarget {
                    current_value: Some(UnitValue::MIN),
                    control_type: ControlType::Relative,
                };
                // When
                // Then
                assert!(mode.control(abs(0.0), &target).is_none());
                assert_abs_diff_eq!(mode.control(abs(0.1), &target).unwrap(), rel(3));
                assert_abs_diff_eq!(mode.control(abs(0.5), &target).unwrap(), rel(5));
                assert_abs_diff_eq!(mode.control(abs(1.0), &target).unwrap(), rel(8));
            }

            #[test]
            fn max_step_count() {
                // Given
                let mut mode: Mode<TestTransformation> = Mode {
                    absolute_mode: AbsoluteMode::IncrementalButtons,
                    step_count_interval: create_discrete_increment_interval(1, 2),
                    ..Default::default()
                };
                let target = TestTarget {
                    current_value: Some(UnitValue::MIN),
                    control_type: ControlType::Relative,
                };
                // When
                // Then
                assert!(mode.control(abs(0.0), &target).is_none());
                assert_abs_diff_eq!(mode.control(abs(0.1), &target).unwrap(), rel(1));
                assert_abs_diff_eq!(mode.control(abs(0.5), &target).unwrap(), rel(2));
                assert_abs_diff_eq!(mode.control(abs(1.0), &target).unwrap(), rel(2));
            }

            #[test]
            fn source_interval() {
                // Given
                let mut mode: Mode<TestTransformation> = Mode {
                    absolute_mode: AbsoluteMode::IncrementalButtons,
                    source_value_interval: create_unit_value_interval(0.5, 1.0),
                    ..Default::default()
                };
                let target = TestTarget {
                    current_value: Some(UnitValue::MIN),
                    control_type: ControlType::Relative,
                };
                // When
                // Then
                assert!(mode.control(abs(0.0), &target).is_none());
                assert!(mode.control(abs(0.25), &target).is_none());
                assert_abs_diff_eq!(mode.control(abs(0.5), &target).unwrap(), rel(1));
                assert_abs_diff_eq!(mode.control(abs(1.0), &target).unwrap(), rel(1));
            }

            #[test]
            fn source_interval_step_interval() {
                // Given
                let mut mode: Mode<TestTransformation> = Mode {
                    absolute_mode: AbsoluteMode::IncrementalButtons,
                    source_value_interval: create_unit_value_interval(0.5, 1.0),
                    step_count_interval: create_discrete_increment_interval(4, 8),
                    ..Default::default()
                };
                let target = TestTarget {
                    current_value: Some(UnitValue::MIN),
                    control_type: ControlType::Relative,
                };
                // When
                // Then
                assert!(mode.control(abs(0.0), &target).is_none());
                assert!(mode.control(abs(0.25), &target).is_none());
                assert_abs_diff_eq!(mode.control(abs(0.5), &target).unwrap(), rel(4));
                assert_abs_diff_eq!(mode.control(abs(1.0), &target).unwrap(), rel(8));
            }

            #[test]
            fn reverse() {
                // Given
                let mut mode: Mode<TestTransformation> = Mode {
                    absolute_mode: AbsoluteMode::IncrementalButtons,
                    reverse: true,
                    ..Default::default()
                };
                let target = TestTarget {
                    current_value: Some(UnitValue::MIN),
                    control_type: ControlType::Relative,
                };
                // When
                // Then
                assert!(mode.control(abs(0.0), &target).is_none());
                assert_abs_diff_eq!(mode.control(abs(0.1), &target).unwrap(), rel(-1));
                assert_abs_diff_eq!(mode.control(abs(0.5), &target).unwrap(), rel(-1));
                assert_abs_diff_eq!(mode.control(abs(1.0), &target).unwrap(), rel(-1));
            }
        }

        mod feedback {
            use super::*;

            #[test]
            fn default() {
                // Given
                let mode: Mode<TestTransformation> = Mode {
                    absolute_mode: AbsoluteMode::IncrementalButtons,
                    ..Default::default()
                };
                // When
                // Then
                assert_abs_diff_eq!(mode.feedback(uv(0.0)).unwrap(), uv(0.0));
                assert_abs_diff_eq!(mode.feedback(uv(0.5)).unwrap(), uv(0.5));
                assert_abs_diff_eq!(mode.feedback(uv(1.0)).unwrap(), uv(1.0));
            }

            #[test]
            fn reverse() {
                // Given
                let mode: Mode<TestTransformation> = Mode {
                    absolute_mode: AbsoluteMode::IncrementalButtons,
                    reverse: true,
                    ..Default::default()
                };
                // When
                // Then
                assert_abs_diff_eq!(mode.feedback(uv(0.0)).unwrap(), uv(1.0));
                assert_abs_diff_eq!(mode.feedback(uv(0.5)).unwrap(), uv(0.5));
                assert_abs_diff_eq!(mode.feedback(uv(1.0)).unwrap(), uv(0.0));
            }

            #[test]
            fn source_and_target_interval() {
                // Given
                let mode: Mode<TestTransformation> = Mode {
                    absolute_mode: AbsoluteMode::IncrementalButtons,
                    source_value_interval: create_unit_value_interval(0.2, 0.8),
                    target_value_interval: create_unit_value_interval(0.4, 1.0),
                    ..Default::default()
                };
                // When
                // Then
                assert_abs_diff_eq!(mode.feedback(uv(0.0)).unwrap(), uv(0.2));
                assert_abs_diff_eq!(mode.feedback(uv(0.4)).unwrap(), uv(0.2));
                assert_abs_diff_eq!(mode.feedback(uv(0.7)).unwrap(), uv(0.5));
                assert_abs_diff_eq!(mode.feedback(uv(1.0)).unwrap(), uv(0.8));
            }
        }
    }

    fn uv(number: f64) -> UnitValue {
        UnitValue::new(number)
    }

    fn abs(number: f64) -> ControlValue {
        ControlValue::absolute(number)
    }

    fn rel(increment: i32) -> ControlValue {
        ControlValue::relative(increment)
    }
}
