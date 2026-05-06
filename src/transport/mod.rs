pub mod detect;
pub mod serial;

pub use detect::resolve_port;
pub use serial::{spawn_serial_tasks, OutboundFrame};
