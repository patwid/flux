# Flux

Flux is a GTK4/libadwaita streaming frontend written in Rust using Relm4.
It takes its inspiration from [NewPipe](https://newpipe.net/) (Android), but it
targets **mobile Linux first** (Phosh, Plasma Mobile; for example PinePhone and Librem 5)
and also runs on desktop Linux.

This file is the project spec and the working agreement for anyone
contributing to the code, human or agent.

## Goals

- A privacy-friendly YouTube client. It needs no Google account and has no ads.
- **Responsive UI**: it must work at 360×640 (phone portrait) and scale up to desktop windows.
- **Features (v1)**:
  1. **Search**: search for videos and channels.
  2. **Playback**: play videos in-app, with quality selection, seeking and subtitles. Subtitles are off until the user picks a language, which is then remembered for later videos.
  3. **Subscriptions**: subscribe to channels and get an aggregated feed from RSS.
  4. **History**: keep a watch history with resume position, and let the user clear it.
  5. **Download**: download videos or audio to disk, with progress and cancel. Downloaded items can be played offline. Video downloads include their subtitles, saved next to the video as `.vtt` files.
  6. **Comments**: show a video's top comments on its page, loaded page by page: the first page with the video's details, the next ones when the user asks for them. The replies to a comment open on their own page (one level, as on YouTube).
  7. **Playlists**: a channel's page switches between its videos and its playlists, which load when first shown. A playlist opens on its own page with its videos, where it can be saved to the library (a bookmark: its videos are still fetched from YouTube).

### Non-goals (v1)

- Background/audio-only playback and MPRIS
- Importing NewPipe or OPML subscriptions
- Flatpak packaging
- Sites other than YouTube. The backend is still kept behind a trait, so other sites can be added later.
- Writing comments, creating or editing playlists, and account login

## Technology

| Concern       | Choice                                                                 |
|---------------|------------------------------------------------------------------------|
| Language      | Rust (stable, edition 2024)                                             |
| UI            | GTK4 and libadwaita via `relm4` (The Elm Architecture). Target the **latest GNOME** (currently GNOME 50: relm4 feature `gnome_50`, libadwaita 1.9, GTK 4.22) and prefer modern libadwaita widgets (e.g. `AdwSpinner`, `AdwMultiLayoutView`, `AdwViewSwitcherSidebar`, `AdwToggleGroup`, `AdwWrapBox`) over older or GTK-only equivalents. |
| Extraction    | `rustypipe` (YouTube internal API, pure Rust) for search, details and channels. Stream URLs come from YouTube's visionOS player client (`core/backend/visionos.rs`), because rustypipe's clients' URLs only serve the start of a video without a PO token. rustypipe's player is the fallback, also when a visionOS stream URL refuses a byte from the middle of its stream (checked before the streams are used). Channel playlists and playlist videos are raw browse requests through rustypipe, parsed by Flux (`core/backend/lockup.rs`), because rustypipe 0.11 cannot parse YouTube's lockup view models there. |
| Subscriptions | YouTube channel RSS (`https://www.youtube.com/feeds/videos.xml?channel_id=…`) |
| Playback      | GStreamer (`playbin3` + `gtk4paintablesink` → `gtk::Picture`). Separate video/audio streams are played through a generated DASH manifest, so they are fetched in bounded byte ranges. Subtitles come from the player response: manual tracks as srv3, whose window positions YouTube's WebVTT conversion loses, and auto-generated ones as WebVTT; downloads save both as WebVTT. They are drawn over the video by the player widget rather than by GStreamer, placed by their cue settings (except auto-generated tracks, which are centered as on YouTube). |
| Storage       | SQLite via `rusqlite`, with schema migrations and the DB in `$XDG_DATA_HOME/flux/` |
| Downloads     | Own downloader: 10 MiB byte-range chunks (YouTube refuses whole-file requests), muxed with ffmpeg; saved to `$XDG_DOWNLOAD_DIR/Flux/`. Failed requests are retried with backoff from the last byte received, and a retried download resumes its `.part` files. |
| Async         | `tokio`, through Relm4 commands/workers                                         |

App ID: `io.github.patwid.Flux`

## Architecture

Pure logic is kept separate from GTK so that most of the code can be tested
without a display.

```
flake.nix, nix/        # Nix flake: package, app, dev shell
migrations/            # numbered SQL schema migrations (append-only)
data/                  # desktop file, AppStream metainfo, app icon
examples/demo.rs       # offline demo with fake backend and generated videos
tests/                 # integration tests (data files) and fixtures
src/
  main.rs              # app bootstrap only
  lib.rs               # crate root (so integration tests in tests/ can use it)
  core/                # no gtk imports allowed
    model.rs           # domain types: Video, Channel, Playlist, Streams, HistoryEntry, Download…
    backend/           # traits Backend and Downloader; rustypipe and fake impls; feed refresh
    feed.rs            # RSS/Atom parsing → Vec<Video>; feed aggregation and sorting
    streams.rs         # choosing streams / playback source
    subtitles.rs       # WebVTT parsing; the cue to show at a position
    db.rs              # rusqlite: subscriptions, saved playlists, history, downloads, feed cache
    download.rs        # download queue state machine and file naming
  player.rs            # GStreamer playback engine (no gtk; sinks are injected)
  ui/                  # relm4 components, each with a pure `State` + `update`
    app.rs             # root window: navigation, adaptive layouts, page wiring
    feed.rs search.rs                                            # top-level sections
    subscriptions.rs playlists.rs history.rs downloads.rs        # library tabs
    video.rs channel.rs playlist.rs replies.rs                   # pushed pages
    comments.rs        # paged comment list state and widget (video and replies pages)
    player_view.rs     # video widget and controls
    cue_layout.rs      # places subtitles in the video by their cue settings
    thumbnail_row_layout.rs # video rows: thumbnail beside the title, or above it when narrow
    format.rs widgets.rs images.rs aspect.rs style.css           # shared helpers
```

### TEA conventions

- Every component has a `Model`, an `Input`/`Msg` enum, and an `update`. Side effects such as network, DB, and the filesystem
  run as Relm4 commands, and their results come back as messages.
- The state transitions of each component live in a **plain function or struct
  that does not depend on GTK**, for example `fn update(state: &mut State, msg: Msg) -> Vec<Effect>`.
  The Relm4 component is a thin adapter: it calls the pure update and then runs the
  returned `Effect`s. This is what makes the TEA logic unit-testable.
- IO is reached only through traits (`Backend`, `Downloader`) or the `Db`, which tests open
  in memory. Pure functions take the current time as a parameter instead of reading the clock.

### Responsive layout

- `AdwNavigationView` for drill-down (list → video detail → player).
- Top-level sections are Feed, Search, and Library, and they live in an `AdwViewStack`.
  - Library is an `AdwViewStack` of Subscriptions, Playlists, History, and Downloads. Each keeps its
    own header bar, with an `AdwInlineViewSwitcher` below it that shows icons in windows under 800sp.
  - An `AdwMultiLayoutView` provides two layouts, and the stack is placed in each through an `AdwLayoutSlot`:
    - `narrow`: an `AdwViewSwitcherBar` at the bottom.
    - `wide`: an `AdwViewSwitcherSidebar` in an `AdwOverlaySplitView`.
  - Each section is an `AdwToolbarView` with its own header bar, which holds the section's actions
    (refresh, clear, the search entry) and the primary menu.
- An `AdwBreakpoint` (max-width 550sp) switches the layout, rather than size-allocation hacks. Only one
  breakpoint applies at a time, so the narrow one repeats the 800sp one's setters.
- Touch-friendly: list rows have adequate hit targets, and nothing depends on hover.
- Video rows (search, feed, downloads, channels, playlists, related videos) show a 320px wide
  thumbnail beside the title. Rows narrower than 600px stack a full-width thumbnail above the
  title instead, decided by each row's own width rather than by a window breakpoint. History rows
  and playlist rows (channels, library) keep a 160px thumbnail beside the title at every width.
- Lists of video and playlist rows show each row as a separate card; the subscriptions list joins
  its rows.
- Video page: when the page is under 700sp wide, or fullscreen, the player is fixed above the scrolled details.
  On wider pages, the player scrolls with the details inside a clamp. An `AdwMultiLayoutView` with the
  layouts `narrow`, `wide` and `fullscreen` places the player. Below the description, an
  `AdwToggleGroup` switches between comments (shown first) and related videos. Related videos are
  shown instead when a video has no comments.
- Prefer libadwaita patterns over hand-built ones: menus are `gio::Menu` models backed by actions,
  lists are boxed lists of `AdwActionRow`/`AdwExpanderRow`/`AdwButtonRow` (long lists such as the
  feed use a `GtkListView`), loading and error pages are `AdwStatusPage`s, failures that can be
  retried offer a Retry button or an `AdwBanner`, and outcomes the user should know about (or undo)
  are toasts.
- Keyboard shortcuts are listed in the shortcuts dialog (`ui/app.rs`); keep it up to date.
- Videos never start playing by themselves. Opening a video (new or reopened, streamed or
  downloaded) loads it at its resume position, paused.
- After a playback error (an expired stream URL, a network or decoder failure), the video is
  loaded again at its position with freshly fetched streams, and keeps playing if it was. Only
  repeated errors without progress in between show the error page. After a hardware decoder
  fails, videos are decoded in software until the app restarts.
- While a video plays, the session is kept from idling and suspending (`gtk::Application::inhibit`).
- Player controls hide during playback. A tap toggles them; a double tap, `f` or the button toggles
  fullscreen; `Esc` leaves fullscreen; `Space` plays or pauses.

## Offline demo

`cargo run --example demo` starts Flux against the fake backend with an in-memory database
and generated test videos. Search for "demo". Use it to check UI changes without network access,
e.g. under `xvfb-run` with screenshots at phone (360×720) and desktop sizes.

## Building with Nix

A Nix flake (`flake.nix`, with the package in `nix/package.nix`) builds and runs the app on
`x86_64-linux` and `aarch64-linux`. It tracks `nixos-unstable`, which provides the latest GNOME.

```sh
nix run              # build and run Flux
nix build            # build into ./result (runs the test suite)
nix develop          # shell with the Rust toolchain and all system libraries
nix flake update     # bump nixpkgs
```

- The flake only sees files tracked by git. `git add` new files before running `nix build`.
- Whenever a change needs a new system library, GStreamer plugin or runtime tool, add it to
  `nix/package.nix` in the same commit.
- The `src` file set in `nix/package.nix` lists the paths the build needs. Extend it when adding
  top-level directories that the build or the tests read.

## Development process: TDD (mandatory)

Each change follows **red → green → refactor**:

1. **Red**: write a test for the next small behaviour first. Run it and **confirm it
   fails** for the expected reason, not because of a compile error in unrelated code.
2. **Green**: write the minimal implementation that makes the test pass. Run the
   test and confirm it passes.
3. **Refactor**: clean up while all tests stay green.
4. Run `./scripts/check.sh` (fmt check, clippy with `-D warnings`, full test suite)
   before committing. If it fails, do not commit. Check its **exit code**. Piping its output
   through `grep`/`head` hides failures.

The tests are the specification of the behaviour. The implementation is validated
by them, not by manual inspection alone.

### Test guidelines

- Unit tests sit next to the code (`#[cfg(test)] mod tests`). Cross-module tests go in `tests/`.
- **No network in tests.** Use fakes for `Backend`. Feed parsing is tested against
  recorded fixtures in `tests/fixtures/`.
- DB tests use an in-memory SQLite (`Connection::open_in_memory()`) with migrations applied.
- TEA logic is tested by feeding messages to the pure `update` and asserting both the
  resulting state and the emitted effects.
- GTK widget code is kept thin. Where a widget test is needed, it runs under a headless
  display (e.g. `xvfb-run` or `GDK_BACKEND=broadway`) and is marked `#[ignore]` unless the
  environment supports it.
- Tests that hit the real network (e.g. rustypipe smoke tests) are `#[ignore]` and run
  manually.

## Version control

- Use git, and make **one commit per change**: one TDD cycle, or one coherent step.
- A commit message is a **single short line** in imperative mood, e.g. `Add RSS feed parser`.
  It has **no body and no trailers** (no `Co-Authored-By`, no `Signed-off-by`).
- Never commit a red test suite. The red step is run locally, and the test and its implementation are
  committed together once they are green. A setup step with no testable behaviour (e.g. scaffolding, docs) is
  its own commit.

## Milestones

All v1 milestones are implemented:

1. Scaffolding: cargo project, deps, empty adw window, CI-style checks
2. Core domain and DB: migrations, subscriptions, history, and download repos
3. RSS feed parsing and aggregation
4. Backend trait, rustypipe impl, and fake
5. Search UI
6. Video detail and playback (GStreamer)
7. Subscriptions and feed UI
8. History, with resume position
9. Downloads: manager, UI, and offline playback
10. Responsive polish and mobile testing
