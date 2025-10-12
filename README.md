<div align="center">
  <img src="assets/logo.png" alt="hyprwhspr-rs logo" width="200" />
  <h1>hyprwhspr-rs</h1>
  <p>Rust implementation of [hyprwhspr](https://github.com/goodroot/hyprwhspr) | Native speech-to-text voice dictation for Hyprland.</p>
</div>



# Requirements

- whisper.cpp ([GitHub](https://github.com/ggml-org/whisper.cpp), [AUR](https://aur.archlinux.org/packages/whisper.cpp))

# Development

1. `git clone https://github.com/better-slop/hyprwhispr-rs.git`
2. `cd hyprwhspr-rs`
3. `cargo build --release`
4. Run using:
    - Nice logs with pretty text transformation diffs: `RUST_LOG=debug ./target/release/hyprwhspr-rs --test`
    - Production release `./target/release/hyprwhspr-rs`
