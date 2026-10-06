//! The client fastetcd is spoken to with (#47): `console_core::tls`, which
//! started here and is shared with stormcluster's plugin since #89.

pub use console_core::tls::{build, Client, TlsFiles};
