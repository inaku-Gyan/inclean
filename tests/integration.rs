//! Integration tests. Each module under `integration/` covers one fixture
//! / scenario; helpers live in `common`. All tests drive the public library
//! API in [`inclean::pipeline::run`].
//!
//! Submodules are pulled in via `#[path]` so we keep a single test binary
//! (faster compile and link) while the files themselves live in the
//! `integration/` subdirectory.

#[path = "integration/common.rs"]
mod common;

#[path = "integration/action_error.rs"]
mod action_error;

#[path = "integration/angle_allowed.rs"]
mod angle_allowed;

#[path = "integration/auto_file_dir.rs"]
mod auto_file_dir;

#[path = "integration/child_wider.rs"]
mod child_wider;

#[path = "integration/cross_chain.rs"]
mod cross_chain;

#[path = "integration/flat_library.rs"]
mod flat_library;

#[path = "integration/init_template.rs"]
mod init_template;

#[path = "integration/layer5_ambiguity.rs"]
mod layer5_ambiguity;

#[path = "integration/layer5_disambiguation.rs"]
mod layer5_disambiguation;

#[path = "integration/layer5_under.rs"]
mod layer5_under;

#[path = "integration/multi_module.rs"]
mod multi_module;

#[path = "integration/nested_library.rs"]
mod nested_library;

#[path = "integration/trailing_comment.rs"]
mod trailing_comment;

#[path = "integration/validation_keep.rs"]
mod validation_keep;
