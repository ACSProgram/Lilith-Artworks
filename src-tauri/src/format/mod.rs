//! Work-file container detection and structural layout.
//!
//! The chunk writer and the (stage 8) thumbnail extractor share this module so
//! container knowledge exists once. Parsing is strictly defensive: any
//! inconsistency returns an error and the caller falls back to generic
//! content-defined chunking, so a parse failure can only change how well a file
//! chunks, never what gets stored or restored.

pub(crate) mod container;
