//! Typed packets for the Phase-02 protocol slice.
//!
//! Every packet follows the same shape: `ID` constant, [`Packet::decode`] from
//! a payload slice and [`Packet::encode`] to a full payload (id included).
//! Writes return [`mc_core::error::ServerResult`] because strings are
//! length-capped on the wire; reads never panic on hostile input.

pub mod config;
pub mod handshake;
pub mod login;
pub mod play;
pub mod status;

use crate::RawPacket;
use mc_core::error::ServerResult;

/// Common interface for typed packets.
///
/// `encode` returns the packet **body** (everything after the id `VarInt`);
/// [`Packet::to_raw`] pairs it with [`Packet::ID`] for the frame layer.
pub trait Packet: Sized {
    /// Wire packet id for protocol 775.
    const ID: i32;

    /// Decode the payload (after the id) into `Self`.
    ///
    /// # Errors
    ///
    /// [`mc_core::error::ServerError::Protocol`] for malformed data.
    fn decode(payload: &[u8]) -> ServerResult<Self>;

    /// Encode `self` into a body (without the packet id).
    ///
    /// # Errors
    ///
    /// [`mc_core::error::ServerError`] only for invariant violations such as
    /// over-long server-authored strings.
    fn encode(&self) -> ServerResult<Vec<u8>>;

    /// Pair the encoded body with the packet id for framing.
    ///
    /// # Errors
    ///
    /// Same as [`Packet::encode`].
    fn to_raw(&self) -> ServerResult<RawPacket> {
        Ok(RawPacket::new(Self::ID, self.encode()?))
    }
}
