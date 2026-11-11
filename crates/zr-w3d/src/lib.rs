//! W3D assets for the viewer: chunk-tree assembly into world-space
//! (model-space) triangle meshes in the rest pose, texture decoding (DDS/TGA),
//! Object INI -> model / condition-state resolution, and hierarchy animation
//! decoding with a pose evaluator.
//!
//! Multi-pass materials, vertex colours and LOD switching are out of scope: the
//! highest LOD of the HLOD is used, in the rest pose. `Model` is baked in the
//! rest pose; animated playback needs the `Hierarchy` / `Animation` API
//! (`Hierarchy::pose`).

mod anim;
mod dds;
mod error;
mod hierarchy;
mod model;
mod state;
mod template;
mod texture;
mod tga;

pub use anim::{load_animation, parse_animation, AnimFlavor, Animation};
pub use error::Error;
pub use hierarchy::{
    load_hierarchy, model_hierarchy, quat_about_axis, quat_mul, quat_rotate, quat_to_mat, xform_dir, xform_point,
    BoneControl, Hierarchy, Mat34, Pivot, Trs, IDENTITY34, IDENTITY_QUAT,
};
pub use model::{load_model, BlendMode, Model, ModelMesh, TextureParams, ALPHA_TEST_REF};
pub use state::{
    draw_states_by_module, states_for_template, AnimMode, AnimationRef, ConditionFlags, ConditionState, DrawStates,
    TransitionState,
};
pub use template::model_for_template;
pub use texture::load_texture;
