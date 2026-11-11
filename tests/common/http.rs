//! The services of a test's own on 127.0.0.1, strict as the real ones: the
//! demo's services (src/demo/services.rs), which `switchboard demo` serves
//! its made-up data through, so the tests and the demo share one copy.

pub use switchboard::demo::services::*;
