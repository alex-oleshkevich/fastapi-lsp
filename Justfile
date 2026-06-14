# Step 3 — opt in via ~/.config/zed/settings.json:
#   "languages": { "Python": { "language_servers": ["fastapi-lsp", "..."] } }

install-zed:
    cargo build --release
    cp target/release/fastapi-lsp ~/.cargo/bin/
    ./scripts/install-zed-extension.sh
