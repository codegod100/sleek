//! Redirect — use the workspace package:
//!
//! ```bash
//! cargo run -p vidya-demo
//! ./scripts/waydroid-demo.sh install
//! ```

fn main() {
    eprintln!("Use: cargo run -p vidya-demo              # Waydroid");
    eprintln!("     cargo run -p vidya-demo -- --host    # desktop");
    std::process::exit(2);
}
