use std::{collections::HashMap, num::ParseIntError, path::Path, sync::Arc};

use bellframe::{
    method::LABEL_LEAD_END,
    music::Regex,
    place_not::{self, PnBlockParseError},
    Bell, Method, Stage,
};
use hmap::hmap;
use itertools::Itertools;
use monument::{
    single_method::{single_method_layout, SingleMethodError},
    Config, MusicType, Spec,
};
use serde_derive::Deserialize;

use crate::test_data::TestData;

use self::{
    calls::{BaseCalls, SpecificCall},
    length::Length,
};

mod calls;
mod length;

/// The specification for a set of compositions which Monument should search.  The [`Spec`] type is
/// parsed directly from the `TOML`, and can be thought of as an AST representation of the TOML
/// file.  This requires additional processing and verification before it can be converted into an
/// [`Engine`].
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AbstractSpec {
    /// Which bells are allowed to be affected by calls
    non_fixed_bells: Option<Vec<Bell>>,
    /// The range of lengths of composition which are allowed
    length: Length,
    /// Monument won't stop until it generates the `num_comps` best compositions
    num_comps: usize,
    /// The call type specified by the `base_calls` argument
    base_calls: Option<BaseCalls>,

    /// The [`Method`] who's compositions we are after
    method: MethodSpec,
    /// Which calls to use in the compositions
    #[serde(default)]
    calls: Vec<SpecificCall>,
    /// Which music to use
    music: Vec<MusicSpec>,

    /// Data for the testing/benchmark harness.  It is public so that it can be accessed by the
    /// testing harness
    pub test_data: Option<TestData>,
}

impl AbstractSpec {
    /// Read a `Spec` from a TOML file
    pub fn read_from_file(path: &Path) -> Result<Self, toml::de::Error> {
        let spec_toml = std::fs::read_to_string(&path).unwrap();
        toml::from_str(&spec_toml)
    }

    /// Create an [`Engine`] which generates compositions according to this `Spec`
    pub fn to_spec(&self, config: &Config) -> Result<Arc<Spec>, SpecConvertError> {
        // Parse and verify into its fully specified form
        let method = self.method.gen_method()?;
        let calls = calls::gen_calls(self.method.stage, self.base_calls.as_ref(), &self.calls)?;
        let non_fixed_bells = self.non_fixed_bells.as_ref().map_or_else(
            || tenors_together_non_fixed_bells(self.method.stage),
            Vec::clone,
        );
        let music_types = self
            .music
            .iter()
            .map(|b| b.to_music_type(method.stage()))
            .collect_vec();
        // For the time being, just use the default plain calling positions.  These will be wrong
        // for anything other than lead end calls, but are only used for debugging so that's
        // probably fine.  TODO: Make these specifiable
        let plain_lead_positions = None;

        // Generate a `Layout` from the data about the method and calls
        let layout = single_method_layout(&method, plain_lead_positions, &calls, &non_fixed_bells)
            .map_err(SpecConvertError::LayoutGenError)?;

        Ok(Spec::new(
            layout,
            self.length.range.clone(),
            self.num_comps,
            music_types,
            // Always normalise music
            true,
            config,
        ))
    }
}

fn tenors_together_non_fixed_bells(stage: Stage) -> Vec<Bell> {
    // By default, fix the treble and >=7.  Also, make sure to always fix the tenor even on Minor
    // or lower.  Note that we're using 1-indexed bell 'numbers' here
    (2..=(stage.as_usize() - 1).min(6))
        .map(|num| Bell::from_number(num).unwrap())
        .collect_vec()
}

/// The possible ways that a [`Spec`] -> [`Engine`] conversion can fail
#[derive(Debug, Clone)]
pub enum SpecConvertError<'s> {
    NoCalls,
    CallPnParse(&'s str, place_not::ParseError),
    MethodPnParse(PnBlockParseError),
    LeadLocationIndex(&'s str, ParseIntError),
    LayoutGenError(SingleMethodError),
}

/// The contents of the `[method]` header in the input TOML file
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MethodSpec {
    #[serde(default)]
    name: String,
    place_notation: String,
    stage: Stage,
    /// The inputs to this map are numerical strings representing sub-lead indices, and the outputs
    /// are the lead location names
    #[serde(default = "default_lead_locations")]
    lead_locations: HashMap<String, String>,
}

impl MethodSpec {
    fn gen_method(&self) -> Result<Method, SpecConvertError<'_>> {
        let mut m =
            Method::from_place_not_string(self.name.clone(), self.stage, &self.place_notation)
                .map_err(SpecConvertError::MethodPnParse)?;
        for (index_str, name) in &self.lead_locations {
            let index = index_str
                .parse::<isize>()
                .map_err(|e| SpecConvertError::LeadLocationIndex(&index_str, e))?;
            let lead_len = m.lead_len() as isize;
            let wrapped_index = ((index % lead_len) + lead_len) % lead_len;
            // This cast is OK because we used % twice to guarantee a positive index
            m.set_label(wrapped_index as usize, Some(name.clone()));
        }
        Ok(m)
    }
}

/* Music */

/// The specification for one type of music
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged, deny_unknown_fields)]
pub enum MusicSpec {
    Runs {
        #[serde(rename = "run_length")]
        length: usize,
        #[serde(default = "get_one")]
        weight: f32,
    },
    RunsList {
        #[serde(rename = "run_lengths")]
        lengths: Vec<usize>,
        #[serde(default = "get_one")]
        weight: f32,
    },
    Pattern {
        pattern: String,
        #[serde(default = "get_one")]
        weight: f32,
    },
    Patterns {
        patterns: Vec<String>,
        #[serde(default = "get_one")]
        weight: f32,
    },
}

impl MusicSpec {
    /// Gets the weight given to one instance of this `MusicSpec`
    pub fn weight(&self) -> f32 {
        match self {
            Self::Runs { weight, .. } => *weight,
            Self::RunsList { weight, .. } => *weight,
            Self::Pattern { weight, .. } => *weight,
            Self::Patterns { weight, .. } => *weight,
        }
    }

    /// Gets the [`Regex`]es which match this `MusicSpec`
    pub fn regexes(&self, stage: Stage) -> Vec<Regex> {
        match self {
            Self::Runs { length, .. } => Regex::runs_front_or_back(stage, *length),
            Self::RunsList { lengths, .. } => lengths
                .iter()
                .flat_map(|length| Regex::runs_front_or_back(stage, *length))
                .collect_vec(),
            Self::Pattern { pattern, .. } => vec![Regex::parse(&pattern)],
            Self::Patterns { patterns, .. } => {
                patterns.iter().map(|s| Regex::parse(&s)).collect_vec()
            }
        }
    }

    /// Generates a [`MusicType`] representing `self`.
    pub fn to_music_type(&self, stage: Stage) -> MusicType {
        MusicType::new(self.regexes(stage), self.weight())
    }
}

/* Deserialization helpers */

#[inline(always)]
fn get_one() -> f32 {
    1.0
}

/// By default, add a lead location "LE" on the 0th row (i.e. when the place notation repeats).
#[inline]
fn default_lead_locations() -> HashMap<String, String> {
    hmap! { "0".to_owned() => LABEL_LEAD_END.to_owned() }
}
