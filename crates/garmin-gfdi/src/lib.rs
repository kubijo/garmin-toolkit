//! Transport-independent Garmin GFDI framing and message validation.
//!
//! The caller owns transport, deadlines, device selection, and permission to send.
//! This crate does not discover, pair, connect, or perform transport I/O.

pub mod download;
pub mod file_access;
mod framing;
mod identity;
mod message;
mod mlr;
mod mlr_sender;
mod multilink;
pub mod pairing;
mod probe;
pub mod proto;
mod route;
pub mod rpc;

pub use framing::{FrameDecoder, FrameError, encode_frame};
pub use identity::{DeviceInformation, IdentityError};
pub use message::{Message, MessageError, encode_message};
pub use mlr::{MlrError, MlrReceiver, MlrUpdate};
pub use mlr_sender::{MlrSendError, MlrSender};
pub use multilink::{Channel, LinkMode, Registration, RegistrationError, RegistrationState};
pub use probe::{IdentityProbe, ProbeError};
pub use route::{Characteristic, Route, RouteKind, select_route};
