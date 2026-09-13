//! Break one block on a **running** server, so a real Minecraft client connected to it receives a
//! `light_update`.
//!
//! ## Why this is not a normal test
//!
//! Our `light_update` has never been received by a real client, and that is the one thing about it that no
//! amount of self-testing can settle — our own `TestClient` agreeing with our own encoder is exactly the
//! evidence that failed in KD-44. A real client only receives one if the server changes a block while it is
//! connected, and the server changes blocks only when a *player* breaks or places one, which needs a person at
//! a keyboard.
//!
//! This test is the person. It logs in as a second player, breaks the block it is standing on, and the
//! resulting `light_update` goes to **every** session holding that chunk — including the real client, which is
//! the whole point.
//!
//! Ignored by default because it needs a server that somebody else started.
//!
//! ## Running it
//!
//! ```text
//! MC_TRIGGER_ADDR=127.0.0.1:25577 \
//!   cargo test -p mc-server --test light_update_trigger -- --ignored --nocapture
//! ```
//!
//! `tools/light-update-trigger/run.py` starts the server, the rig and the real client and then runs this,
//! so that is the command to use unless you are doing something unusual.

// The coordinates come from the server and are within the world, so the `f64 -> i32` casts are exact
// for any position a player can occupy. The same exemption is documented at the top of `game.rs`.
#![allow(clippy::cast_possible_truncation)]

use mc_protocol::RawPacket;
use mc_protocol::ids::serverbound;
use mc_protocol::packets::Packet;
use mc_protocol::packets::play::{PlayerPosition, block_position};
use mc_protocol::varint::write_varint;
use mc_test_support::client::TestClient;
use std::time::Duration;

/// The block-break action, as `ServerboundPlayerActionPacket` writes it: `VarInt` action, packed position,
/// `u8` direction, `VarInt` sequence.
fn block_break_payload(x: i32, y: i32, z: i32) -> Vec<u8> {
    let mut payload = Vec::new();
    write_varint(&mut payload, 0); // START_DESTROY_BLOCK
    payload.extend_from_slice(&block_position(x, y, z).to_be_bytes());
    payload.push(1); // direction: up — any value is accepted for a break
    write_varint(&mut payload, 0); // sequence
    payload
}

#[tokio::test]
#[ignore = "needs a server started by target/p10_trigger_light_update.py"]
async fn break_a_block_so_a_real_client_receives_a_light_update() {
    let addr = std::env::var("MC_TRIGGER_ADDR")
        .expect("MC_TRIGGER_ADDR must name a running server, e.g. 127.0.0.1:25577");
    let (mut client, joined) = TestClient::login_join(addr.parse().expect("address"), "Trigger")
        .await
        .expect("logs in");
    println!("logged in as {}", joined.login.name);

    // The player's position is sent **after** `join_game`, which `login_join` stops at, so it is read here.
    // Without it there is nothing to reach: the server refuses a break further than 4.5 blocks away.
    let (x, y, z) = tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let packet = client.recv().await.expect("a packet arrives");
            if packet.id == mc_protocol::ids::clientbound::play::PLAYER_POSITION {
                let position = PlayerPosition::decode(&packet.payload).expect("decodes");
                break (position.x, position.y, position.z);
            }
        }
    })
    .await
    .expect("a position arrives after the join");

    // The block the player stands on: within reach, and breaking it opens a hole the sky light falls into, so
    // the chunk's light genuinely changes and the server has something to send.
    let (bx, by, bz) = (x.floor() as i32, y.floor() as i32 - 1, z.floor() as i32);
    println!("breaking the block at ({bx}, {by}, {bz}) under the player at ({x}, {y}, {z})");

    // **Wait until the chunks around us are there before digging.** The server refuses a break in a chunk it has
    // not loaded, and a client is in exactly that state for a while after `join_game` — so a break sent
    // immediately is silently discarded, which is what made the first `light_update` capture contain zero id-48
    // packets and left KD-49 resting on a packet that was never sent.
    let mut seen_chunks = 0_u32;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    while seen_chunks < 40 && tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_secs(10), client.recv()).await {
            Ok(Ok(packet)) => {
                if packet.id == mc_protocol::ids::clientbound::play::LEVEL_CHUNK_WITH_LIGHT {
                    seen_chunks += 1;
                }
            }
            _ => break,
        }
    }
    println!("received {seen_chunks} chunks before digging");

    client
        .send_raw_packet(&RawPacket::new(
            serverbound::play::PLAYER_ACTION,
            block_break_payload(bx, by, bz),
        ))
        .await
        .expect("the break is sent");

    // The server needs a few ticks: one to apply the change and queue the light, and up to a tick's worth of
    // the per-tick budget to send it.
    tokio::time::sleep(Duration::from_secs(4)).await;
    println!(
        "done; if the break was accepted, a light_update went to every session holding that chunk"
    );
}
