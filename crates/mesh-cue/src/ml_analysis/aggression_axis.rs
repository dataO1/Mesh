//! Re-export of the IntensityAxis types from mesh-core. The implementation
//! moved to `mesh_core::intensity_axis` so mesh-player can load the same
//! axis without depending on mesh-cue. See
//! `documents/intensity-axis-pipeline-runbook.md` for the pipeline.

pub use mesh_core::intensity_axis::{IntensityAxis, SubAxis, EMBEDDING_DIM};
