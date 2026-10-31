# Desktop foundation — implementation and validation

Updated: 17 October 2026.

## Scope

The desktop foundation is the Tauri/Rust application shell, React bootstrap, window behavior, packaged assets, repeatable build configuration, and installation-validation procedure. Capture, reviewed handoffs, remote security approval, and developer-pilot gates remain separate product work.

## Implemented

- Installer product name, native window title, and HTML title use **Lexsus**.
- The window has an explicit `main` label matching its capability file and minimum dimensions of 800 × 600.
- Tauri development and build hooks use the repository's pinned pnpm manager.
- Frontend dependency lockfile matches the current manifest, including the previous removal of HeroUI, xterm, and the JavaScript opener dependency.
- The WebView has explicit production and loopback-development content security policies. Production script sources are local; objects and child frames are disabled. Inline styles remain allowed for the component library. Network-facing tools execute through Rust rather than browser fetch.
- An application-level render boundary contains failures outside the dashboard. Routine render logs omit exception content and component stacks.
- Failed saved-workspace reads show a retry action instead of becoming an apparently empty workspace. Startup callbacks ignore results after disposal. Unavailable optional recent-project storage does not block workspace opening.
- `.github/workflows/desktop-build.yml` builds unsigned Linux DEB, Windows NSIS, and macOS DMG artifacts on relevant pull requests or manual dispatch. It uploads artifacts without publishing releases.
- The existing bridge split is repaired: schema dependencies and parent helper visibility are explicit, trailing orphan attributes/comments are removed, and command events retain their serializable, cloneable desktop wire shape.

The application identifier remains `com.aicb.bridge`. Changing it would change platform application-data locations and can strand existing archives and credentials. The internal Rust crate name is retained for compatibility; it is separate from the displayed product name.

## Build commands

```bash
pnpm install --frozen-lockfile
pnpm run typecheck
pnpm run lint
pnpm run build
cargo check --manifest-path src-tauri/Cargo.toml
cargo fmt --manifest-path src-tauri/Cargo.toml --all -- --check
cargo test --manifest-path src-tauri/Cargo.toml --lib
pnpm tauri build --ci --no-sign --bundles deb
```

Use `--bundles nsis` on Windows and `--bundles dmg` on macOS. Use `--debug` for a development package; a debug package is not release-build evidence. Linux requires GTK 3, WebKitGTK 4.1, and the other native build dependencies listed in the package workflow. Packages appear under `src-tauri/target/<profile>/bundle/`.

Unsigned artifacts are for validation. Windows signing and macOS signing/notarization require separately provisioned credentials before public distribution. No signing keys or publishing credentials are added here.

## Installation and runtime acceptance

Run on each named OS/version before marking that platform ready. Record app revision, package checksum, OS/architecture, and actual outcomes.

- [ ] Install the package on a clean test account; confirm the launcher and native title identify Lexsus.
- [ ] Launch from the installed application, without a Vite server or repository checkout.
- [ ] Confirm local scripts, styles, and fonts load under the production CSP; inspect for blocked IPC or asset requests.
- [ ] Confirm the dashboard becomes interactive and record startup time and hardware for NFR-001.
- [ ] Minimize, maximize/restore, drag, resize to the minimum dimensions, and close the window.
- [ ] Toggle the theme, restart, and confirm preference restoration.
- [ ] Select a temporary project, restart, and confirm the intended workspace is restored.
- [ ] Remove a saved temporary project and confirm the app offers project selection.
- [ ] Exercise the startup-retry and render-error recovery states in a development environment.
- [ ] Confirm approval/question banners and keyboard focus remain usable at the minimum size.
- [ ] Verify writes/commands remain disabled by default and no public tunnel starts without activation.
- [ ] Close with active test processes and verify connector, tunnel, and owned process cleanup.
- [ ] Upgrade over a previous test installation; confirm existing archive and settings remain readable.
- [ ] Uninstall and document retained application data without deleting real user data.

These items require observed runtime evidence. CI package generation alone does not complete them.

## Validation record

See the latest session notes for actual command outcomes. Windows/macOS installation, signing, production-WebView CSP behavior, startup timing, and upgrade checks remain unverified until their acceptance runs are recorded.
