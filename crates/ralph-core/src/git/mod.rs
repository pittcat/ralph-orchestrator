//! Boundary-correct git ancestry probes.
//!
//! `is_git_ancestor` is the single shared implementation the DAG
//! runtime uses to answer "is A an ancestor of B". It delegates to
//! `git merge-base --is-ancestor` so callers can never fall back on
//! unsafe string-prefix matching of commit OIDs.

pub mod ancestry;

pub use ancestry::{GitError, is_git_ancestor};
