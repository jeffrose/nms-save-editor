//! Save document model for No Man's Sky.
//!
//! This crate works one level above [`nms_format`]: it turns a save payload
//! into an editable JSON tree that preserves the exact text of everything the
//! user did not touch, resolves obfuscated on-disk keys to readable field
//! names, and can compare payloads to prove what a write changed.

pub mod document;
pub mod keymap;

pub use document::{ByteDiff, Document, ModelError, PathError, SaveInfo, TreeChange, readable_tree, tree_diff};
pub use keymap::KeyMap;
