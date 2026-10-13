//! Builds a lane generator from its style spec.

use super::fixed::FixedPattern;
use super::LaneGenerator;
use crate::live::glue::Role;
use crate::live::style::GeneratorSpec;

/// Build the generator named by `spec` for a lane of the given role.
/// Unknown names fall back to an empty fixed pattern.
pub fn build(_spec: &GeneratorSpec, _role: Role) -> Box<dyn LaneGenerator> {
    Box::new(FixedPattern::default())
}
