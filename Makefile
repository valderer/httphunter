.PHONY: web-install web-dev web-build desktop-dev desktop-build

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

# Builds the installer for the current operating system.
desktop-build:
	cd desktop/src-tauri && ../web/node_modules/.bin/tauri build
