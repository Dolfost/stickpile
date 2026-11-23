//! The Bot API mock of the tests (`tests/mock_bot`), served on its own for
//! trying exports by hand without Telegram:
//!
//! ```sh
//! cargo run -p stickpile-telegram --example mock_bot
//! ```
//!
//! It prints its base URL, the token and the bot's username. Point a
//! throwaway server at it: `[telegram] api_url` = the URL and
//! `bot_token_file` = a file holding the token. Its packs live in memory
//! and are gone when it stops.

#[path = "../tests/mock_bot/mod.rs"]
mod mock_bot;

#[tokio::main]
async fn main() {
    let (url, _state) = mock_bot::start().await;
    println!("url: {url}");
    println!("token: {}", mock_bot::TOKEN);
    println!("bot: {}", mock_bot::BOT);
    std::future::pending::<()>().await;
}
