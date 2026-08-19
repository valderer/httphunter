.PHONY: dev check run web-install web-dev web-build desktop-dev

# Watches Rust sources and restarts the local proxy after each change.
dev:
	RUST_LOG=info cargo watch --watch src --watch crates/hunter-core --watch Cargo.toml --watch config.toml -x 'run -- --config config.toml proxy'

check:
	cargo check

run:
	RUST_LOG=info cargo run -- --config config.toml proxy

# React/Vite desktop UI. Requires Node.js 22 LTS (see desktop/web/.nvmrc).
web-install:
	npm --prefix desktop/web install

web-dev:
	npm --prefix desktop/web run dev

web-build:
	npm --prefix desktop/web run build

# Starts the Tauri desktop shell and its Vite development server.
desktop-dev:
	cd desktop/src-tauri && ../web/node_modules/.bin/tauri dev
