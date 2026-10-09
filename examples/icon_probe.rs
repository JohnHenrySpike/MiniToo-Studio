//! Which icon a notification card shows for an application: renders cards to PNG.
//!
//! ```text
//! cargo run --example icon_probe -- OUT_DIR APP[:DESKTOP_ENTRY]...
//! ```

use minitoo::notify_card::{IconSource, card_icon, render};

fn main() {
    let mut args = std::env::args().skip(1);
    let dir = std::path::PathBuf::from(args.next().expect("OUT_DIR"));
    for (i, spec) in args.enumerate() {
        let (app, entry) = spec.split_once(':').unwrap_or((spec.as_str(), ""));
        let src = IconSource { desktop_entry: entry, ..Default::default() };
        let found = card_icon(app, src).is_some();
        println!("card{i}.png  {app} / {entry:?}: {}", if found { "icon" } else { "no icon" });
        std::fs::write(dir.join(format!("card{i}.png")), render(app, "Summary", "Body text of the notification", src, "12:00").png()).unwrap();
    }
}
