//! Legacy sync-v2 runtime wire builders.
//!
//! These helpers intentionally isolate the remaining v2 bootstrap/change feed
//! payload assembly from the live server module so the runtime wire boundary
//! makes the compat-only surface explicit.
