//! Descriptor-driven Starlark configuration constructors with a shared runtime.
mod generate;
pub use generate::{generate, generate_with_runtime};
pub mod codec;
pub mod runtime;
