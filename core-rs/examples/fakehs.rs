//! Runs the fake homeserver on localhost until stopped (dev aid): prints its address; "bob" says a few things in the encrypted room once
//! "alice" has signed in. `cargo run --example fakehs --features testkit`.
use vector_core::testkit::{alice_other_session, bob_says, FakeHs};

#[tokio::main]
async fn main() {
    let hs = FakeHs::start().await;
    hs.invite_alice("Bob's club");
    println!("{}", hs.uri());
    let lines = ["Hello alice, this room is end-to-end encrypted.", "You are reading it through matrix-sdk, drawn by QML.", "Reply below!"].map(String::from).to_vec();
    tokio::spawn(bob_says(hs.clone(), lines));
    tokio::spawn(alice_other_session(hs.clone()));
    let mut shown = 0;
    loop { /* what the client does that a person would not see: receipts and typing */
        let log = hs.log();
        for l in &log[shown..] { if l.starts_with("receipt") || l.starts_with("typing") || l.starts_with("read_markers") { eprintln!("fake server saw: {l}"); } }
        shown = log.len();
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    }
}
