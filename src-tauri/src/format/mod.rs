//! Work-file container detection and structural layout.
//!
//! The chunk writer and the history-node thumbnail extractor share this module
//! so container knowledge exists once. Parsing is strictly defensive: any
//! inconsistency returns an error, and each caller degrades on its own — the
//! chunk writer falls back to generic content-defined chunking, the preview
//! extractor to no preview. A parse failure can therefore only change how well
//! a file chunks or whether it gets a thumbnail, never what gets stored or
//! restored.

pub(crate) mod container;
