// Legacy loop-runner regression suite, split into per-behavior submodules.
// This file is the directory entry; the actual tests live in `legacy::*`.

#[path = "legacy/activation_outcome.rs"]
pub mod activation_outcome;
#[path = "legacy/diagnosis.rs"]
pub mod diagnosis;
#[path = "legacy/event_processing.rs"]
pub mod event_processing;
#[path = "legacy/helpers.rs"]
pub mod helpers;
#[path = "legacy/misc.rs"]
pub mod misc;
#[path = "legacy/pty.rs"]
pub mod pty;
#[path = "legacy/recovery.rs"]
pub mod recovery;
#[path = "legacy/termination.rs"]
pub mod termination;
