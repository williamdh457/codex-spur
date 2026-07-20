# Codex Spur website

Standalone Vite + React + TypeScript marketing site. It is intentionally isolated from the desktop application's root `dist/` and Tauri frontend entrypoint.

```bash
npm run site:dev
npm run site:typecheck
npm run site:build
npm run site:preview
npm run site:deploy
```

`site:build` validates the release manifest against the root package, Tauri config, Cargo version, and the local Apple Silicon DMG when one exists. `site:deploy` copies the built site into a Git-SHA release directory and atomically updates the `current` symlink.
